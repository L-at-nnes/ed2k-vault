//! File selection rules applied while walking the tree.

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::error::{Error, Result};

/// User-facing filter settings.
#[derive(Debug, Clone, Default)]
pub struct FilterConfig {
    /// Only keep these extensions (without the dot). Empty keeps everything.
    pub include_ext: Vec<String>,
    /// Drop these extensions (without the dot).
    pub exclude_ext: Vec<String>,
    /// Glob patterns dropping files, matched on the name or the relative path.
    pub exclude: Vec<String>,
    /// Glob patterns dropping whole directories, matched on the name or the
    /// relative path.
    pub exclude_dirs: Vec<String>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    /// Also scan hidden files and directories.
    pub include_hidden: bool,
}

#[derive(Debug)]
pub struct Filters {
    include_ext: Vec<String>,
    exclude_ext: Vec<String>,
    exclude: GlobSet,
    exclude_dirs: GlobSet,
    min_size: Option<u64>,
    max_size: Option<u64>,
    include_hidden: bool,
}

fn glob_set(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = GlobBuilder::new(pattern)
            .case_insensitive(true)
            .build()
            .map_err(|e| Error::Invalid(format!("bad pattern `{pattern}`: {e}")))?;
        builder.add(glob);
    }
    builder.build().map_err(|e| Error::Invalid(e.to_string()))
}

fn normalize_ext(ext: &str) -> String {
    ext.trim().trim_start_matches('.').to_lowercase()
}

impl Filters {
    pub fn new(config: &FilterConfig) -> Result<Self> {
        Ok(Self {
            include_ext: config
                .include_ext
                .iter()
                .map(|e| normalize_ext(e))
                .collect(),
            exclude_ext: config
                .exclude_ext
                .iter()
                .map(|e| normalize_ext(e))
                .collect(),
            exclude: glob_set(&config.exclude)?,
            exclude_dirs: glob_set(&config.exclude_dirs)?,
            min_size: config.min_size,
            max_size: config.max_size,
            include_hidden: config.include_hidden,
        })
    }

    /// Should the walker descend into this directory?
    pub fn dir_allowed(&self, name: &str, relative: &str, hidden: bool) -> bool {
        (self.include_hidden || !hidden)
            && !self.exclude_dirs.is_match(name)
            && !self.exclude_dirs.is_match(relative)
    }

    /// Should this file be hashed?
    pub fn file_allowed(&self, name: &str, relative: &str, size: u64, hidden: bool) -> bool {
        if hidden && !self.include_hidden {
            return false;
        }
        if self.min_size.is_some_and(|min| size < min)
            || self.max_size.is_some_and(|max| size > max)
        {
            return false;
        }
        let ext = name
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_lowercase())
            .unwrap_or_default();
        if !self.include_ext.is_empty() && !self.include_ext.contains(&ext) {
            return false;
        }
        if self.exclude_ext.contains(&ext) {
            return false;
        }
        !self.exclude.is_match(name) && !self.exclude.is_match(relative)
    }
}

/// Parses `1024`, `500k`, `10MB`, `1.5G`, `2TiB`... (1024 based).
pub fn parse_size(text: &str) -> Result<u64> {
    let text = text.trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let value: f64 = number
        .parse()
        .map_err(|_| Error::Invalid(format!("invalid size `{text}`")))?;
    let factor: u64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1 << 10,
        "m" | "mb" | "mib" => 1 << 20,
        "g" | "gb" | "gib" => 1 << 30,
        "t" | "tb" | "tib" => 1 << 40,
        _ => return Err(Error::Invalid(format!("invalid size unit in `{text}`"))),
    };
    Ok((value * factor as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filters(config: FilterConfig) -> Filters {
        Filters::new(&config).unwrap()
    }

    #[test]
    fn default_keeps_everything_visible() {
        let f = filters(FilterConfig::default());
        assert!(f.file_allowed("a.bin", "a.bin", 10, false));
        assert!(!f.file_allowed(".secret", ".secret", 10, true));
    }

    #[test]
    fn hidden_files_can_be_included() {
        let f = filters(FilterConfig {
            include_hidden: true,
            ..Default::default()
        });
        assert!(f.file_allowed(".secret", ".secret", 10, true));
        assert!(f.dir_allowed(".git", ".git", true));
    }

    #[test]
    fn include_and_exclude_extensions_ignore_case_and_dots() {
        let f = filters(FilterConfig {
            include_ext: vec![".MKV".into(), "avi".into()],
            exclude_ext: vec!["avi".into()],
            ..Default::default()
        });
        assert!(f.file_allowed("Movie.mkv", "Movie.mkv", 1, false));
        assert!(!f.file_allowed("Movie.avi", "Movie.avi", 1, false));
        assert!(!f.file_allowed("Movie.txt", "Movie.txt", 1, false));
        assert!(!f.file_allowed("noext", "noext", 1, false));
    }

    #[test]
    fn size_bounds_are_inclusive() {
        let f = filters(FilterConfig {
            min_size: Some(10),
            max_size: Some(20),
            ..Default::default()
        });
        assert!(!f.file_allowed("a", "a", 9, false));
        assert!(f.file_allowed("a", "a", 10, false));
        assert!(f.file_allowed("a", "a", 20, false));
        assert!(!f.file_allowed("a", "a", 21, false));
    }

    #[test]
    fn globs_match_name_or_relative_path() {
        let f = filters(FilterConfig {
            exclude: vec!["*.tmp".into(), "cache/*".into()],
            ..Default::default()
        });
        assert!(!f.file_allowed("x.TMP", "deep/x.TMP", 1, false));
        assert!(!f.file_allowed("y.bin", "cache/y.bin", 1, false));
        assert!(f.file_allowed("y.bin", "other/y.bin", 1, false));
    }

    #[test]
    fn excluded_directories_are_pruned() {
        let f = filters(FilterConfig {
            exclude_dirs: vec!["node_modules".into(), "backup/old".into()],
            ..Default::default()
        });
        assert!(!f.dir_allowed("node_modules", "a/node_modules", false));
        assert!(!f.dir_allowed("old", "backup/old", false));
        assert!(f.dir_allowed("new", "backup/new", false));
    }

    #[test]
    fn invalid_pattern_is_an_error() {
        let config = FilterConfig {
            exclude: vec!["[".into()],
            ..Default::default()
        };
        assert!(Filters::new(&config).is_err());
    }

    #[test]
    fn sizes_parse_with_units() {
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("500k").unwrap(), 512_000);
        assert_eq!(parse_size("10MB").unwrap(), 10 << 20);
        assert_eq!(parse_size("1.5G").unwrap(), 3 << 29);
        assert_eq!(parse_size("2 TiB").unwrap(), 2 << 40);
        assert!(parse_size("abc").is_err());
        assert!(parse_size("5 parsecs").is_err());
    }
}
