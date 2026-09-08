//! SSH 可达性探测（QuickConnect 红绿灯）。
//!
//! 生产：`ssh -o BatchMode=yes -o ConnectTimeout=2 <alias> true`
//! 不要 `-tt`（探测不需要远端 pty）。不要在 16ms GTK tick 里跑。

use std::time::Duration;

/// SSH 别名探测结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshReach {
    /// 还没探，或超时未归。
    Unknown,
    /// `ssh ... true` 退出 0。
    Ok,
    /// 非 0 / 解析失败 / 超时。
    Err,
}

/// 探测用 ssh 参数（不含 program 名）。
pub fn ssh_probe_args(alias: &str, timeout_secs: u8) -> Vec<String> {
    vec![
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        format!("ConnectTimeout={timeout_secs}"),
        alias.into(),
        "true".into(),
    ]
}

/// 控件名：`muxterm-ssh-dot-{alias}`。
pub fn ssh_dot_widget_name(alias: &str) -> String {
    format!("muxterm-ssh-dot-{alias}")
}

/// CSS class：ok / err / unknown。
pub fn ssh_dot_css_class(reach: SshReach) -> &'static str {
    match reach {
        SshReach::Ok => "muxterm-ssh-dot-ok",
        SshReach::Err => "muxterm-ssh-dot-err",
        SshReach::Unknown => "muxterm-ssh-dot-unknown",
    }
}

/// 退出码 → 可达性。None = 还在跑/超时。
pub fn classify_ssh_probe(status: Option<i32>) -> SshReach {
    match status {
        Some(0) => SshReach::Ok,
        Some(_) => SshReach::Err,
        None => SshReach::Unknown,
    }
}

/// 默认探测超时（ConnectTimeout=2 再留一点）。
pub const SSH_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_args_batch_mode_short_timeout_no_pty() {
        let args = ssh_probe_args("ryzen", 2);
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-o" && w[1] == "BatchMode=yes"),
            "应含 BatchMode=yes: {args:?}"
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-o" && w[1] == "ConnectTimeout=2"),
            "应含 ConnectTimeout=2: {args:?}"
        );
        assert!(args.contains(&"ryzen".into()), "{args:?}");
        assert_eq!(args.last().map(String::as_str), Some("true"));
        assert!(
            !args.iter().any(|a| a == "-tt" || a == "-t"),
            "探测不要分配 pty: {args:?}"
        );
    }

    #[test]
    fn widget_name_and_class() {
        assert_eq!(ssh_dot_widget_name("ryzen"), "muxterm-ssh-dot-ryzen");
        assert_eq!(ssh_dot_css_class(SshReach::Ok), "muxterm-ssh-dot-ok");
        assert_eq!(ssh_dot_css_class(SshReach::Err), "muxterm-ssh-dot-err");
        assert_eq!(
            ssh_dot_css_class(SshReach::Unknown),
            "muxterm-ssh-dot-unknown"
        );
    }

    #[test]
    fn classify_exit() {
        assert_eq!(classify_ssh_probe(Some(0)), SshReach::Ok);
        assert_eq!(classify_ssh_probe(Some(255)), SshReach::Err);
        assert_eq!(classify_ssh_probe(None), SshReach::Unknown);
    }
}
