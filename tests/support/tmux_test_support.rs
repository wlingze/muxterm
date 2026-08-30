#![allow(dead_code)]
//! tmux 测试支持：管理独立 tmux socket/session，硬超时，自动清理。

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 唯一的 tmux socket 名（绝不复用宿主默认 socket）。
pub fn unique_socket(label: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("muxterm-test-{}-{}", label, nanos)
}

/// 检查 tmux 是否可用。
pub fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// RAII guard for one uniquely named isolated tmux server.
///
/// Tests still create sessions explicitly, but every unwind path cleans the
/// same `-L` server without touching the user's default tmux server.
pub struct TmuxServerGuard {
    socket: String,
}

impl TmuxServerGuard {
    pub fn new(label: &str) -> Self {
        Self {
            socket: unique_socket(label),
        }
    }

    pub fn socket(&self) -> &str {
        &self.socket
    }
}

impl Drop for TmuxServerGuard {
    fn drop(&mut self) {
        kill_server(&self.socket);
    }
}

/// 在独立 socket 创建 detached session。
pub fn create_session(socket: &str, name: &str, cols: u32, rows: u32) {
    let output = Command::new("tmux")
        .args([
            "-L",
            socket,
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            name,
            "-x",
            &cols.to_string(),
            "-y",
            &rows.to_string(),
        ])
        .output()
        .expect("创建 tmux session 失败");
    assert!(
        output.status.success(),
        "tmux new-session 失败: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// kill tmux server（清理独立 socket 上的所有 session）。
///
/// `output()` 无超时：残留 control client 会让 `kill-server` 一直等，
/// CI job 就被矩阵格拖到 45 分钟取消。spawn + try_wait 有界回收。
pub fn kill_server(socket: &str) {
    let Ok(mut child) = Command::new("tmux")
        .args(["-L", socket, "kill-server"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => break,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// 在指定 session 的 pane 执行 send-keys。
pub fn send_keys(socket: &str, session: &str, keys: &str) {
    let _ = Command::new("tmux")
        .args(["-L", socket, "send-keys", "-t", session, keys, "Enter"])
        .output();
}

/// capture-pane 获取文本。
pub fn capture_pane(socket: &str, target: &str) -> String {
    let output = Command::new("tmux")
        .args(["-L", socket, "capture-pane", "-p", "-t", target])
        .output()
        .expect("capture-pane 失败");
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// `capture-pane -p -S -`：可见屏 + 全部 scrollback。
pub fn capture_pane_history(socket: &str, target: &str) -> String {
    let output = Command::new("tmux")
        .args(["-L", socket, "capture-pane", "-p", "-S", "-", "-t", target])
        .output()
        .expect("capture-pane -S - 失败");
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// 硬超时 wrapper：在 deadline 内执行 fn，超时 panic。
pub fn run_with_timeout<F, T>(timeout: Duration, label: &str, f: F) -> T
where
    F: FnOnce() -> T + Send,
    T: Send,
{
    let deadline = Instant::now() + timeout;
    let result = std::thread::scope(|s| {
        let h = s.spawn(|| {
            // Check if we're past deadline before even starting
            if Instant::now() >= deadline {
                panic!("测试超时（{}s）：{}", timeout.as_secs(), label);
            }
            f()
        });
        loop {
            if h.is_finished() {
                break;
            }
            if Instant::now() >= deadline {
                panic!("测试超时（{}s）：{}", timeout.as_secs(), label);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        h.join().unwrap()
    });
    result
}

/// 等待条件满足，硬超时。
pub fn wait_for<F>(timeout: Duration, label: &str, mut check: F)
where
    F: FnMut() -> bool,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("等待超时（{}s）：{}", timeout.as_secs(), label);
}

/// 带 `-L` 跑一条 tmux 命令；失败时带 stderr panic。
pub fn tmux_ok(socket: &str, args: &[&str]) {
    let output = Command::new("tmux")
        .arg("-L")
        .arg(socket)
        .args(args)
        .output()
        .expect("spawn tmux 失败");
    assert!(
        output.status.success(),
        "tmux {args:?} 失败: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `list-panes -F #{pane_id}` → `%N` 去 `%` 后的数字。
pub fn list_pane_ids(socket: &str, target: &str) -> Vec<u32> {
    let output = Command::new("tmux")
        .args(["-L", socket, "list-panes", "-t", target, "-F", "#{pane_id}"])
        .output()
        .expect("list-panes 失败");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let s = line.trim().trim_start_matches('%');
            s.parse().ok()
        })
        .collect()
}

/// 字面写入 pane PTY（给 `/bin/cat` 用，不要 Enter）。
pub fn send_keys_literal(socket: &str, target: &str, text: &str) {
    tmux_ok(socket, &["send-keys", "-t", target, "-l", text]);
}

/// 一行 + Enter。`send-keys -l` 里夹 `\n` 往往滚不出可见区。
pub fn send_keys_line(socket: &str, target: &str, text: &str) {
    send_keys_literal(socket, target, text);
    tmux_ok(socket, &["send-keys", "-t", target, "Enter"]);
}

/// 用 `send-keys -H` 发送原始字节（`-l` 会把 ESC/BEL 转成 `^[`/`^G` 字面量，
/// OSC 133 / BEL 注意力信号必须走 hex 才能原样进 pane）。
pub fn send_keys_hex(socket: &str, target: &str, bytes: &[u8]) {
    let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let mut args = vec!["send-keys", "-t", target, "-H"];
    args.extend(hex.iter().map(String::as_str));
    tmux_ok(socket, &args);
}

/// 用 `send-keys -l` 发送原始字节（测试用；`-H` 会把控制字节转成 `^[`/`^G`）。
pub fn send_keys_raw(socket: &str, target: &str, bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes).to_string();
    tmux_ok(socket, &["send-keys", "-t", target, "-l", &text]);
}

/// 等到 `capture-pane -p` 含 needle。
pub fn wait_capture_contains(socket: &str, target: &str, needle: &str, timeout: Duration) {
    wait_for(timeout, &format!("capture {target} 含 {needle}"), || {
        capture_pane(socket, target).contains(needle)
    });
}

/// 分离该 session 上所有 client（含 muxterm 的 `-CC`），**不**杀 session。
pub fn detach_all_clients(socket: &str, session: &str) {
    tmux_ok(socket, &["detach-client", "-s", session]);
}

/// `tmux has-session -t`（带同一 `-L`）。
pub fn has_session(socket: &str, session: &str) -> bool {
    Command::new("tmux")
        .args(["-L", socket, "has-session", "-t", session])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 把 pane 换成 CUP 洪水脚本，刷完后停在 `/bin/cat`。
pub fn respawn_cup_flood(socket: &str, pane_percent: &str, frames: u32) {
    let script = format!(
        "bash -c 'for i in $(seq 1 {frames}); do printf \"\\033[H\\033[2Jframe-%s\\n\" \"$i\"; done; printf \"FLOOD_DONE\\n\"; exec /bin/cat'"
    );
    tmux_ok(socket, &["respawn-pane", "-k", "-t", pane_percent, &script]);
}
