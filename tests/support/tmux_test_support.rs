#![allow(dead_code)]
//! tmux 测试支持：管理独立 tmux socket/session，硬超时，自动清理。

use std::process::Command;
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
pub fn kill_server(socket: &str) {
    let _ = Command::new("tmux")
        .args(["-L", socket, "kill-server"])
        .output();
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
