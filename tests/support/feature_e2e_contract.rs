#![allow(dead_code)]
//! 功能保真契约（W14）：搜索 / 通知 / mock-codex / tail-f / cat。
//!
//! 无 GTK。夹具一律 `tmux -L muxterm-test-*` + `/bin/cat` 或脚本。
//! `linux_search_e2e` 的 Mock PaneBuf **不算**本契约。

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::tmux_test_support::*;

pub const FEATURE_TIMEOUT: Duration = Duration::from_secs(10);

/// 两 pane `/bin/cat` 工作区：pane0 给搜索，pane1 给后台完成通知。
pub struct TwoPaneCat {
    pub socket: String,
    pub session: String,
    pub panes: [u32; 2],
    pub search_token: String,
    pub bg_token: String,
}

impl Drop for TwoPaneCat {
    fn drop(&mut self) {
        kill_server(&self.socket);
    }
}

impl TwoPaneCat {
    pub fn pane_target(&self, idx: usize) -> String {
        format!("%{}", self.panes[idx])
    }
}

pub fn mock_codex_py() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/scripts/mock_codex.py")
}

pub fn build_two_pane_cat(label: &str) -> TwoPaneCat {
    assert!(tmux_available(), "需要本机 tmux");
    let socket = unique_socket(label);
    let session = format!("feat-{label}");
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(1);

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
        &["split-window", "-t", &session, "-h", "--", "/bin/cat"],
    );
    tmux_ok(&socket, &["select-pane", "-t", &format!("{session}:0.0")]);

    let ids = list_pane_ids(&socket, &format!("{session}:0"));
    assert_eq!(ids.len(), 2, "应有 2 pane: {ids:?}");

    let search_token = format!("SEARCH_LIVE_{suffix}");
    let bg_token = format!("BG_PANE_{suffix}");
    send_keys_literal(&socket, &format!("%{}", ids[0]), &search_token);
    send_keys_literal(&socket, &format!("%{}", ids[1]), &bg_token);
    wait_capture_contains(
        &socket,
        &format!("%{}", ids[0]),
        &search_token,
        Duration::from_secs(3),
    );
    wait_capture_contains(
        &socket,
        &format!("%{}", ids[1]),
        &bg_token,
        Duration::from_secs(3),
    );

    TwoPaneCat {
        socket,
        session,
        panes: [ids[0], ids[1]],
        search_token,
        bg_token,
    }
}

/// 后台 pane：OSC 133 C + 一行 + D（Done，不加 BEL；BEL 会盖成 Blocked）。
///
/// tmux `send-keys` 会把控制字节转成 `^[`/`^G` 字面量，OSC 133 必须由 pane
/// 进程直接写 stdout（与 mock_codex.py 同理），所以这里 respawn 成脚本。
pub fn send_background_task_done(socket: &str, pane_percent: &str) {
    let py = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/scripts/osc133_done.py");
    assert!(py.is_file(), "缺少 {}", py.display());
    let cmd = format!("python3 -u {}", py.display());
    tmux_ok(socket, &["respawn-pane", "-k", "-t", pane_percent, &cmd]);
}

/// OSC 133 D，后面不再写 BEL（前台 Done / 看见即熄）。
pub fn send_command_done_no_bel(socket: &str, pane_percent: &str) {
    let py = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/scripts/osc133_d_only.py");
    assert!(py.is_file(), "缺少 {}", py.display());
    let cmd = format!("python3 -u {}", py.display());
    tmux_ok(socket, &["respawn-pane", "-k", "-t", pane_percent, &cmd]);
}

/// 后台 pane BEL（Blocked / needs attention）。
///
/// pane 必须仍是 `/bin/cat`：`-H` 把 0x07 打进 stdin，cat 原样写出，
/// `%output` 才能带 BEL。不要 respawn（会丢掉 cat）。不要 `-l`（会变成 `^G`）。
///
/// canonical 模式下 cat 要 Enter 才收到输入：只发 `-H 07` 时 `%output` 只有
/// 行规程的 `^G` 回显，没有 `\007`。补一个 Enter 让 cat 真正收到并回显 BEL。
pub fn send_background_bel(socket: &str, pane_percent: &str) {
    send_keys_hex(socket, pane_percent, b"\x07");
    tmux_ok(socket, &["send-keys", "-t", pane_percent, "Enter"]);
}

pub fn respawn_mock_codex(socket: &str, pane_percent: &str) {
    let py = mock_codex_py();
    assert!(py.is_file(), "缺少 {}: 先提交 mock_codex.py", py.display());
    let cmd = format!(
        "MOCK_CODEX_FRAMES=6 MOCK_CODEX_SLEEP=0.03 python3 -u {}",
        py.display()
    );
    tmux_ok(socket, &["respawn-pane", "-k", "-t", pane_percent, &cmd]);
}

pub fn start_tail_f(socket: &str, pane_percent: &str, file: &Path) {
    let cmd = format!("/usr/bin/tail -f {}", file.display());
    tmux_ok(socket, &["respawn-pane", "-k", "-t", pane_percent, &cmd]);
}

pub fn append_line(file: &Path, line: &str) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .expect("append log");
    writeln!(f, "{line}").expect("write log");
    f.flush().expect("flush log");
}
