#![allow(dead_code)]
//! W16a：attach 之前 pane 里已经有滚出可见区的历史。
//!
//! 无 GTK。夹具一律 `tmux -L muxterm-test-*` + `/bin/cat`。

use std::time::Duration;

use super::tmux_test_support::*;

pub const HISTORY_TIMEOUT: Duration = Duration::from_secs(10);
pub const OFFSCREEN_LINES: u32 = 40;

pub struct HistoryPane {
    pub socket: String,
    pub session: String,
    pub pane: u32,
    pub token: String,
    pub tail_mark: String,
}

impl Drop for HistoryPane {
    fn drop(&mut self) {
        kill_server(&self.socket);
    }
}

impl HistoryPane {
    pub fn pane_target(&self) -> String {
        format!("%{}", self.pane)
    }
}

/// 80×24 的 `/bin/cat` pane：先写离屏 token，再写 40 行 padding。
///
/// 返回前必须：可见 `capture-pane -p` 不含 token，`-S -` 含 token。
pub fn build_offscreen_history(label: &str) -> HistoryPane {
    assert!(tmux_available(), "需要本机 tmux");
    let socket = unique_socket(label);
    let session = format!("hist-{label}");
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(1);
    let token = format!("HIST_OFFSCREEN_{suffix}");
    let tail_mark = format!("HIST_TAIL_{suffix}");

    let output = std::process::Command::new("tmux")
        .args([
            "-L",
            &socket,
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            &session,
            "-x",
            "80",
            "-y",
            "24",
            "--",
            "/bin/cat",
        ])
        .output()
        .expect("new-session");
    assert!(
        output.status.success(),
        "new-session 失败: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    tmux_ok(
        &socket,
        &["set-option", "-t", &session, "history-limit", "2000"],
    );

    let ids = list_pane_ids(&socket, &session);
    assert_eq!(ids.len(), 1, "应有 1 pane: {ids:?}");
    let pane = ids[0];
    let target = format!("%{pane}");

    send_keys_line(&socket, &target, &token);
    for i in 1..=OFFSCREEN_LINES {
        send_keys_line(&socket, &target, &format!("pad-{i:02}"));
    }
    send_keys_line(&socket, &target, &tail_mark);

    wait_for(Duration::from_secs(3), "history 含离屏 token", || {
        capture_pane_history(&socket, &target).contains(&token)
    });
    wait_capture_contains(&socket, &target, &tail_mark, Duration::from_secs(3));

    let visible = capture_pane(&socket, &target);
    assert!(
        !visible.contains(&token),
        "夹具失败：token 还在可见屏，无法证明 -S 历史。visible={visible:?}"
    );
    assert!(
        visible.contains(&tail_mark),
        "夹具失败：可见屏应有尾标 {tail_mark}。visible={visible:?}"
    );

    HistoryPane {
        socket,
        session,
        pane,
        token,
        tail_mark,
    }
}
