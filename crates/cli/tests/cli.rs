use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn vault(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ed2k-vault"))
        .arg("scan")
        .args(args)
        .output()
        .unwrap()
}

fn sample_tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join("a b.txt"), b"hello world").unwrap();
    fs::write(dir.path().join("sub/c.log"), b"some log").unwrap();
    fs::write(dir.path().join("empty"), b"").unwrap();
    dir
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn links_go_to_stdout_by_default() {
    let tree = sample_tree();
    let output = vault(&[path(tree.path()), "--no-cache", "--quiet"]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("ed2k://|file|a%20b.txt|11|"));
    assert!(lines[1].starts_with("ed2k://|file|c.log|8|"));
}

#[test]
fn output_file_and_format_are_guessed_from_the_extension() {
    let tree = sample_tree();
    let out = tree.path().join("..").join(format!(
        "ed2k-vault-test-{}.html",
        tree.path().file_name().unwrap().to_string_lossy()
    ));
    let output = vault(&[path(tree.path()), "--out", path(&out), "--no-cache"]);
    assert!(output.status.success());
    let html = fs::read_to_string(&out).unwrap();
    fs::remove_file(&out).unwrap();
    assert!(html.contains("<a href=\"ed2k://|file|a%20b.txt|11|"));
    assert!(html.trim_end().ends_with("</html>"));
}

#[test]
fn filters_are_applied() {
    let tree = sample_tree();
    let output = vault(&[
        path(tree.path()),
        "--no-cache",
        "--quiet",
        "--exclude",
        "*.log",
    ]);
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(text.lines().count(), 1);
    assert!(text.contains("a%20b.txt"));
}

#[test]
fn hashset_flag_adds_the_part_list_to_multi_part_files() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("big.bin"), vec![3u8; 9_728_001]).unwrap();
    let with = vault(&[path(dir.path()), "--no-cache", "--quiet", "--hashset"]);
    let without = vault(&[path(dir.path()), "--no-cache", "--quiet"]);
    assert!(String::from_utf8(with.stdout).unwrap().contains("|p="));
    assert!(!String::from_utf8(without.stdout).unwrap().contains("|p="));
}

#[test]
fn second_run_is_served_by_the_cache() {
    let tree = sample_tree();
    let cache = tree.path().join("..").join(format!(
        "ed2k-vault-cache-{}.db",
        tree.path().file_name().unwrap().to_string_lossy()
    ));
    let args = [path(tree.path()), "--cache", path(&cache)];
    let first = vault(&args);
    let second = vault(&args);
    for ext in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{ext}", cache.display()));
    }
    assert!(String::from_utf8(first.stderr)
        .unwrap()
        .contains("(0 from cache)"));
    assert!(String::from_utf8(second.stderr)
        .unwrap()
        .contains("(2 from cache)"));
    assert_eq!(first.stdout, second.stdout);
}

#[test]
fn missing_folder_fails_with_a_message() {
    let output = vault(&["definitely/not/here", "--no-cache"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .starts_with("error:"));
}

#[test]
fn collection_needs_an_output_file() {
    let tree = sample_tree();
    let output = vault(&[path(tree.path()), "--format", "collection", "--no-cache"]);
    assert_eq!(output.status.code(), Some(1));
}

fn compare(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ed2k-vault"))
        .arg("compare")
        .args(args)
        .output()
        .unwrap()
}

/// A folder, its backup list, and the folder after the names were lost.
fn lost_names() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("Album 01.cbz"), b"first file").unwrap();
    fs::write(dir.path().join("Album 02.cbz"), b"second file").unwrap();
    fs::write(dir.path().join("Album 03.cbz"), b"third file").unwrap();
    let list = dir.path().join("..").join(format!(
        "ed2k-vault-list-{}.txt",
        dir.path().file_name().unwrap().to_string_lossy()
    ));
    let output = vault(&[path(dir.path()), "--no-cache", "--quiet"]);
    fs::write(&list, output.stdout).unwrap();
    fs::rename(dir.path().join("Album 01.cbz"), dir.path().join("scan-1")).unwrap();
    fs::remove_file(dir.path().join("Album 03.cbz")).unwrap();
    (dir, list)
}

#[test]
fn compare_reports_without_touching_anything_by_default() {
    let (dir, list) = lost_names();
    let output = compare(&[path(&list), path(dir.path()), "--no-cache", "--quiet"]);
    let _ = fs::remove_file(&list);
    assert!(output.status.success());
    assert!(dir.path().join("scan-1").is_file());
    assert!(!dir.path().join("Album 01.cbz").exists());
}

#[test]
fn compare_rename_restores_names_and_exports_missing_links() {
    let (dir, list) = lost_names();
    let missing = dir.path().join("..").join(format!(
        "ed2k-vault-missing-{}.txt",
        dir.path().file_name().unwrap().to_string_lossy()
    ));
    let output = compare(&[
        path(&list),
        path(dir.path()),
        "--no-cache",
        "--rename",
        "--missing-out",
        path(&missing),
    ]);
    let _ = fs::remove_file(&list);
    let missing_links = fs::read_to_string(&missing).unwrap_or_default();
    let _ = fs::remove_file(&missing);

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("scan-1 -> Album 01.cbz"));
    assert!(stdout.contains("1 renamed"));
    assert_eq!(
        fs::read(dir.path().join("Album 01.cbz")).unwrap(),
        b"first file"
    );
    assert!(!dir.path().join("scan-1").exists());
    assert_eq!(missing_links.lines().count(), 1);
    assert!(missing_links.contains("Album%2003.cbz"));
}

#[test]
fn compare_fails_on_a_file_without_links() {
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("nothing.txt");
    fs::write(&empty, "no link in here").unwrap();
    let output = compare(&[path(&empty), path(dir.path()), "--no-cache"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("no ed2k file link"));
}
