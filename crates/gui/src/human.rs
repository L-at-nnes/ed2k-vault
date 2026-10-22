//! Human readable sizes and durations for the status line.

pub fn bytes(value: u64) -> String {
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

pub fn duration(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_use_binary_units() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1024), "1.0 KiB");
        assert_eq!(bytes(5 * 1024 * 1024 + 512 * 1024), "5.5 MiB");
        assert_eq!(bytes(3 << 40), "3.0 TiB");
    }

    #[test]
    fn durations_switch_units() {
        assert_eq!(duration(5), "5s");
        assert_eq!(duration(65), "1m 05s");
        assert_eq!(duration(3 * 3600 + 7 * 60), "3h 07m");
    }
}
