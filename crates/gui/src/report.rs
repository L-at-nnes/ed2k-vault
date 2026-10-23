//! Text shown in the window while and after a scan.

use ed2k_vault_core::progress::Snapshot;
use ed2k_vault_core::scan::ScanSummary;

use crate::human;

/// Status line while a scan runs. `speed` is in bytes per second.
pub fn status(snap: &Snapshot, speed: f64) -> String {
    let plus = if snap.walk_finished { "" } else { "+" };
    let mut text = format!(
        "{} / {}{plus} files  ·  {} of {}{plus}",
        snap.files_done,
        snap.files_found,
        human::bytes(snap.bytes_done),
        human::bytes(snap.bytes_found),
    );
    if speed >= 1.0 {
        text.push_str(&format!("  ·  {}/s", human::bytes(speed as u64)));
        if snap.walk_finished {
            let left = snap.bytes_found.saturating_sub(snap.bytes_done);
            text.push_str(&format!(
                "  ·  about {} left",
                human::duration((left as f64 / speed) as u64)
            ));
        }
    }
    text
}

/// One line summing up a finished scan.
pub fn headline(summary: &ScanSummary) -> String {
    let seconds = summary.elapsed.as_secs_f64();
    let verb = if summary.cancelled {
        "Cancelled after"
    } else {
        "Done in"
    };
    format!(
        "{verb} {}: {} links written ({} from the cache), {}",
        human::duration(seconds as u64),
        summary.files_written,
        summary.files_cached,
        human::bytes(summary.bytes),
    )
}

/// Details worth reading: warnings, skipped empty files and every error.
/// Empty when the headline says it all.
pub fn details(summary: &ScanSummary) -> String {
    let mut lines = Vec::new();
    for warning in &summary.warnings {
        lines.push(format!("Warning: {warning}"));
    }
    if !summary.empty_files.is_empty() {
        lines.push(format!(
            "{} empty file(s) skipped (eMule does not share them).",
            summary.empty_files.len()
        ));
    }
    if !summary.errors.is_empty() {
        lines.push(format!("{} error(s):", summary.errors.len()));
        for error in &summary.errors {
            lines.push(format!("  {}: {}", error.path, error.message));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed2k_vault_core::scan::ScanError;
    use std::time::Duration;

    fn summary() -> ScanSummary {
        ScanSummary {
            files_written: 12,
            files_cached: 5,
            bytes: 3 * 1024 * 1024,
            elapsed: Duration::from_secs(75),
            ..ScanSummary::default()
        }
    }

    #[test]
    fn status_marks_a_running_walk_with_a_plus() {
        let snap = Snapshot {
            files_found: 10,
            files_done: 4,
            bytes_found: 2048,
            bytes_done: 1024,
            ..Snapshot::default()
        };
        let text = status(&snap, 0.0);
        assert!(text.starts_with("4 / 10+ files"));
        assert!(!text.contains("left"));
    }

    #[test]
    fn status_shows_speed_and_eta_once_the_walk_is_done() {
        let snap = Snapshot {
            files_found: 10,
            files_done: 4,
            bytes_found: 3 * 1024 * 1024,
            bytes_done: 1024 * 1024,
            walk_finished: true,
            ..Snapshot::default()
        };
        let text = status(&snap, 1024.0 * 1024.0);
        assert!(text.contains("4 / 10 files"));
        assert!(text.contains("1.0 MiB/s"));
        assert!(text.contains("about 2s left"));
    }

    #[test]
    fn headline_reports_totals() {
        assert_eq!(
            headline(&summary()),
            "Done in 1m 15s: 12 links written (5 from the cache), 3.0 MiB"
        );
    }

    #[test]
    fn cancelled_scans_say_so() {
        let mut summary = summary();
        summary.cancelled = true;
        assert!(headline(&summary).starts_with("Cancelled after"));
    }

    #[test]
    fn details_are_empty_for_a_clean_scan() {
        assert_eq!(details(&summary()), "");
    }

    #[test]
    fn details_list_every_error() {
        let mut summary = summary();
        summary.empty_files = vec!["a".into()];
        summary.errors = vec![ScanError {
            path: "x.bin".into(),
            message: "access denied".into(),
        }];
        let text = details(&summary);
        assert!(!text.contains("Done in"));
        assert!(text.contains("1 empty file(s) skipped"));
        assert!(text.contains("1 error(s):\n  x.bin: access denied"));
    }
}
