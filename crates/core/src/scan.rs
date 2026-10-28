//! Scan orchestration: walk a tree, hash every file and stream the links out.
//!
//! One producer walks the tree, a few reader threads hash files (the heavy
//! lifting happens in the shared pool, see [`crate::hasher`]) and the calling
//! thread writes results in walk order, so the output is deterministic.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, unbounded};
use unicode_normalization::UnicodeNormalization;

use crate::cache::Cache;
use crate::error::{Error, Result};
use crate::filter::{FilterConfig, Filters};
use crate::hasher::{FileDigest, Hasher};
use crate::link::LinkOptions;
use crate::output::{self, Entry, Format, Layout, Target};
use crate::pathutil;
use crate::progress::Progress;
use crate::tree::TreeBuilder;
use crate::walk::{walk, FoundFile, WalkItem};

#[derive(Debug, Clone)]
pub struct ScanConfig {
    pub root: PathBuf,
    pub filters: FilterConfig,
    /// Threads hashing file parts.
    pub hash_threads: usize,
    /// Files read at the same time. Keep it low on hard drives and NAS.
    pub jobs: usize,
    pub format: Format,
    /// `link.aich` also decides whether the AICH root is computed.
    pub link: LinkOptions,
    /// `None` writes to the standard output.
    pub output: Option<PathBuf>,
    /// `None` disables the cache.
    pub cache: Option<PathBuf>,
    /// Folder-aware output options (headings, one collection per folder).
    pub layout: Layout,
    /// Also write the folder structure as a text tree to this file.
    pub tree: Option<PathBuf>,
}

impl ScanConfig {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let hash_threads = default_threads();
        Self {
            root: root.into(),
            filters: FilterConfig::default(),
            hash_threads,
            jobs: default_jobs(hash_threads),
            format: Format::Links,
            link: LinkOptions {
                hashset: false,
                aich: true,
            },
            output: None,
            cache: crate::cache::default_path(),
            layout: Layout::default(),
            tree: None,
        }
    }
}

/// One hashing thread per core, capped.
pub fn default_threads() -> usize {
    thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(16)
}

/// Concurrent files: hard drives and NAS dislike many parallel readers.
pub fn default_jobs(hash_threads: usize) -> usize {
    (hash_threads / 2).clamp(2, 4)
}

#[derive(Debug, Clone)]
pub struct ScanError {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct ScanSummary {
    /// Links written to the output.
    pub files_written: u64,
    /// Of which taken from the cache.
    pub files_cached: u64,
    /// Total size of the written files.
    pub bytes: u64,
    /// Zero-byte files left out.
    pub empty_files: Vec<String>,
    pub errors: Vec<ScanError>,
    /// Non fatal problems (for example an unusable cache).
    pub warnings: Vec<String>,
    pub cancelled: bool,
    pub elapsed: Duration,
}

/// A file whose digest is ready, as handed to the consumer of a scan.
#[derive(Debug, Clone)]
pub struct HashedFile {
    pub path: PathBuf,
    /// Folder below the scanned root, `/` separated (empty for the root).
    pub folder: String,
    /// File name in NFC.
    pub name: String,
    pub digest: FileDigest,
    /// The digest came from the cache.
    pub cached: bool,
}

enum Outcome {
    Hashed(HashedFile),
    Failed(ScanError),
    Empty(String),
    Folder(String),
    Cancelled,
}

struct Context<'a> {
    hasher: &'a Hasher,
    cache: Option<&'a Cache>,
    progress: &'a Progress,
    cancel: &'a AtomicBool,
}

/// Names are written in NFC so macOS (NFD) and other systems agree in every format.
fn nfc(name: &str) -> String {
    name.nfc().collect()
}

/// Folder part of a `/` separated relative path.
fn folder_of(relative: &str) -> String {
    relative
        .rsplit_once('/')
        .map(|(folder, _)| folder.to_string())
        .unwrap_or_default()
}

fn process(ctx: &Context, file: FoundFile) -> Outcome {
    let key = pathutil::display(&file.path);
    ctx.progress.set_last_file(&file.relative);

    if let Some(cache) = ctx.cache {
        if let Some(digest) = cache.get(&key, file.size, file.mtime_ns, ctx.hasher.aich_enabled()) {
            ctx.progress
                .bytes_done
                .fetch_add(file.size, Ordering::Relaxed);
            ctx.progress.files_done.fetch_add(1, Ordering::Relaxed);
            ctx.progress.files_cached.fetch_add(1, Ordering::Relaxed);
            return Outcome::Hashed(HashedFile {
                name: nfc(&file.name),
                folder: folder_of(&file.relative),
                path: file.path,
                digest,
                cached: true,
            });
        }
    }

    let counted = AtomicU64::new(0);
    let on_bytes = |n: u64| {
        counted.fetch_add(n, Ordering::Relaxed);
        ctx.progress.bytes_done.fetch_add(n, Ordering::Relaxed);
        ctx.progress.bytes_read.fetch_add(n, Ordering::Relaxed);
    };
    match ctx.hasher.hash_file(&file.path, ctx.cancel, &on_bytes) {
        Ok(digest) => {
            if let Some(cache) = ctx.cache {
                // A cache write failure only costs a rehash next time.
                let _ = cache.put(&key, file.mtime_ns, &digest);
            }
            ctx.progress.files_done.fetch_add(1, Ordering::Relaxed);
            Outcome::Hashed(HashedFile {
                name: nfc(&file.name),
                folder: folder_of(&file.relative),
                path: file.path,
                digest,
                cached: false,
            })
        }
        Err(Error::Cancelled) => Outcome::Cancelled,
        Err(e) => {
            let missing = file.size.saturating_sub(counted.load(Ordering::Relaxed));
            ctx.progress
                .bytes_done
                .fetch_add(missing, Ordering::Relaxed);
            ctx.progress.files_failed.fetch_add(1, Ordering::Relaxed);
            Outcome::Failed(ScanError {
                path: key,
                message: match e {
                    Error::Io { cause, .. } => cause.to_string(),
                    other => other.to_string(),
                },
            })
        }
    }
}

/// Canonical paths of the files this scan creates (output, cache and its
/// SQLite side files).
fn own_files(config: &ScanConfig) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for path in config
        .output
        .iter()
        .chain(config.cache.iter())
        .chain(config.tree.iter())
    {
        if let Ok(canonical) = pathutil::extended(path) {
            files.push(canonical);
        }
    }
    if let Some(cache) = config
        .cache
        .as_ref()
        .and_then(|p| pathutil::extended(p).ok())
    {
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut name = cache.clone().into_os_string();
            name.push(suffix);
            files.push(PathBuf::from(name));
        }
    }
    files
}

/// Runs a scan. Cancelling through `cancel` ends it cleanly: the output is
/// finalized with what was written so far and the cache keeps every digest
/// already computed, so running the same scan again resumes almost instantly.
pub fn scan(config: &ScanConfig, progress: &Progress, cancel: &AtomicBool) -> Result<ScanSummary> {
    let target = match &config.output {
        Some(path) => Target::File(path),
        None => Target::Stdout,
    };
    let mut writer = output::create_with(config.format, target, config.link, &config.layout)?;
    let result = scan_with(config, progress, cancel, &mut |file| {
        writer.write_entry(&Entry {
            folder: &file.folder,
            name: &file.name,
            digest: &file.digest,
        })
    });
    let finished = writer.finish();
    let summary = result?;
    finished.map_err(|e| Error::Invalid(format!("cannot finish output: {e}")))?;
    Ok(summary)
}

/// Walks and hashes like [`scan`], but hands every file to `on_file` in walk
/// order instead of writing links. `config.output` and `config.format` are
/// ignored (the output file, if any, is still never hashed).
pub fn scan_with(
    config: &ScanConfig,
    progress: &Progress,
    cancel: &AtomicBool,
    on_file: &mut dyn FnMut(&HashedFile) -> std::io::Result<()>,
) -> Result<ScanSummary> {
    let started = Instant::now();
    let root = pathutil::extended(&config.root).map_err(|e| Error::io(&config.root, e))?;
    if !root.is_dir() {
        return Err(Error::Invalid(format!(
            "{} is not a directory",
            config.root.display()
        )));
    }
    let filters = Arc::new(Filters::new(&config.filters)?);
    let mut summary = ScanSummary::default();

    let cache = match &config.cache {
        Some(path) => match Cache::open(path) {
            Ok(cache) => Some(cache),
            Err(e) => {
                summary
                    .warnings
                    .push(format!("cache disabled for this run: {e}"));
                None
            }
        },
        None => None,
    };
    let hasher = Hasher::new(config.hash_threads, config.link.aich)?;
    // The result and the cache may live inside the scanned folder: never hash them.
    let own_files = own_files(config);

    let jobs = config.jobs.max(1);
    let (work_tx, work_rx) = bounded::<(u64, FoundFile)>(jobs * 4);
    let (result_tx, result_rx) = unbounded::<(u64, Outcome)>();
    let ctx = Context {
        hasher: &hasher,
        cache: cache.as_ref(),
        progress,
        cancel,
    };

    let mut write_error: Option<Error> = None;
    let mut tree_text: Option<TreeBuilder> = None;
    thread::scope(|scope| {
        let producer_results = result_tx.clone();
        let root = &root;
        let own_files = &own_files;
        scope.spawn(move || {
            let items = walk(root, filters, jobs.max(2))
                .filter(|item| !matches!(item, WalkItem::File(f) if own_files.contains(&f.path)));
            for (seq, item) in items.enumerate() {
                let seq = seq as u64;
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                match item {
                    WalkItem::File(file) => {
                        progress.files_found.fetch_add(1, Ordering::Relaxed);
                        progress.bytes_found.fetch_add(file.size, Ordering::Relaxed);
                        if work_tx.send((seq, file)).is_err() {
                            break;
                        }
                    }
                    WalkItem::Dir { relative } => {
                        let _ = producer_results.send((seq, Outcome::Folder(relative)));
                    }
                    WalkItem::Empty(path) => {
                        let outcome = Outcome::Empty(pathutil::display(&path));
                        let _ = producer_results.send((seq, outcome));
                    }
                    WalkItem::Error { path, message } => {
                        progress.files_failed.fetch_add(1, Ordering::Relaxed);
                        let outcome = Outcome::Failed(ScanError {
                            path: pathutil::display(&path),
                            message,
                        });
                        let _ = producer_results.send((seq, outcome));
                    }
                }
            }
            progress.walk_finished.store(true, Ordering::Relaxed);
        });

        for _ in 0..jobs {
            let work_rx = work_rx.clone();
            let result_tx = result_tx.clone();
            let ctx = &ctx;
            scope.spawn(move || {
                for (seq, file) in work_rx {
                    let outcome = process(ctx, file);
                    if result_tx.send((seq, outcome)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(work_rx);
        drop(result_tx);

        let mut pending: BTreeMap<u64, Outcome> = BTreeMap::new();
        let mut next = 0u64;
        let mut stopped = false;
        let mut tree = TreeBuilder::default();
        for (seq, outcome) in result_rx {
            pending.insert(seq, outcome);
            while let Some(outcome) = pending.remove(&next) {
                next += 1;
                match outcome {
                    Outcome::Hashed(file) if !stopped => {
                        if let Err(e) = on_file(&file) {
                            write_error = Some(Error::Invalid(format!("cannot write output: {e}")));
                            stopped = true;
                            cancel.store(true, Ordering::Relaxed);
                            continue;
                        }
                        tree.add_file(&file.folder, file.digest.size);
                        summary.files_written += 1;
                        summary.bytes += file.digest.size;
                        if file.cached {
                            summary.files_cached += 1;
                        }
                    }
                    Outcome::Hashed(_) => {}
                    Outcome::Failed(error) => summary.errors.push(error),
                    Outcome::Empty(path) => summary.empty_files.push(path),
                    Outcome::Folder(relative) => tree.add_folder(&relative),
                    Outcome::Cancelled => stopped = true,
                }
            }
        }
        tree_text = Some(tree);
    });

    if let (Some(path), Some(tree)) = (&config.tree, tree_text) {
        let root_name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| pathutil::display(&root));
        if let Err(e) = std::fs::write(path, tree.render(&root_name)) {
            summary.warnings.push(format!("folder tree not saved: {e}"));
        }
    }
    if let Some(cache) = &cache {
        if let Err(e) = cache.flush() {
            summary.warnings.push(format!("cache not saved: {e}"));
        }
    }
    if let Some(e) = write_error {
        return Err(e);
    }
    summary.cancelled = cancel.load(Ordering::Relaxed);
    summary.elapsed = started.elapsed();
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("sub/deeper")).unwrap();
        fs::write(root.join("b file.txt"), b"hello world").unwrap();
        fs::write(root.join("a.bin"), vec![7u8; 5000]).unwrap();
        fs::write(root.join("sub/c.txt"), b"third").unwrap();
        fs::write(root.join("sub/deeper/d.txt"), b"fourth").unwrap();
        fs::write(root.join("empty.dat"), b"").unwrap();
        dir
    }

    fn config(root: &Path, out: &Path) -> ScanConfig {
        let mut config = ScanConfig::new(root);
        config.output = Some(out.to_path_buf());
        config.cache = None;
        config.hash_threads = 2;
        config.jobs = 2;
        config
    }

    fn run(config: &ScanConfig) -> ScanSummary {
        scan(config, &Progress::default(), &AtomicBool::new(false)).unwrap()
    }

    #[test]
    fn links_come_out_in_sorted_walk_order() {
        let dir = tree();
        let out = tempfile::NamedTempFile::new().unwrap();
        let summary = run(&config(dir.path(), out.path()));
        assert_eq!(summary.files_written, 4);
        assert_eq!(summary.empty_files.len(), 1);
        assert!(summary.errors.is_empty());
        assert!(!summary.cancelled);

        let text = fs::read_to_string(out.path()).unwrap();
        let names: Vec<&str> = text.lines().map(|l| l.split('|').nth(2).unwrap()).collect();
        assert_eq!(names, ["a.bin", "b%20file.txt", "c.txt", "d.txt"]);
    }

    #[test]
    fn link_contents_match_the_hash_of_the_data() {
        let dir = tree();
        let out = tempfile::NamedTempFile::new().unwrap();
        run(&config(dir.path(), out.path()));
        let text = fs::read_to_string(out.path()).unwrap();
        let hello = text.lines().find(|l| l.contains("b%20file")).unwrap();
        let expected_hash = crate::ed2k::to_hex(&crate::ed2k::part_hash(b"hello world"));
        let expected_aich = crate::aich::to_base32(&crate::aich::block_hash(b"hello world"));
        assert_eq!(
            hello,
            format!("ed2k://|file|b%20file.txt|11|{expected_hash}|h={expected_aich}|/")
        );
    }

    #[test]
    fn second_run_uses_the_cache() {
        let dir = tree();
        let out = tempfile::NamedTempFile::new().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let mut config = config(dir.path(), out.path());
        config.cache = Some(cache_dir.path().join("cache.db"));

        let first = run(&config);
        assert_eq!(first.files_cached, 0);
        let first_text = fs::read_to_string(out.path()).unwrap();

        let progress = Progress::default();
        let second = scan(&config, &progress, &AtomicBool::new(false)).unwrap();
        assert_eq!(second.files_cached, 4);
        assert_eq!(progress.snapshot().bytes_read, 0);
        assert_eq!(fs::read_to_string(out.path()).unwrap(), first_text);
    }

    #[test]
    fn modified_files_are_rehashed() {
        let dir = tree();
        let out = tempfile::NamedTempFile::new().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let mut config = config(dir.path(), out.path());
        config.cache = Some(cache_dir.path().join("cache.db"));
        run(&config);

        fs::write(dir.path().join("sub/c.txt"), b"third but longer").unwrap();
        let second = run(&config);
        assert_eq!(second.files_cached, 3);
    }

    #[test]
    fn cancelled_scan_still_finalizes_the_output() {
        let dir = tree();
        let out = tempfile::NamedTempFile::new().unwrap();
        let mut config = config(dir.path(), out.path());
        config.format = Format::Html;
        let summary = scan(&config, &Progress::default(), &AtomicBool::new(true)).unwrap();
        assert!(summary.cancelled);
        let text = fs::read_to_string(out.path()).unwrap();
        assert!(text.trim_end().ends_with("</html>"));
    }

    #[test]
    fn output_and_cache_inside_the_scanned_folder_are_not_hashed() {
        let dir = tree();
        let mut config = config(dir.path(), &dir.path().join("links.txt"));
        config.cache = Some(dir.path().join("cache.db"));
        let first = run(&config);
        let second = run(&config);
        assert_eq!(first.files_written, 4);
        assert_eq!(second.files_written, 4);
        assert!(second.errors.is_empty());
        let text = fs::read_to_string(dir.path().join("links.txt")).unwrap();
        assert!(!text.contains("links.txt") && !text.contains("cache.db"));
    }

    #[test]
    fn names_are_written_in_nfc_in_every_format() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("e\u{301}te\u{301}.txt"), b"x").unwrap();
        let out = tempfile::NamedTempFile::new().unwrap();
        let mut config = config(dir.path(), out.path());
        config.format = Format::Csv;
        run(&config);
        let text = fs::read_to_string(out.path()).unwrap();
        assert!(text.contains("\"\u{e9}t\u{e9}.txt\""));
    }

    #[test]
    fn scan_with_hands_every_file_to_the_callback_in_order() {
        let dir = tree();
        let mut config = config(dir.path(), Path::new("unused"));
        config.output = None;
        let mut seen = Vec::new();
        let summary = scan_with(
            &config,
            &Progress::default(),
            &AtomicBool::new(false),
            &mut |file| {
                seen.push((file.name.clone(), file.path.clone(), file.digest.size));
                Ok(())
            },
        )
        .unwrap();
        let names: Vec<&str> = seen.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(names, ["a.bin", "b file.txt", "c.txt", "d.txt"]);
        assert!(seen.iter().all(|(_, path, _)| path.is_file()));
        assert_eq!(summary.files_written, 4);
    }

    #[test]
    fn callback_errors_abort_the_scan() {
        let dir = tree();
        let config = config(dir.path(), Path::new("unused"));
        let result = scan_with(
            &config,
            &Progress::default(),
            &AtomicBool::new(false),
            &mut |_| Err(std::io::Error::other("boom")),
        );
        assert!(result.is_err());
    }

    #[test]
    fn tree_file_lists_every_folder_with_its_counts() {
        let dir = tree();
        fs::create_dir(dir.path().join("hollow")).unwrap();
        let out = tempfile::NamedTempFile::new().unwrap();
        let tree_file = tempfile::NamedTempFile::new().unwrap();
        let mut config = config(dir.path(), out.path());
        config.tree = Some(tree_file.path().to_path_buf());
        run(&config);

        let text = fs::read_to_string(tree_file.path()).unwrap();
        assert!(text
            .lines()
            .next()
            .unwrap()
            .ends_with("/  [2 files, 4.9 KiB]"));
        assert!(text.contains(
            "
  sub/  [1 file, 5 B]"
        ));
        assert!(text.contains(
            "
    deeper/  [1 file, 6 B]"
        ));
        assert!(text.contains("  hollow/  [0 files, 0 B]"));
    }

    #[test]
    fn grouped_html_has_one_heading_per_folder() {
        let dir = tree();
        let out = tempfile::NamedTempFile::new().unwrap();
        let mut config = config(dir.path(), out.path());
        config.format = Format::Html;
        config.layout.group_by_folder = true;
        config.layout.root_name = "Share".into();
        run(&config);
        let html = fs::read_to_string(out.path()).unwrap();
        let headings: Vec<&str> = html.lines().filter(|l| l.starts_with("<h2>")).collect();
        assert_eq!(
            headings,
            ["<h2>Share</h2>", "<h2>sub</h2>", "<h2>sub / deeper</h2>"]
        );
    }

    #[test]
    fn missing_root_is_an_error() {
        let out = tempfile::NamedTempFile::new().unwrap();
        let config = config(Path::new("definitely/not/here"), out.path());
        assert!(scan(&config, &Progress::default(), &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn root_must_be_a_directory() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let out = tempfile::NamedTempFile::new().unwrap();
        let config = config(file.path(), out.path());
        assert!(scan(&config, &Progress::default(), &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn hashset_and_no_aich_options_reach_the_links() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("big.bin"),
            vec![1u8; crate::PART_SIZE as usize + 10],
        )
        .unwrap();
        let out = tempfile::NamedTempFile::new().unwrap();
        let mut config = config(dir.path(), out.path());
        config.link = LinkOptions {
            hashset: true,
            aich: false,
        };
        run(&config);
        let text = fs::read_to_string(out.path()).unwrap();
        assert!(text.contains("|p="));
        assert!(!text.contains("|h="));
    }
}
