//! 人类可读字节与速率（ls -h：1024，一位小数）。
//!
//! 状态栏 popover 用这个。禁止把累计字节标成 `B/s`。

use std::time::Duration;

/// 累计字节 → `999 B` / `1.5 KB` / `1.0 MB` / `1.0 GB`。
pub fn format_bytes(n: u64) -> String {
    const K: f64 = 1024.0;
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / K)
    } else if n < 1024 * 1024 * 1024 {
        format!("{:.1} MB", n as f64 / (K * K))
    } else {
        format!("{:.1} GB", n as f64 / (K * K * K))
    }
}

/// 字节/秒 → `1.5 KB/s`。
pub fn format_rate(bps: u64) -> String {
    format!("{}/s", format_bytes(bps))
}

/// 两次快照之间的速率。计数回绕或 dt=0 → 0。
pub fn rate_bps(prev: u64, now: u64, dt: Duration) -> u64 {
    if now < prev {
        return 0;
    }
    let secs = dt.as_secs_f64();
    if secs <= 0.0 {
        return 0;
    }
    ((now - prev) as f64 / secs).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_ls_h_1024() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(1_048_576), "1.0 MB");
        assert_eq!(format_bytes(1_073_741_824), "1.0 GB");
    }

    #[test]
    fn format_rate_appends_per_sec() {
        assert_eq!(format_rate(0), "0 B/s");
        assert_eq!(format_rate(56), "56 B/s");
        assert_eq!(format_rate(1536), "1.5 KB/s");
        assert_eq!(format_rate(1_048_576), "1.0 MB/s");
    }

    #[test]
    fn rate_bps_uses_delta() {
        assert_eq!(rate_bps(0, 1536, Duration::from_secs(1)), 1536);
        assert_eq!(rate_bps(100, 100, Duration::from_secs(1)), 0);
        assert_eq!(rate_bps(200, 100, Duration::from_secs(1)), 0);
        assert_eq!(rate_bps(0, 1536, Duration::ZERO), 0);
        assert_eq!(rate_bps(0, 3000, Duration::from_millis(500)), 6000);
    }
}
