//! The settings typed in the window, turned into a scan configuration.

use std::path::{Path, PathBuf};

use ed2k_vault_core::filter::{parse_size, FilterConfig};
use ed2k_vault_core::link::LinkOptions;
use ed2k_vault_core::output::{Format, Layout};
use ed2k_vault_core::scan::ScanConfig;

/// Output formats in the order of the drop-down, with their file extension.
pub const FORMATS: [(&str, Format, &str); 5] = [
    ("Plain links (.txt)", Format::Links, "txt"),
    ("HTML page (.html)", Format::Html, "html"),
    ("URL tags (.txt)", Format::UrlTag, "txt"),
    ("CSV (.csv)", Format::Csv, "csv"),
    (
        "eMule collection (.emulecollection)",
        Format::Collection,
        "emulecollection",
    ),
];

#[derive(Debug, Clone, Default)]
pub struct Form {
    pub root: String,
    pub output: String,
    pub format_index: usize,
    pub hashset: bool,
    pub aich: bool,
    pub hidden: bool,
    pub group_by_folder: bool,
    pub split_collections: bool,
    pub save_tree: bool,
    pub use_cache: bool,
    pub include_ext: String,
    pub exclude_ext: String,
    pub exclude: String,
    pub exclude_dirs: String,
    pub min_size: String,
    pub max_size: String,
    pub threads: usize,
    pub jobs: usize,
}

/// Splits a list typed with commas, semicolons or line breaks.
fn split_list(text: &str) -> Vec<String> {
    text.split([',', ';', '\n'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

fn optional_size(text: &str, label: &str) -> Result<Option<u64>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    parse_size(text)
        .map(Some)
        .map_err(|_| format!("{label}: `{text}` is not a size (try 10MB or 500k)"))
}

/// `path` with the extension of `format`; an empty path stays empty.
pub fn with_format_extension(path: &str, format_index: usize) -> String {
    if path.trim().is_empty() {
        return String::new();
    }
    let extension = FORMATS[format_index.min(FORMATS.len() - 1)].2;
    Path::new(path)
        .with_extension(extension)
        .to_string_lossy()
        .into_owned()
}

/// Where the folder tree is saved: beside the result, or inside the folder of
/// per-folder collections.
pub fn tree_path(output: &str, split_collections: bool) -> PathBuf {
    let output = Path::new(output);
    if split_collections {
        output.join("folder-tree.txt")
    } else {
        let stem = output
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "ed2k-links".into());
        output.with_file_name(format!("{stem}.tree.txt"))
    }
}

/// Format implied by a typed output name; `.txt` is ambiguous, so it implies none.
pub fn format_for_path(path: &str) -> Option<usize> {
    let extension = Path::new(path.trim()).extension()?.to_str()?.to_lowercase();
    match extension.as_str() {
        "html" | "htm" => Some(1),
        "csv" => Some(3),
        "emulecollection" => Some(4),
        _ => None,
    }
}

impl Form {
    pub fn to_config(&self) -> Result<ScanConfig, String> {
        let root = self.root.trim();
        if root.is_empty() {
            return Err("Choose the folder to scan.".into());
        }
        if !Path::new(root).is_dir() {
            return Err(format!("`{root}` is not a folder."));
        }
        let output = self.output.trim();
        if output.is_empty() {
            return Err("Choose where to save the result.".into());
        }

        let mut config = ScanConfig::new(root);
        config.filters = FilterConfig {
            include_ext: split_list(&self.include_ext),
            exclude_ext: split_list(&self.exclude_ext),
            exclude: split_list(&self.exclude),
            exclude_dirs: split_list(&self.exclude_dirs),
            min_size: optional_size(&self.min_size, "Minimum size")?,
            max_size: optional_size(&self.max_size, "Maximum size")?,
            include_hidden: self.hidden,
        };
        config.hash_threads = self.threads.max(1);
        config.jobs = self.jobs.max(1);
        config.format = if self.split_collections {
            Format::Collection
        } else {
            FORMATS[self.format_index.min(FORMATS.len() - 1)].1
        };
        config.layout = Layout {
            group_by_folder: self.group_by_folder,
            split_collections: self.split_collections,
            root_name: Path::new(root)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "root".into()),
        };
        if self.save_tree {
            config.tree = Some(tree_path(output, self.split_collections));
        }
        config.link = LinkOptions {
            hashset: self.hashset,
            aich: self.aich,
        };
        config.output = Some(PathBuf::from(output));
        if !self.use_cache {
            config.cache = None;
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> Form {
        Form {
            root: std::env::temp_dir().to_string_lossy().into_owned(),
            output: "out.txt".into(),
            threads: 4,
            jobs: 2,
            aich: true,
            use_cache: true,
            ..Form::default()
        }
    }

    #[test]
    fn lists_accept_commas_semicolons_and_lines() {
        assert_eq!(
            split_list("mkv, avi;mp4\n iso ,,"),
            ["mkv", "avi", "mp4", "iso"]
        );
        assert!(split_list("  ").is_empty());
    }

    #[test]
    fn valid_form_builds_a_config() {
        let mut form = valid();
        form.hashset = true;
        form.include_ext = "mkv, avi".into();
        form.min_size = "1MB".into();
        form.format_index = 4;
        let config = form.to_config().unwrap();
        assert_eq!(config.format, Format::Collection);
        assert!(config.link.hashset && config.link.aich);
        assert_eq!(config.filters.include_ext, ["mkv", "avi"]);
        assert_eq!(config.filters.min_size, Some(1 << 20));
        assert_eq!((config.hash_threads, config.jobs), (4, 2));
        assert_eq!(config.output, Some(PathBuf::from("out.txt")));
    }

    #[test]
    fn cache_can_be_turned_off() {
        let mut form = valid();
        form.use_cache = false;
        assert!(form.to_config().unwrap().cache.is_none());
    }

    #[test]
    fn missing_folder_or_output_is_reported() {
        let mut form = valid();
        form.root = String::new();
        assert!(form.to_config().unwrap_err().contains("folder"));
        form.root = "definitely/not/here".into();
        assert!(form.to_config().unwrap_err().contains("not a folder"));
        let mut form = valid();
        form.output = " ".into();
        assert!(form.to_config().unwrap_err().contains("save"));
    }

    #[test]
    fn bad_size_names_the_field() {
        let mut form = valid();
        form.max_size = "huge".into();
        assert!(form.to_config().unwrap_err().starts_with("Maximum size"));
    }

    #[test]
    fn folder_options_are_passed_on() {
        let mut form = valid();
        form.group_by_folder = true;
        form.save_tree = true;
        let config = form.to_config().unwrap();
        assert!(config.layout.group_by_folder && !config.layout.split_collections);
        assert_eq!(config.tree, Some(PathBuf::from("out.tree.txt")));
    }

    #[test]
    fn split_collections_force_the_collection_format() {
        let mut form = valid();
        form.format_index = 1;
        form.split_collections = true;
        form.save_tree = true;
        let config = form.to_config().unwrap();
        assert_eq!(config.format, Format::Collection);
        assert!(config.layout.split_collections);
        assert_eq!(config.tree, Some(PathBuf::from("out.txt/folder-tree.txt")));
    }

    #[test]
    fn typed_extension_selects_the_format() {
        assert_eq!(format_for_path("a/links.HTML"), Some(1));
        assert_eq!(format_for_path("x.csv"), Some(3));
        assert_eq!(format_for_path("x.emulecollection"), Some(4));
        assert_eq!(format_for_path("x.txt"), None);
        assert_eq!(format_for_path("noext"), None);
    }

    #[test]
    fn extension_follows_the_chosen_format() {
        assert_eq!(with_format_extension("links.txt", 1), "links.html");
        assert_eq!(with_format_extension("backup", 4), "backup.emulecollection");
        assert_eq!(with_format_extension("", 1), "");
    }
}
