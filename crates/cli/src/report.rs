//! Progress bar and end-of-scan summary.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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
