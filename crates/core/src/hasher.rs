//! Single-pass file hashing: ed2k (MD4) and AICH (SHA-1) from one read.
//!
//! The calling thread reads the file sequentially, one part at a time, and
//! hands every part to a shared rayon pool. Part buffers come from a bounded
//! pool shared by all files, so memory stays flat whatever the file size.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::Duration;

use crate::aich::{self, Sha1Digest};
use crate::ed2k::{self, Md4Digest};
use crate::error::{Error, Result};
use crate::pathutil::open_sequential;
use crate::PART_SIZE;

/// Read granularity: large enough for sequential throughput, small enough to
/// react quickly to a cancellation.
const READ_CHUNK: usize = 4 * 1024 * 1024;

/// Everything needed to build a link for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDigest {
    pub size: u64,
    pub hash: Md4Digest,
    /// Part hashes, including the trailing empty-block hash for exact multiples.
    pub part_hashes: Vec<Md4Digest>,
    pub aich: Option<Sha1Digest>,
}

struct PartResult {
    index: usize,
    md4: Md4Digest,
    blocks: Vec<Sha1Digest>,
}

/// Part buffers shared by every file being hashed, so memory stays bounded
/// no matter how many files are read at once.
struct BufferPool {
    free: Mutex<Vec<Vec<u8>>>,
    available: Condvar,
    allocated: Mutex<usize>,
    max: usize,
}

impl BufferPool {
    fn new(max: usize) -> Self {
        Self {
            free: Mutex::new(Vec::new()),
            available: Condvar::new(),
            allocated: Mutex::new(0),
            max,
        }
    }

    /// Blocks until a buffer is available. Buffers are given back by hashing
    /// tasks that never block, so waiting always ends.
    fn acquire(&self, cancel: &AtomicBool) -> Result<Vec<u8>> {
        let mut free = self.free.lock().unwrap();
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            if let Some(buffer) = free.pop() {
                return Ok(buffer);
            }
            {
                let mut allocated = self.allocated.lock().unwrap();
                if *allocated < self.max {
                    *allocated += 1;
                    return Ok(Vec::new());
                }
            }
            free = self
                .available
                .wait_timeout(free, Duration::from_millis(100))
                .unwrap()
                .0;
        }
    }

    fn release(&self, buffer: Vec<u8>) {
        self.free.lock().unwrap().push(buffer);
        self.available.notify_one();
    }
}

pub struct Hasher {
    pool: rayon::ThreadPool,
    buffers: Arc<BufferPool>,
    aich: bool,
}

impl Hasher {
    /// `threads` workers share the hashing of all files; `aich` also computes
    /// the AICH root.
    pub fn new(threads: usize, aich: bool) -> Result<Self> {
        let threads = threads.max(1);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("ed2k-hash-{i}"))
            .build()
            .map_err(|e| Error::Invalid(e.to_string()))?;
        Ok(Self {
            pool,
            buffers: Arc::new(BufferPool::new(threads * 2 + 2)),
            aich,
        })
    }

    pub fn aich_enabled(&self) -> bool {
        self.aich
    }

    /// Hashes `path`. `on_bytes` is called with the size of each chunk read.
    pub fn hash_file(
        &self,
        path: &Path,
        cancel: &AtomicBool,
        on_bytes: &(dyn Fn(u64) + Sync),
    ) -> Result<FileDigest> {
        let io_err = |source| Error::io(path, source);
        let mut file = open_sequential(path).map_err(io_err)?;
        let before = file.metadata().map_err(io_err)?;
        let size = before.len();
        let reads = size.div_ceil(PART_SIZE).max(1);

        let (tx, rx) = mpsc::channel::<PartResult>();
        for index in 0..reads {
            let mut buffer = self.buffers.acquire(cancel)?;
            buffer.resize((size - index * PART_SIZE).min(PART_SIZE) as usize, 0);
            if let Err(e) = read_part(&mut file, &mut buffer, cancel, on_bytes) {
                self.buffers.release(buffer);
                return Err(match e {
                    ReadError::Cancelled => Error::Cancelled,
                    ReadError::Truncated => Error::Modified(path.to_path_buf()),
                    ReadError::Io(source) => Error::io(path, source),
                });
            }

            let tx = tx.clone();
            let buffers = Arc::clone(&self.buffers);
            let with_aich = self.aich;
            let index = index as usize;
            self.pool.spawn(move || {
                let (md4, blocks) = hash_part(&buffer, with_aich);
                buffers.release(buffer);
                let _ = tx.send(PartResult { index, md4, blocks });
            });
        }
        drop(tx);
        let mut slots: Vec<Option<(Md4Digest, Vec<Sha1Digest>)>> = Vec::new();
        slots.resize_with(reads as usize, || None);
        for result in rx {
            slots[result.index] = Some((result.md4, result.blocks));
        }

        let after = file.metadata().map_err(io_err)?;
        if after.len() != size || after.modified().ok() != before.modified().ok() {
            return Err(Error::Modified(path.to_path_buf()));
        }

        let mut part_hashes = Vec::with_capacity(slots.len() + 1);
        let mut blocks = Vec::new();
        for slot in slots {
            let (md4, part_blocks) = slot.expect("every part was hashed");
            part_hashes.push(md4);
            blocks.extend(part_blocks);
        }
        if size > 0 && size % PART_SIZE == 0 {
            part_hashes.push(ed2k::part_hash(b""));
        }
        Ok(FileDigest {
            size,
            hash: ed2k::file_hash(&part_hashes),
            part_hashes,
            aich: self.aich.then(|| aich::root_hash(&blocks, size)),
        })
    }
}

fn hash_part(data: &[u8], with_aich: bool) -> (Md4Digest, Vec<Sha1Digest>) {
    if with_aich {
        rayon::join(|| ed2k::part_hash(data), || aich::block_hashes(data))
    } else {
        (ed2k::part_hash(data), Vec::new())
    }
}

enum ReadError {
    Cancelled,
    Truncated,
    Io(io::Error),
}

fn read_part(
    file: &mut File,
    buffer: &mut [u8],
    cancel: &AtomicBool,
    on_bytes: &(dyn Fn(u64) + Sync),
) -> std::result::Result<(), ReadError> {
    for chunk in buffer.chunks_mut(READ_CHUNK) {
        if cancel.load(Ordering::Relaxed) {
            return Err(ReadError::Cancelled);
        }
        match file.read_exact(chunk) {
            Ok(()) => on_bytes(chunk.len() as u64),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(ReadError::Truncated),
            Err(e) => return Err(ReadError::Io(e)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_file(len: u64, seed: u8) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let chunk: Vec<u8> = (0..65536u32)
            .map(|i| (i as u8).wrapping_add(seed))
            .collect();
        let mut left = len;
        while left > 0 {
            let n = left.min(chunk.len() as u64) as usize;
            file.write_all(&chunk[..n]).unwrap();
            left -= n as u64;
        }
        file.flush().unwrap();
        file
    }

    fn digest(file: &tempfile::NamedTempFile, aich: bool) -> FileDigest {
        let hasher = Hasher::new(3, aich).unwrap();
        hasher
            .hash_file(file.path(), &AtomicBool::new(false), &|_| {})
            .unwrap()
    }

    #[test]
    fn empty_file_is_the_md4_of_nothing() {
        let d = digest(&write_file(0, 0), true);
        assert_eq!(ed2k::to_hex(&d.hash), "31d6cfe0d16ae931b73c59d7e0c089c0");
        assert_eq!(d.part_hashes.len(), 1);
        assert_eq!(d.aich, Some(aich::block_hash(b"")));
    }

    #[test]
    fn small_file_hash_is_plain_md4() {
        let file = write_file(1000, 7);
        let bytes = std::fs::read(file.path()).unwrap();
        let d = digest(&file, true);
        assert_eq!(d.hash, ed2k::part_hash(&bytes));
        assert_eq!(d.aich, Some(aich::block_hash(&bytes)));
        assert_eq!(d.part_hashes, vec![d.hash]);
    }

    #[test]
    fn multi_part_file_matches_the_reference_construction() {
        let file = write_file(PART_SIZE * 2 + 12345, 3);
        let bytes = std::fs::read(file.path()).unwrap();
        let d = digest(&file, true);

        let parts: Vec<Md4Digest> = bytes
            .chunks(PART_SIZE as usize)
            .map(ed2k::part_hash)
            .collect();
        assert_eq!(d.part_hashes, parts);
        assert_eq!(d.hash, ed2k::file_hash(&parts));

        let blocks: Vec<Sha1Digest> = bytes
            .chunks(PART_SIZE as usize)
            .flat_map(aich::block_hashes)
            .collect();
        assert_eq!(d.aich, Some(aich::root_hash(&blocks, bytes.len() as u64)));
    }

    #[test]
    fn exact_multiple_appends_the_empty_block_hash() {
        let file = write_file(PART_SIZE * 2, 5);
        let bytes = std::fs::read(file.path()).unwrap();
        let d = digest(&file, false);
        let mut parts: Vec<Md4Digest> = bytes
            .chunks(PART_SIZE as usize)
            .map(ed2k::part_hash)
            .collect();
        parts.push(ed2k::part_hash(b""));
        assert_eq!(d.part_hashes.len() as u64, ed2k::part_count(PART_SIZE * 2));
        assert_eq!(d.hash, ed2k::file_hash(&parts));
        assert_eq!(d.aich, None);
    }

    #[test]
    fn cancellation_is_reported() {
        let file = write_file(PART_SIZE + 1, 1);
        let hasher = Hasher::new(2, false).unwrap();
        let result = hasher.hash_file(file.path(), &AtomicBool::new(true), &|_| {});
        assert!(matches!(result, Err(Error::Cancelled)));
    }

    #[test]
    fn progress_adds_up_to_the_file_size() {
        use std::sync::atomic::AtomicU64;
        let file = write_file(PART_SIZE + 4321, 9);
        let seen = AtomicU64::new(0);
        let hasher = Hasher::new(2, false).unwrap();
        hasher
            .hash_file(file.path(), &AtomicBool::new(false), &|n| {
                seen.fetch_add(n, Ordering::Relaxed);
            })
            .unwrap();
        assert_eq!(seen.load(Ordering::Relaxed), PART_SIZE + 4321);
    }
}
