mod args;
mod report;

use std::io::IsTerminal;
use std::process::{Command, ExitCode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use anyhow::{Context, Result};
use clap::Parser;
use ed2k_vault_core::compare::{apply, compare};
use ed2k_vault_core::linklist;
use ed2k_vault_core::progress::Progress;
use ed2k_vault_core::scan::scan;

use args::{Cli, Command as Subcommand, CompareArgs, ScanArgs};

/// The scan finished but some files could not be read.
const EXIT_PARTIAL: u8 = 3;
/// Interrupted with Ctrl-C (the output is complete up to that point).
const EXIT_CANCELLED: u8 = 130;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Subcommand::Scan(args) => run_scan(*args),
        Subcommand::Compare(args) => run_compare(*args),
        Subcommand::Gui => run_gui(),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Runs `job` with Ctrl-C wired to its cancel flag and a progress bar on stderr.
fn with_progress<T: Send>(
    quiet: bool,
    job: impl FnOnce(&Progress, &AtomicBool) -> Result<T> + Send,
) -> Result<T> {
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let cancel = Arc::clone(&cancel);
        ctrlc::set_handler(move || {
            // A second Ctrl-C gives up on the clean shutdown.
            if cancel.swap(true, Ordering::Relaxed) {
                std::process::exit(i32::from(EXIT_CANCELLED));
            }
            eprintln!("\ninterrupted, finishing cleanly (Ctrl-C again to abort)...");
        })
        .context("cannot install the Ctrl-C handler")?;
    }

    let progress = Progress::default();
    let stop = AtomicBool::new(false);
    let show_progress = !quiet && std::io::stderr().is_terminal();
    thread::scope(|scope| {
        if show_progress {
            scope.spawn(|| report::watch(&progress, &stop));
        }
        let result = job(&progress, &cancel);
        stop.store(true, Ordering::Relaxed);
        result
    })
}

fn run_scan(args: ScanArgs) -> Result<u8> {
    let quiet = args.options.quiet;
    let config = args.into_config();
    let summary = with_progress(quiet, |progress, cancel| {
        Ok(scan(&config, progress, cancel)?)
    })?;

    if !quiet {
        report::print_summary(&summary);
    }
    Ok(if summary.cancelled {
        EXIT_CANCELLED
    } else if !summary.errors.is_empty() {
        EXIT_PARTIAL
    } else {
        0
    })
}

fn run_compare(args: CompareArgs) -> Result<u8> {
    let quiet = args.options.quiet;
    let rename = args.rename;
    let missing_out = args.missing_out.clone();
    let report_path = args.report.clone();
    let list = linklist::load(&args.list)?;
    if list.is_empty() {
        anyhow::bail!("no ed2k file link found in {}", args.list.display());
    }
    let config = args.into_config();

    let report = with_progress(quiet, |progress, cancel| {
        Ok(compare(&list, &config, progress, cancel)?)
    })?;
    if report.scan.cancelled {
        // An unfinished scan would make every unseen file look missing.
        eprintln!("cancelled: nothing was renamed or written");
        return Ok(EXIT_CANCELLED);
    }

    let applied = rename.then(|| apply(&report.renames));
    if !quiet {
        report::print_compare(&report, applied.as_ref());
    }
    if let Some(path) = missing_out {
        let links: String = report
            .missing
            .iter()
            .map(|entry| format!("{}\n", entry.link))
            .collect();
        std::fs::write(&path, links).with_context(|| format!("cannot write {}", path.display()))?;
    }
    if let Some(path) = report_path {
        std::fs::write(&path, report::compare_report_text(&report))
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    let failed = applied.is_some_and(|outcome| !outcome.failed.is_empty());
    Ok(if failed || !report.scan.errors.is_empty() {
        EXIT_PARTIAL
    } else {
        0
    })
}

/// Starts `ed2k-vault-gui`, which is installed next to this executable.
fn run_gui() -> Result<u8> {
    let name = format!("ed2k-vault-gui{}", std::env::consts::EXE_SUFFIX);
    let exe = std::env::current_exe()
        .context("cannot locate the running executable")?
        .with_file_name(name);
    Command::new(&exe)
        .spawn()
        .with_context(|| format!("cannot start {}", exe.display()))?;
    Ok(0)
}
