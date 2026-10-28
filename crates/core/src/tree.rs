//! Text listing of the folder structure of a scan.

use std::collections::HashMap;

/// Collects folders and per-folder file counts while a scan runs.
#[derive(Debug, Default)]
pub struct TreeBuilder {
    folders: Vec<String>,
    direct: HashMap<String, (u64, u64)>,
}

fn human_bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = value as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

fn plural(count: u64) -> &'static str {
    if count == 1 {
        "file"
    } else {
        "files"
    }
}

impl TreeBuilder {
    /// Registers a folder (relative path with `/`), empty ones included.
    pub fn add_folder(&mut self, relative: &str) {
        self.folders.push(relative.to_string());
    }

    /// Counts a file directly inside `folder` ("" is the root).
    pub fn add_file(&mut self, folder: &str, size: u64) {
        let entry = self.direct.entry(folder.to_string()).or_default();
        entry.0 += 1;
        entry.1 += size;
    }

    fn line(&self, indent: usize, label: &str, folder: &str) -> String {
        let (files, bytes) = self.direct.get(folder).copied().unwrap_or_default();
        format!(
            "{}{label}/  [{files} {}, {}]\n",
            "  ".repeat(indent),
            plural(files),
            human_bytes(bytes)
        )
    }

    /// One line per folder in walk order, indented by depth.
    pub fn render(&self, root_name: &str) -> String {
        let mut text = self.line(0, root_name, "");
        for folder in &self.folders {
            let depth = folder.matches('/').count() + 1;
            let label = folder.rsplit('/').next().unwrap_or(folder);
            text.push_str(&self.line(depth, label, folder));
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_are_indented_by_depth_with_their_own_counts() {
        let mut tree = TreeBuilder::default();
        tree.add_file("", 10);
        tree.add_folder("--- A ---");
        tree.add_folder("--- A ---/Akim");
        tree.add_file("--- A ---/Akim", 2048);
        tree.add_file("--- A ---/Akim", 2048);
        tree.add_folder("--- K ---");

        let text = tree.render("Periodiques");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Periodiques/  [1 file, 10 B]");
        assert_eq!(lines[1], "  --- A ---/  [0 files, 0 B]");
        assert_eq!(lines[2], "    Akim/  [2 files, 4.0 KiB]");
        assert_eq!(lines[3], "  --- K ---/  [0 files, 0 B]");
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn empty_scan_still_lists_the_root() {
        assert_eq!(TreeBuilder::default().render("r"), "r/  [0 files, 0 B]\n");
    }
}
