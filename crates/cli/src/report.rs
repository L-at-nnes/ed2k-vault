//! Progress bar and end-of-scan summary.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ed2k_vault_core::compare::{ApplyOutcome, CompareReport};
use ed2k_vault_core::progress::Progress;
use ed2k_vault_core::scan::ScanSummary;
use indicatif::{HumanBytes, ProgressBar, ProgressDrawTarget, ProgressStyle};

/// Errors printed in full before the rest is only counted.
const MAX_ERRORS_SHOWN: usize = 200;
/// Window used to smooth the throughput.
const SPEED_WINDOW: Duration = Duration::from_secs(2);

/// Redraws a progress bar from the shared counters until `stop` is set.
pub fn watch(progress: &Progress, stop: &AtomicBool) {
    let bar = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr_with_hz(10));
    bar.set_style(
        ProgressStyle::with_template("{spinner} [{bar:32.cyan/blue}] {bytes}/{total_bytes} {msg}")
            .expect("valid template")
            .progress_chars("=> "),
    );

    let mut samples: VecDeque<(Instant, u64)> = VecDeque::new();
    while !stop.load(Ordering::Relaxed) {
        let snap = progress.snapshot();
        let now = Instant::now();
        samples.push_back((now, snap.bytes_read));
        while samples
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > SPEED_WINDOW)
        {
            samples.pop_front();
        }
        let speed = match (samples.front(), samples.back()) {
            (Some((t0, b0)), Some((t1, b1))) if t1 > t0 => {
                (b1 - b0) as f64 / t1.duration_since(*t0).as_secs_f64()
            }
            _ => 0.0,
        };

        bar.set_length(snap.bytes_found);
        bar.set_position(snap.bytes_done.min(snap.bytes_found));
        let discovered = if snap.walk_finished {
            snap.files_found.to_string()
        } else {
            format!("{}+", snap.files_found)
        };
        bar.set_message(format!(
            "{}/{} files, {}/s  {}",
            snap.files_done,
            discovered,
            HumanBytes(speed as u64),
            snap.last_file
        ));
        std::thread::sleep(Duration::from_millis(100));
    }
    bar.finish_and_clear();
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// Prints the outcome of a scan on stderr.
pub fn print_summary(summary: &ScanSummary) {
    let seconds = summary.elapsed.as_secs_f64().max(0.001);
    let rate = (summary.bytes as f64 / seconds) as u64;
    eprintln!(
        "{} written ({} from cache), {} in {:.1}s ({}/s){}",
        plural(summary.files_written as usize, "link", "links"),
        summary.files_cached,
        HumanBytes(summary.bytes),
        seconds,
        HumanBytes(rate),
        if summary.cancelled {
            " - cancelled"
        } else {
            ""
        }
    );
    for warning in &summary.warnings {
        eprintln!("warning: {warning}");
    }
    if !summary.empty_files.is_empty() {
        eprintln!(
            "{} skipped (eMule does not share empty files)",
            plural(summary.empty_files.len(), "empty file", "empty files")
        );
    }
    if !summary.errors.is_empty() {
        eprintln!("{}:", plural(summary.errors.len(), "error", "errors"));
        for error in summary.errors.iter().take(MAX_ERRORS_SHOWN) {
            eprintln!("  {}: {}", error.path, error.message);
        }
        if summary.errors.len() > MAX_ERRORS_SHOWN {
            eprintln!("  ... and {} more", summary.errors.len() - MAX_ERRORS_SHOWN);
        }
    }
}

/// Renames shown on screen before the rest is only counted.
const MAX_RENAMES_SHOWN: usize = 20;

/// Prints what a comparison found. `applied` is `None` for a dry run.
pub fn print_compare(report: &CompareReport, applied: Option<&ApplyOutcome>) {
    let matched = report.identical as usize + report.renames.len() + report.skipped.len();
    println!(
        "{matched} files of the folder are in the list: {} already named right, {} to rename, {} left alone.",
        report.identical,
        report.renames.len(),
        report.skipped.len()
    );
    println!(
        "{} listed files are not in the folder, {} files of the folder are not in the list.",
        report.missing.len(),
        report.unlisted.len()
    );
    for plan in report.renames.iter().take(MAX_RENAMES_SHOWN) {
        println!("  {} -> {}", plan.old_name, plan.new_name);
    }
    if report.renames.len() > MAX_RENAMES_SHOWN {
        println!(
            "  ... and {} more",
            report.renames.len() - MAX_RENAMES_SHOWN
        );
    }
    for skipped in report.skipped.iter().take(MAX_RENAMES_SHOWN) {
        println!("  skipped {}: {}", skipped.path, skipped.reason);
    }
    match applied {
        None if !report.renames.is_empty() => {
            println!("Nothing was renamed. Add --rename to apply the renames above.");
        }
        None => {}
        Some(outcome) => {
            println!("{} renamed.", outcome.renamed);
            for failed in &outcome.failed {
                println!("  could not rename {}: {}", failed.path, failed.reason);
            }
        }
    }
    if !report.scan.errors.is_empty() {
        println!(
            "{} files or folders could not be read: the list of missing files may be incomplete.",
            report.scan.errors.len()
        );
        for error in report.scan.errors.iter().take(MAX_ERRORS_SHOWN) {
            println!("  {}: {}", error.path, error.message);
        }
    }
}

/// The full report as text, for `--report`.
pub fn compare_report_text(report: &CompareReport) -> String {
    let mut text = String::new();
    text.push_str(&format!("== To rename ({})\n", report.renames.len()));
    for plan in &report.renames {
        text.push_str(&format!("{} -> {}\n", plan.from.display(), plan.new_name));
    }
    text.push_str(&format!("\n== Left alone ({})\n", report.skipped.len()));
    for skipped in &report.skipped {
        text.push_str(&format!("{}: {}\n", skipped.path, skipped.reason));
    }
    text.push_str(&format!(
        "\n== In the list, not in the folder ({})\n",
        report.missing.len()
    ));
    for entry in &report.missing {
        text.push_str(&format!("{}\n", entry.link));
    }
    text.push_str(&format!(
        "\n== In the folder, not in the list ({})\n",
        report.unlisted.len()
    ));
    for path in &report.unlisted {
        text.push_str(&format!("{path}\n"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed2k_vault_core::compare::{RenamePlan, Skipped};
    use ed2k_vault_core::linklist::ListEntry;
    use std::path::PathBuf;

    #[test]
    fn full_report_lists_every_section() {
        let report = CompareReport {
            renames: vec![RenamePlan {
                from: PathBuf::from("d/x1"),
                to: PathBuf::from("d/Album.cbz"),
                old_name: "x1".into(),
                new_name: "Album.cbz".into(),
            }],
            skipped: vec![Skipped {
                path: "d/copy".into(),
                reason: "same content".into(),
            }],
            missing: vec![ListEntry {
                name: "lost.cbz".into(),
                size: 1,
                hash: [0; 16],
                link: "ed2k://|file|lost.cbz|1|00000000000000000000000000000000|/".into(),
            }],
            unlisted: vec!["d/other.txt".into()],
            ..CompareReport::default()
        };
        let text = compare_report_text(&report);
        assert!(text.contains("== To rename (1)\nd/x1 -> Album.cbz"));
        assert!(text.contains("d/copy: same content"));
        assert!(text.contains("== In the list, not in the folder (1)\ned2k://|file|lost.cbz"));
        assert!(text.contains("== In the folder, not in the list (1)\nd/other.txt"));
    }
}
