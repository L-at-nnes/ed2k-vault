mod args;
mod report;

use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use anyhow::{Context, Result};
use clap::Parser;
use ed2k_vault_core::progress::Progress;
use ed2k_vault_core::scan::scan;

use args::{Cli, Command, ScanArgs};

/// The scan finished but some files could not be read.
const EXIT_PARTIAL: u8 = 3;
/// Interrupted with Ctrl-C (the output is complete up to that point).
const EXIT_CANCELLED: u8 = 130;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Scan(args) => run_scan(*args),
        Command::Gui => Err(anyhow::anyhow!("the graphical interface is not built yet")),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run_scan(args: ScanArgs) -> Result<u8> {
    let quiet = args.quiet;
    let config = args.into_config();

    let cancel = Arc::new(AtomicBool::new(false));
    {
        let cancel = Arc::clone(&cancel);
        ctrlc::set_handler(move || {
            // A second Ctrl-C gives up on the clean shutdown.
            if cancel.swap(true, Ordering::Relaxed) {
                std::process::exit(i32::from(EXIT_CANCELLED));
            }
            eprintln!("\ninterrupted, finishing the output (Ctrl-C again to abort)...");
        })
        .context("cannot install the Ctrl-C handler")?;
    }

    let progress = Progress::default();
    let stop = AtomicBool::new(false);
    let show_progress = !quiet && std::io::stderr().is_terminal();
    let summary = thread::scope(|scope| {
        if show_progress {
            scope.spawn(|| report::watch(&progress, &stop));
        }
        let result = scan(&config, &progress, &cancel);
        stop.store(true, Ordering::Relaxed);
        result
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
