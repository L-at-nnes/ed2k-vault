//! Parallel, deterministic directory walk.
//!
//! Directories are read in parallel but entries come out sorted, so the same
//! tree always produces the same output order. Symbolic links are never
//! followed and filters prune the tree while it is being read.

use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use jwalk::{Parallelism, WalkDirGeneric};

use crate::filter::Filters;

/// A file that passed every filter.
#[derive(Debug, Clone)]
pub struct FoundFile {
    pub path: PathBuf,
    /// Path below the walk root, always with `/` separators.
    pub relative: String,
    pub name: String,
    pub size: u64,
    /// Modification time in nanoseconds since the Unix epoch (0 if unknown).
    pub mtime_ns: i64,
}

#[derive(Debug)]
pub enum WalkItem {
    File(FoundFile),
    /// A folder that is walked (empty ones included); `relative` uses `/`.
    Dir {
        relative: String,
    },
    /// A zero-byte file: eMule does not share these, so they are skipped.
    Empty(PathBuf),
    /// An entry that could not be read.
    Error {
        path: PathBuf,
        message: String,
    },
}

#[derive(Debug, Clone, Default)]
enum EntryInfo {
    #[default]
    None,
    File(FoundFile),
    Dir(String),
    Failed(String),
}

type State = ((), EntryInfo);

fn is_hidden(name: &str, metadata: Option<&Metadata>) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    if let Some(metadata) = metadata {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        return metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0;
    }
    let _ = metadata;
    false
}

fn mtime_ns(metadata: &Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

/// Walks `root` (use [`crate::pathutil::extended`] first for long paths).
///
/// `threads` is the number of directory readers.
pub fn walk(
    root: &Path,
    filters: Arc<Filters>,
    threads: usize,
) -> impl Iterator<Item = WalkItem> + Send {
    let base = root.to_path_buf();
    let iter = WalkDirGeneric::<State>::new(root)
        .skip_hidden(false)
        .follow_links(false)
        .parallelism(Parallelism::RayonNewPool(threads.max(1)))
        .process_read_dir(move |depth, _, _, children| {
            // The root itself is handed over once with no depth: never prune it.
            if depth.is_none() {
                return;
            }
            children.retain_mut(|child| {
                let Ok(entry) = child else { return true };
                let file_type = entry.file_type();
                if file_type.is_symlink() {
                    return false;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                let path = entry.path();
                let relative = relative_path(&base, &path);
                let metadata = entry.metadata();
                let hidden = is_hidden(&name, metadata.as_ref().ok());

                if file_type.is_dir() {
                    if filters.dir_allowed(&name, &relative, hidden) {
                        entry.client_state = EntryInfo::Dir(relative);
                    } else {
                        entry.read_children_path = None;
                    }
                    return true;
                }
                if !file_type.is_file() {
                    return false;
                }
                match metadata {
                    Ok(metadata) => {
                        let size = metadata.len();
                        // Empty files are judged like a 1-byte file so they are only
                        // reported when the name rules keep them.
                        if !filters.file_allowed(&name, &relative, size.max(1), hidden) {
                            return false;
                        }
                        entry.client_state = EntryInfo::File(FoundFile {
                            path,
                            relative,
                            name,
                            size,
                            mtime_ns: mtime_ns(&metadata),
                        });
                    }
                    Err(e) => entry.client_state = EntryInfo::Failed(e.to_string()),
                }
                true
            });
            // Files first, then sub-folders, each sorted by name: the files of a
            // folder come out together, which folder-grouped outputs rely on.
            children.sort_by(|a, b| match (a, b) {
                (Ok(a), Ok(b)) => (a.file_type().is_dir(), a.file_name())
                    .cmp(&(b.file_type().is_dir(), b.file_name())),
                (Err(_), Ok(_)) => std::cmp::Ordering::Less,
                (Ok(_), Err(_)) => std::cmp::Ordering::Greater,
                (Err(_), Err(_)) => std::cmp::Ordering::Equal,
            });
        });

    iter.into_iter().filter_map(|entry| match entry {
        // A folder that could not be listed is reported, not silently ignored.
        Ok(entry) if entry.read_children_error.is_some() => Some(WalkItem::Error {
            message: entry
                .read_children_error
                .as_ref()
                .map(error_message)
                .unwrap_or_default(),
            path: entry.path(),
        }),
        Ok(entry) => match entry.client_state.clone() {
            EntryInfo::File(file) if file.size == 0 => Some(WalkItem::Empty(file.path)),
            EntryInfo::File(file) => Some(WalkItem::File(file)),
            EntryInfo::Dir(relative) => Some(WalkItem::Dir { relative }),
            EntryInfo::Failed(message) => Some(WalkItem::Error {
                path: entry.path(),
                message,
            }),
            EntryInfo::None => None,
        },
        Err(e) => Some(WalkItem::Error {
            path: e.path().map(Path::to_path_buf).unwrap_or_default(),
            message: error_message(&e),
        }),
    })
}

/// The OS message without the path jwalk repeats.
fn error_message(error: &jwalk::Error) -> String {
    error
        .io_error()
        .map(|e| e.to_string())
        .unwrap_or_else(|| error.to_string())
}

fn relative_path(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::FilterConfig;
    use std::fs;

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("b/deep")).unwrap();
        fs::create_dir_all(root.join("a")).unwrap();
        fs::create_dir_all(root.join("skip")).unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::write(root.join("z.bin"), b"zz").unwrap();
        fs::write(root.join("a/one.txt"), b"1").unwrap();
        fs::write(root.join("b/deep/two.txt"), b"22").unwrap();
        fs::write(root.join("skip/three.txt"), b"333").unwrap();
        fs::write(root.join(".hidden/four.txt"), b"4444").unwrap();
        fs::write(root.join("empty.bin"), b"").unwrap();
        dir
    }

    fn collect(root: &Path, config: FilterConfig) -> Vec<WalkItem> {
        let filters = Arc::new(Filters::new(&config).unwrap());
        walk(root, filters, 2).collect()
    }

    fn names(items: &[WalkItem]) -> Vec<String> {
        items
            .iter()
            .filter_map(|item| match item {
                WalkItem::File(f) => Some(f.relative.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn files_come_out_sorted_and_hidden_ones_are_skipped() {
        let dir = tree();
        let items = collect(dir.path(), FilterConfig::default());
        assert_eq!(
            names(&items),
            ["z.bin", "a/one.txt", "b/deep/two.txt", "skip/three.txt"]
        );
    }

    #[test]
    fn files_of_a_folder_come_before_its_sub_folders() {
        let dir = tree();
        fs::write(dir.path().join("a/zzz.txt"), b"after").unwrap();
        fs::create_dir(dir.path().join("a/inner")).unwrap();
        fs::write(dir.path().join("a/inner/in.txt"), b"in").unwrap();
        let items = collect(dir.path(), FilterConfig::default());
        let names = names(&items);
        let in_a: Vec<&String> = names.iter().filter(|n| n.starts_with("a/")).collect();
        assert_eq!(in_a, ["a/one.txt", "a/zzz.txt", "a/inner/in.txt"]);
        // root files (z.bin) come before every sub-folder
        assert_eq!(names.first().map(String::as_str), Some("z.bin"));
    }

    #[test]
    fn folders_are_reported_even_when_empty() {
        let dir = tree();
        fs::create_dir(dir.path().join("hollow")).unwrap();
        let items = collect(dir.path(), FilterConfig::default());
        let dirs: Vec<&str> = items
            .iter()
            .filter_map(|i| match i {
                WalkItem::Dir { relative } => Some(relative.as_str()),
                _ => None,
            })
            .collect();
        assert!(dirs.contains(&"hollow"));
        assert!(dirs.contains(&"b/deep"));
        assert!(!dirs.contains(&".hidden"));
    }

    #[test]
    fn empty_files_are_reported_separately() {
        let dir = tree();
        let items = collect(dir.path(), FilterConfig::default());
        let empties = items
            .iter()
            .filter(|i| matches!(i, WalkItem::Empty(p) if p.ends_with("empty.bin")))
            .count();
        assert_eq!(empties, 1);
    }

    #[test]
    fn excluded_directories_are_not_entered() {
        let dir = tree();
        let items = collect(
            dir.path(),
            FilterConfig {
                exclude_dirs: vec!["skip".into()],
                ..Default::default()
            },
        );
        assert!(!names(&items).iter().any(|n| n.starts_with("skip/")));
    }

    #[test]
    fn hidden_directories_are_walked_on_request() {
        let dir = tree();
        let items = collect(
            dir.path(),
            FilterConfig {
                include_hidden: true,
                ..Default::default()
            },
        );
        assert!(names(&items).contains(&".hidden/four.txt".to_string()));
    }

    #[test]
    fn empty_files_obey_the_name_filters() {
        let dir = tree();
        let items = collect(
            dir.path(),
            FilterConfig {
                include_ext: vec!["txt".into()],
                ..Default::default()
            },
        );
        assert!(!items.iter().any(|i| matches!(i, WalkItem::Empty(_))));
    }

    #[test]
    fn extension_and_size_filters_apply() {
        let dir = tree();
        let items = collect(
            dir.path(),
            FilterConfig {
                include_ext: vec!["txt".into()],
                min_size: Some(2),
                ..Default::default()
            },
        );
        assert_eq!(names(&items), ["b/deep/two.txt", "skip/three.txt"]);
    }

    #[test]
    fn file_details_are_filled_in() {
        let dir = tree();
        let items = collect(dir.path(), FilterConfig::default());
        let Some(WalkItem::File(file)) = items
            .iter()
            .find(|i| matches!(i, WalkItem::File(f) if f.name == "one.txt"))
        else {
            panic!("expected one.txt");
        };
        assert_eq!(file.name, "one.txt");
        assert_eq!(file.size, 1);
        assert!(file.mtime_ns > 0);
        assert!(file.path.ends_with("one.txt"));
    }
}
