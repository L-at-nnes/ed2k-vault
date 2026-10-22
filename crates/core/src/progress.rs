//! Live counters shared between a running scan and its UI.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Debug, Default)]
pub struct Progress {
    /// Files accepted by the filters so far (grows while the walk runs).
    pub files_found: AtomicU64,
    pub bytes_found: AtomicU64,
    /// Files whose link was produced (hashed or taken from the cache).
    pub files_done: AtomicU64,
    /// Bytes read from disk or skipped thanks to the cache.
    pub bytes_done: AtomicU64,
    /// Bytes actually read from disk (for the throughput).
    pub bytes_read: AtomicU64,
    pub files_cached: AtomicU64,
    pub files_failed: AtomicU64,
    pub walk_finished: AtomicBool,
    last_file: Mutex<String>,
}

/// A consistent copy of the counters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub files_found: u64,
    pub bytes_found: u64,
    pub files_done: u64,
    pub bytes_done: u64,
    pub bytes_read: u64,
    pub files_cached: u64,
    pub files_failed: u64,
    pub walk_finished: bool,
    pub last_file: String,
}

impl Progress {
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            files_found: self.files_found.load(Ordering::Relaxed),
            bytes_found: self.bytes_found.load(Ordering::Relaxed),
            files_done: self.files_done.load(Ordering::Relaxed),
            bytes_done: self.bytes_done.load(Ordering::Relaxed),
            bytes_read: self.bytes_read.load(Ordering::Relaxed),
            files_cached: self.files_cached.load(Ordering::Relaxed),
            files_failed: self.files_failed.load(Ordering::Relaxed),
            walk_finished: self.walk_finished.load(Ordering::Relaxed),
            last_file: self.last_file.lock().unwrap().clone(),
        }
    }

    pub fn set_last_file(&self, name: &str) {
        let mut last = self.last_file.lock().unwrap();
        last.clear();
        last.push_str(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_reflects_the_counters() {
        let progress = Progress::default();
        progress.files_found.fetch_add(3, Ordering::Relaxed);
        progress.bytes_done.fetch_add(10, Ordering::Relaxed);
        progress.walk_finished.store(true, Ordering::Relaxed);
        progress.set_last_file("a.bin");
        let snap = progress.snapshot();
        assert_eq!(snap.files_found, 3);
        assert_eq!(snap.bytes_done, 10);
        assert!(snap.walk_finished);
        assert_eq!(snap.last_file, "a.bin");
    }
}
