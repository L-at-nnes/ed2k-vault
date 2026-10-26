//! Command line definition.

use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use ed2k_vault_core::filter::{parse_size, FilterConfig};
use ed2k_vault_core::link::LinkOptions;
use ed2k_vault_core::output::Format;
use ed2k_vault_core::scan::{default_jobs, default_threads, ScanConfig};

#[derive(Debug, Parser)]
#[command(name = "ed2k-vault", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Hash every file below a folder and write the ed2k links
    Scan(Box<ScanArgs>),
    /// Open the graphical interface
    Gui,
}

#[derive(Debug, Args)]
pub struct ScanArgs {
    /// Folder to scan (local path or UNC path)
    pub root: PathBuf,

    /// Output file; the links go to the standard output when omitted
    #[arg(short, long)]
    pub out: Option<PathBuf>,

    /// links, html, urltag, csv or collection (default: guessed from --out)
    #[arg(short, long)]
    pub format: Option<Format>,

    /// Add the complete hashset (`p=hash1:hash2:...`) to each link
    #[arg(long)]
    pub hashset: bool,

    /// Skip the AICH hash (faster, no `h=` in the links)
    #[arg(long)]
    pub no_aich: bool,

    #[command(flatten)]
    pub options: SharedOptions,
}

/// Filters, performance and cache settings shared by the commands that walk a folder.
#[derive(Debug, Args)]
pub struct SharedOptions {
    /// Only keep these extensions, comma separated (mkv,avi)
    #[arg(long, value_delimiter = ',')]
    pub include_ext: Vec<String>,

    /// Drop these extensions, comma separated (tmp,part)
    #[arg(long, value_delimiter = ',')]
    pub exclude_ext: Vec<String>,

    /// Drop files matching a glob, on the name or the relative path (repeatable)
    #[arg(long)]
    pub exclude: Vec<String>,

    /// Skip directories matching a glob (repeatable)
    #[arg(long)]
    pub exclude_dir: Vec<String>,

    /// Skip files smaller than this (10MB, 500k...)
    #[arg(long, value_parser = parse_size_arg)]
    pub min_size: Option<u64>,

    /// Skip files larger than this
    #[arg(long, value_parser = parse_size_arg)]
    pub max_size: Option<u64>,

    /// Also scan hidden files and folders
    #[arg(long)]
    pub hidden: bool,

    /// Hashing threads (default: one per core, up to 16)
    #[arg(long)]
    pub threads: Option<usize>,

    /// Files read at the same time (default: 2 to 4; keep it low on HDD/NAS)
    #[arg(long)]
    pub jobs: Option<usize>,

    /// Cache file (default: in the user cache folder)
    #[arg(long, conflicts_with = "no_cache")]
    pub cache: Option<PathBuf>,

    /// Do not use the incremental cache
    #[arg(long)]
    pub no_cache: bool,

    /// No progress bar and no summary
    #[arg(short, long)]
    pub quiet: bool,
}

fn parse_size_arg(text: &str) -> Result<u64, String> {
    parse_size(text).map_err(|e| e.to_string())
}

/// Format deduced from the extension of the output file.
fn format_from_path(path: &Path) -> Format {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    ext.parse().unwrap_or(Format::Links)
}

impl SharedOptions {
    /// Applies the filters, thread counts and cache choice to `config`.
    pub fn apply(self, config: &mut ScanConfig) {
        let hash_threads = self.threads.unwrap_or_else(default_threads).max(1);
        config.filters = FilterConfig {
            include_ext: self.include_ext,
            exclude_ext: self.exclude_ext,
            exclude: self.exclude,
            exclude_dirs: self.exclude_dir,
            min_size: self.min_size,
            max_size: self.max_size,
            include_hidden: self.hidden,
        };
        config.hash_threads = hash_threads;
        config.jobs = self
            .jobs
            .unwrap_or_else(|| default_jobs(hash_threads))
            .max(1);
        if self.no_cache {
            config.cache = None;
        } else if let Some(path) = self.cache {
            config.cache = Some(path);
        }
    }
}

impl ScanArgs {
    pub fn into_config(self) -> ScanConfig {
        let format = self
            .format
            .or_else(|| self.out.as_deref().map(format_from_path))
            .unwrap_or(Format::Links);
        let mut config = ScanConfig::new(self.root);
        self.options.apply(&mut config);
        config.format = format;
        config.link = LinkOptions {
            hashset: self.hashset,
            aich: !self.no_aich,
        };
        config.output = self.out;
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(args: &[&str]) -> ScanConfig {
        let mut full = vec!["ed2k-vault", "scan"];
        full.extend_from_slice(args);
        match Cli::try_parse_from(full).unwrap().command {
            Command::Scan(args) => args.into_config(),
            Command::Gui => panic!("expected scan"),
        }
    }

    #[test]
    fn format_is_guessed_from_the_output_extension() {
        assert_eq!(config(&["d", "--out", "x.html"]).format, Format::Html);
        assert_eq!(config(&["d", "--out", "x.csv"]).format, Format::Csv);
        assert_eq!(
            config(&["d", "--out", "x.emulecollection"]).format,
            Format::Collection
        );
        assert_eq!(config(&["d", "--out", "x.txt"]).format, Format::Links);
        assert_eq!(config(&["d", "--out", "x.weird"]).format, Format::Links);
        assert_eq!(config(&["d"]).format, Format::Links);
    }

    #[test]
    fn explicit_format_wins() {
        let config = config(&["d", "--out", "x.html", "--format", "urltag"]);
        assert_eq!(config.format, Format::UrlTag);
    }

    #[test]
    fn options_reach_the_config() {
        let config = config(&[
            "d",
            "--hashset",
            "--no-aich",
            "--threads",
            "8",
            "--jobs",
            "3",
            "--include-ext",
            "mkv,avi",
            "--exclude",
            "*.tmp",
            "--exclude",
            "~*",
            "--min-size",
            "1MB",
            "--hidden",
            "--no-cache",
        ]);
        assert!(config.link.hashset && !config.link.aich);
        assert_eq!((config.hash_threads, config.jobs), (8, 3));
        assert_eq!(config.filters.include_ext, ["mkv", "avi"]);
        assert_eq!(config.filters.exclude, ["*.tmp", "~*"]);
        assert_eq!(config.filters.min_size, Some(1 << 20));
        assert!(config.filters.include_hidden);
        assert!(config.cache.is_none());
    }

    #[test]
    fn invalid_size_is_rejected() {
        assert!(Cli::try_parse_from(["ed2k-vault", "scan", "d", "--min-size", "lots"]).is_err());
    }
}
