//! Compare a saved list of links with the files of a folder.
//!
//! Files are matched on size and ed2k hash, never on name, so a file whose
//! name was lost can get its name back from the list.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use unicode_normalization::UnicodeNormalization;

use crate::ed2k::Md4Digest;
use crate::error::Result;
use crate::linklist::ListEntry;
use crate::pathutil;
use crate::progress::Progress;
use crate::scan::{scan_with, ScanConfig, ScanSummary};

/// A file that is in the list under another name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenamePlan {
    pub from: PathBuf,
    pub to: PathBuf,
    pub old_name: String,
    pub new_name: String,
}

/// A file or rename that was left alone, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct CompareReport {
    /// Files already named like in the list.
    pub identical: u64,
    pub renames: Vec<RenamePlan>,
    /// Matches that cannot be renamed safely.
    pub skipped: Vec<Skipped>,
    /// Listed files that were not found in the folder.
    pub missing: Vec<ListEntry>,
    /// Files of the folder that are not in the list.
    pub unlisted: Vec<String>,
    pub scan: ScanSummary,
}

type Key = (u64, Md4Digest);

/// The list name made valid for the current file system, or `None` when
/// nothing usable is left.
pub fn target_name(name: &str) -> Option<String> {
    let forbidden: &[char] = if cfg!(windows) {
        &['<', '>', ':', '"', '/', '\\', '|', '?', '*']
    } else {
        &['/']
    };
    let cleaned: String = name
        .nfc()
        .map(|c| {
            if forbidden.contains(&c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = if cfg!(windows) {
        cleaned.trim_end_matches([' ', '.']).to_string()
    } else {
        cleaned
    };
    let cleaned = cleaned.trim_start().to_string();
    (!cleaned.is_empty() && cleaned != "." && cleaned != "..").then_some(cleaned)
}

fn path_key(path: &Path) -> String {
    pathutil::display(path).to_lowercase()
}

/// Hashes the folder of `config` and matches it against `list`.
///
/// Nothing is renamed: pass `report.renames` to [`apply`].
pub fn compare(
    list: &[ListEntry],
    config: &ScanConfig,
    progress: &Progress,
    cancel: &AtomicBool,
) -> Result<CompareReport> {
    let mut config = config.clone();
    config.link.aich = false; // only the ed2k hash is needed to match files

    let mut index: HashMap<Key, &ListEntry> = HashMap::new();
    for entry in list {
        index.entry((entry.size, entry.hash)).or_insert(entry);
    }

    let mut report = CompareReport::default();
    let mut seen: HashSet<Key> = HashSet::new();
    let mut plans: Vec<RenamePlan> = Vec::new();

    report.scan = scan_with(&config, progress, cancel, &mut |file| {
        let key = (file.digest.size, file.digest.hash);
        let Some(entry) = index.get(&key) else {
            report.unlisted.push(pathutil::display(&file.path));
            return Ok(());
        };
        let first_match = seen.insert(key);
        let Some(new_name) = target_name(&entry.name) else {
            report.skipped.push(Skipped {
                path: pathutil::display(&file.path),
                reason: format!("the listed name `{}` is not usable", entry.name),
            });
            return Ok(());
        };
        if file.name == new_name {
            report.identical += 1;
        } else if !first_match {
            report.skipped.push(Skipped {
                path: pathutil::display(&file.path),
                reason: "same content as another file already matched".into(),
            });
        } else {
            plans.push(RenamePlan {
                to: file.path.with_file_name(&new_name),
                from: file.path.clone(),
                old_name: file.name.clone(),
                new_name,
            });
        }
        Ok(())
    })?;

    // Keep only the renames that cannot overwrite anything.
    let mut taken: HashSet<String> = HashSet::new();
    for plan in plans {
        let target = path_key(&plan.to);
        let same_file = target == path_key(&plan.from); // case-only change
        if !same_file && (plan.to.exists() || taken.contains(&target)) {
            report.skipped.push(Skipped {
                path: pathutil::display(&plan.from),
                reason: format!("a file named `{}` already exists", plan.new_name),
            });
        } else {
            taken.insert(target);
            report.renames.push(plan);
        }
    }

    let mut reported: HashSet<Key> = HashSet::new();
    for entry in list {
        let key = (entry.size, entry.hash);
        if !seen.contains(&key) && reported.insert(key) {
            report.missing.push(entry.clone());
        }
    }
    Ok(report)
}

#[derive(Debug, Clone, Default)]
pub struct ApplyOutcome {
    pub renamed: u64,
    pub failed: Vec<Skipped>,
}

/// Performs the planned renames. Nothing is ever overwritten.
pub fn apply(plans: &[RenamePlan]) -> ApplyOutcome {
    let mut outcome = ApplyOutcome::default();
    for plan in plans {
        let case_only = path_key(&plan.to) == path_key(&plan.from);
        let result = if !case_only && plan.to.exists() {
            Err("the target name is now taken".to_string())
        } else {
            std::fs::rename(&plan.from, &plan.to).map_err(|e| e.to_string())
        };
        match result {
            Ok(()) => outcome.renamed += 1,
            Err(reason) => outcome.failed.push(Skipped {
                path: pathutil::display(&plan.from),
                reason,
            }),
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linklist::parse_text;
    use crate::scan::scan;
    use std::fs;

    fn config(root: &Path) -> ScanConfig {
        let mut config = ScanConfig::new(root);
        config.cache = None;
        config.hash_threads = 2;
        config.jobs = 2;
        config
    }

    /// The list of a folder, as a backup would hold it.
    fn list_of(root: &Path) -> Vec<ListEntry> {
        let out = tempfile::NamedTempFile::new().unwrap();
        let mut config = config(root);
        config.output = Some(out.path().to_path_buf());
        scan(&config, &Progress::default(), &AtomicBool::new(false)).unwrap();
        parse_text(&fs::read_to_string(out.path()).unwrap())
    }

    fn run(list: &[ListEntry], root: &Path) -> CompareReport {
        compare(
            list,
            &config(root),
            &Progress::default(),
            &AtomicBool::new(false),
        )
        .unwrap()
    }

    fn folder() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Album 01.cbz"), b"first file").unwrap();
        fs::write(dir.path().join("Album 02.cbz"), b"second file").unwrap();
        fs::write(dir.path().join("Album 03.cbz"), b"third file").unwrap();
        dir
    }

    #[test]
    fn untouched_folder_is_all_identical() {
        let dir = folder();
        let list = list_of(dir.path());
        let report = run(&list, dir.path());
        assert_eq!(report.identical, 3);
        assert!(report.renames.is_empty() && report.missing.is_empty());
        assert!(report.unlisted.is_empty() && report.skipped.is_empty());
    }

    #[test]
    fn renamed_missing_and_unlisted_files_are_told_apart() {
        let dir = folder();
        let list = list_of(dir.path());
        fs::rename(
            dir.path().join("Album 01.cbz"),
            dir.path().join("scan001.bin"),
        )
        .unwrap();
        fs::remove_file(dir.path().join("Album 03.cbz")).unwrap();
        fs::write(dir.path().join("other.txt"), b"not in the list").unwrap();

        let report = run(&list, dir.path());
        assert_eq!(report.identical, 1);
        assert_eq!(report.renames.len(), 1);
        assert_eq!(report.renames[0].old_name, "scan001.bin");
        assert_eq!(report.renames[0].new_name, "Album 01.cbz");
        assert_eq!(report.missing.len(), 1);
        assert_eq!(report.missing[0].name, "Album 03.cbz");
        assert_eq!(report.unlisted.len(), 1);
        assert!(report.unlisted[0].ends_with("other.txt"));
    }

    #[test]
    fn applying_the_plan_restores_the_names() {
        let dir = folder();
        let list = list_of(dir.path());
        fs::rename(dir.path().join("Album 01.cbz"), dir.path().join("x1")).unwrap();
        fs::rename(dir.path().join("Album 02.cbz"), dir.path().join("x2")).unwrap();

        let report = run(&list, dir.path());
        let outcome = apply(&report.renames);
        assert_eq!(outcome.renamed, 2);
        assert!(outcome.failed.is_empty());
        assert_eq!(
            fs::read(dir.path().join("Album 01.cbz")).unwrap(),
            b"first file"
        );
        assert!(!dir.path().join("x1").exists());
        assert_eq!(run(&list, dir.path()).identical, 3);
    }

    #[test]
    fn files_in_subfolders_are_renamed_in_place() {
        let dir = folder();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/deep.txt"), b"deep content").unwrap();
        let list = list_of(dir.path());
        fs::rename(dir.path().join("sub/deep.txt"), dir.path().join("sub/zzz")).unwrap();

        let report = run(&list, dir.path());
        assert_eq!(apply(&report.renames).renamed, 1);
        assert!(dir.path().join("sub/deep.txt").is_file());
    }

    #[test]
    fn duplicate_content_is_never_renamed_twice() {
        let dir = folder();
        let list = list_of(dir.path());
        fs::rename(dir.path().join("Album 01.cbz"), dir.path().join("copy-a")).unwrap();
        fs::write(dir.path().join("copy-b"), b"first file").unwrap();

        let report = run(&list, dir.path());
        assert_eq!(report.renames.len(), 1);
        assert_eq!(report.renames[0].old_name, "copy-a");
        assert_eq!(report.skipped.len(), 1);
        assert!(report.skipped[0].path.ends_with("copy-b"));
    }

    #[test]
    fn existing_target_blocks_the_rename() {
        let dir = folder();
        let list = list_of(dir.path());
        // a file with other content now sits on the listed name
        fs::rename(dir.path().join("Album 01.cbz"), dir.path().join("misnamed")).unwrap();
        fs::write(dir.path().join("Album 01.cbz"), b"something else").unwrap();

        let report = run(&list, dir.path());
        assert!(report.renames.is_empty());
        assert_eq!(report.skipped.len(), 1);
        assert!(report.skipped[0].reason.contains("already exists"));
        assert!(dir.path().join("misnamed").is_file());
    }

    #[test]
    fn apply_refuses_to_overwrite_a_file_created_since_the_scan() {
        let dir = folder();
        let list = list_of(dir.path());
        fs::rename(dir.path().join("Album 01.cbz"), dir.path().join("late")).unwrap();
        let report = run(&list, dir.path());
        fs::write(dir.path().join("Album 01.cbz"), b"appeared meanwhile").unwrap();

        let outcome = apply(&report.renames);
        assert_eq!(outcome.renamed, 0);
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(
            fs::read(dir.path().join("Album 01.cbz")).unwrap(),
            b"appeared meanwhile"
        );
    }

    #[test]
    fn same_name_different_content_is_not_a_match() {
        let dir = folder();
        let list = list_of(dir.path());
        fs::write(dir.path().join("Album 02.cbz"), b"corrupted copy").unwrap();

        let report = run(&list, dir.path());
        assert_eq!(report.identical, 2);
        assert_eq!(report.missing.len(), 1);
        assert_eq!(report.unlisted.len(), 1);
    }

    #[test]
    fn invalid_characters_are_replaced_in_target_names() {
        assert_eq!(target_name("a/b").as_deref(), Some("a_b"));
        assert_eq!(
            target_name("ok name (1).cbz").as_deref(),
            Some("ok name (1).cbz")
        );
        assert_eq!(target_name("").as_deref(), None);
        assert_eq!(target_name("..").as_deref(), None);
        if cfg!(windows) {
            assert_eq!(target_name("a:b?c*.txt").as_deref(), Some("a_b_c_.txt"));
            assert_eq!(target_name("name. ").as_deref(), Some("name"));
        }
    }
}
