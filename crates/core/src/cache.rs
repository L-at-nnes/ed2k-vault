//! Incremental hash cache (SQLite).
//!
//! A file is identified by its path, size and modification time. As long as
//! those match, its digest is reused instead of reading the file again.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{Error, Result};
use crate::hasher::FileDigest;

const SCHEMA_VERSION: i64 = 1;
/// Rows written between two commits.
const COMMIT_EVERY: usize = 256;

fn cache_err(e: rusqlite::Error) -> Error {
    Error::Cache(e.to_string())
}

/// Default cache location: `<user cache dir>/ed2k-vault/cache.db`.
pub fn default_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|dir| dir.join("ed2k-vault").join("cache.db"))
}

struct Inner {
    conn: Connection,
    uncommitted: usize,
}

pub struct Cache {
    inner: Mutex<Inner>,
}

impl Cache {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        Self::from_connection(Connection::open(path).map_err(cache_err)?)
    }

    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory().map_err(cache_err)?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")
            .map_err(cache_err)?;
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(cache_err)?;
        if version != SCHEMA_VERSION {
            conn.execute_batch("DROP TABLE IF EXISTS files;")
                .map_err(cache_err)?;
        }
        conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS files (
                 path TEXT PRIMARY KEY,
                 size INTEGER NOT NULL,
                 mtime_ns INTEGER NOT NULL,
                 hash BLOB NOT NULL,
                 aich BLOB,
                 parts BLOB NOT NULL
             ) WITHOUT ROWID;
             PRAGMA user_version = {SCHEMA_VERSION};"
        ))
        .map_err(cache_err)?;
        Ok(Self {
            inner: Mutex::new(Inner {
                conn,
                uncommitted: 0,
            }),
        })
    }

    /// The cached digest of `path`, if size and mtime still match. With
    /// `need_aich`, an entry hashed without AICH counts as a miss.
    pub fn get(&self, path: &str, size: u64, mtime_ns: i64, need_aich: bool) -> Option<FileDigest> {
        let inner = self.inner.lock().unwrap();
        let row = inner
            .conn
            .query_row(
                "SELECT hash, aich, parts FROM files
                 WHERE path = ?1 AND size = ?2 AND mtime_ns = ?3",
                params![path, size as i64, mtime_ns],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Option<Vec<u8>>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                },
            )
            .optional()
            .ok()??;
        let (hash, aich, parts) = row;
        if need_aich && aich.is_none() {
            return None;
        }
        Some(FileDigest {
            size,
            hash: hash.try_into().ok()?,
            part_hashes: parts.as_chunks::<16>().0.to_vec(),
            aich: match aich {
                Some(bytes) => Some(bytes.try_into().ok()?),
                None => None,
            },
        })
    }

    /// Stores a digest. Rows are committed in batches; call [`Cache::flush`]
    /// to force them to disk.
    pub fn put(&self, path: &str, mtime_ns: i64, digest: &FileDigest) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if inner.uncommitted == 0 {
            inner.conn.execute_batch("BEGIN").map_err(cache_err)?;
        }
        let parts: Vec<u8> = digest.part_hashes.iter().flatten().copied().collect();
        inner
            .conn
            .execute(
                "INSERT OR REPLACE INTO files (path, size, mtime_ns, hash, aich, parts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    path,
                    digest.size as i64,
                    mtime_ns,
                    digest.hash.as_slice(),
                    digest.aich.as_ref().map(|a| a.as_slice()),
                    parts
                ],
            )
            .map_err(cache_err)?;
        inner.uncommitted += 1;
        if inner.uncommitted >= COMMIT_EVERY {
            inner.conn.execute_batch("COMMIT").map_err(cache_err)?;
            inner.uncommitted = 0;
        }
        Ok(())
    }

    /// Commits the rows written so far.
    pub fn flush(&self) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if inner.uncommitted > 0 {
            inner.conn.execute_batch("COMMIT").map_err(cache_err)?;
            inner.uncommitted = 0;
        }
        Ok(())
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(with_aich: bool) -> FileDigest {
        FileDigest {
            size: 100,
            hash: [7; 16],
            part_hashes: vec![[1; 16], [2; 16]],
            aich: with_aich.then_some([9; 20]),
        }
    }

    #[test]
    fn stored_digest_is_returned_when_key_matches() {
        let cache = Cache::in_memory().unwrap();
        cache.put("/a", 5, &digest(true)).unwrap();
        assert_eq!(cache.get("/a", 100, 5, true), Some(digest(true)));
    }

    #[test]
    fn size_or_mtime_change_is_a_miss() {
        let cache = Cache::in_memory().unwrap();
        cache.put("/a", 5, &digest(true)).unwrap();
        assert_eq!(cache.get("/a", 101, 5, false), None);
        assert_eq!(cache.get("/a", 100, 6, false), None);
        assert_eq!(cache.get("/b", 100, 5, false), None);
    }

    #[test]
    fn missing_aich_is_a_miss_only_when_required() {
        let cache = Cache::in_memory().unwrap();
        cache.put("/a", 5, &digest(false)).unwrap();
        assert_eq!(cache.get("/a", 100, 5, true), None);
        assert_eq!(cache.get("/a", 100, 5, false), Some(digest(false)));
    }

    #[test]
    fn put_replaces_the_previous_row() {
        let cache = Cache::in_memory().unwrap();
        cache.put("/a", 5, &digest(false)).unwrap();
        cache.put("/a", 5, &digest(true)).unwrap();
        assert_eq!(cache.get("/a", 100, 5, true), Some(digest(true)));
    }

    #[test]
    fn rows_survive_reopening_after_flush() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("cache.db");
        {
            let cache = Cache::open(&path).unwrap();
            cache.put("/a", 5, &digest(true)).unwrap();
            cache.flush().unwrap();
        }
        let cache = Cache::open(&path).unwrap();
        assert_eq!(cache.get("/a", 100, 5, true), Some(digest(true)));
    }

    #[test]
    fn many_rows_cross_the_commit_boundary() {
        let cache = Cache::in_memory().unwrap();
        for i in 0..(COMMIT_EVERY * 2 + 3) {
            cache.put(&format!("/f{i}"), 1, &digest(false)).unwrap();
        }
        assert!(cache.get("/f0", 100, 1, false).is_some());
        assert!(cache
            .get(&format!("/f{}", COMMIT_EVERY * 2 + 2), 100, 1, false)
            .is_some());
    }
}
