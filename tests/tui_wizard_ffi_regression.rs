//! TUI 连接向导 FFI 回归：验证 `muxterm_new_connect` 本地 attach / new-with-cwd。
//!
//! 向导完成时 app.rs 用 `CoreBridge::new_connect` 重连。这里用真实 tmux
//! （独立隔离 socket）验证 attach 与 new（指定起始目录）两条路径的 FFI 建连。

#![cfg(feature = "ffi")]

use std::ffi::CString;
use std::process::Command;
use std::ptr;
use std::time::{Duration, Instant};

use muxterm::core::protocol::ffi::api::{
    muxterm_free, muxterm_get_tabs, muxterm_new_connect, muxterm_poll_events,
};
use muxterm::core::protocol::ffi::types::{CStateChange, CTab};

fn unique_socket(label: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("muxterm-wiz-{}-{}-{}", label, std::process::id(), nanos)
}

fn new_session(socket: &str, name: &str) {
    let out = Command::new("tmux")
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
            "100",
            "-y",
            "30",
        ])
        .output()
        .expect("tmux new-session 失败");
    assert!(out.status.success());
    std::thread::sleep(Duration::from_millis(400));
}

fn list_sessions(socket: &str) -> Vec<String> {
    let out = Command::new("tmux")
        .args(["-L", socket, "list-sessions", "-F", "#{session_name}"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn kill_server(socket: &str) {
    let _ = Command::new("tmux")
        .args(["-L", socket, "kill-server"])
        .output();
}

/// muxterm_new_connect(attach) 成功且能读到 tab。
#[test]
fn new_connect_attach_existing_session() {
    let socket = unique_socket("attach");
    new_session(&socket, "demo");

    let bt = CString::new("tmux").unwrap();
    let sock = CString::new(socket.as_str()).unwrap();
    let sess = CString::new("demo").unwrap();
    let h = muxterm_new_connect(
        bt.as_ptr(),
        sock.as_ptr(),
        sess.as_ptr(),
        ptr::null(),
        ptr::null(),
    );
    assert!(!h.is_null(), "muxterm_new_connect(attach) 应成功");
    unsafe {
        let mut buf = [CStateChange::default(); 64];
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut ok = false;
        while Instant::now() < deadline {
            let _ = muxterm_poll_events(h, buf.as_mut_ptr(), 64);
            let mut tabs = [CTab {
                id: 0,
                name: ptr::null(),
                is_active: 0,
            }; 8];
            if muxterm_get_tabs(h, tabs.as_mut_ptr(), 8) >= 1 {
                ok = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(ok, "attach 后应能读到 tab");
        muxterm_free(h);
    }
    kill_server(&socket);
}

/// muxterm_new_connect(new + cwd) 创建新会话，且 pane cwd 正确。
#[test]
fn new_connect_new_session_with_cwd() {
    let socket = unique_socket("new");
    // 目标目录：临时目录
    let tmp = std::env::temp_dir().join("muxterm-wiz-cwd-test");
    std::fs::create_dir_all(&tmp).unwrap();

    let bt = CString::new("tmux").unwrap();
    let sock = CString::new(socket.as_str()).unwrap();
    let dir = CString::new(tmp.to_string_lossy().as_bytes()).unwrap();
    let h = muxterm_new_connect(
        bt.as_ptr(),
        sock.as_ptr(),
        ptr::null(),
        ptr::null(),
        dir.as_ptr(),
    );
    assert!(!h.is_null(), "muxterm_new_connect(new+cwd) 应成功");
    unsafe {
        let mut buf = [CStateChange::default(); 64];
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut ok = false;
        while Instant::now() < deadline {
            let _ = muxterm_poll_events(h, buf.as_mut_ptr(), 64);
            let mut tabs = [CTab {
                id: 0,
                name: ptr::null(),
                is_active: 0,
            }; 8];
            if muxterm_get_tabs(h, tabs.as_mut_ptr(), 8) >= 1 {
                ok = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(ok, "new 后应能读到 tab");
        muxterm_free(h);
    }
    // 验证新会话存在且 cwd 正确
    let sessions = list_sessions(&socket);
    assert!(!sessions.is_empty(), "应有新 session: {sessions:?}");
    let any = Command::new("tmux")
        .args([
            "-L",
            &socket,
            "display-message",
            "-p",
            "-t",
            sessions[0].as_str(),
            "#{pane_current_path}",
        ])
        .output()
        .unwrap();
    let cwd = String::from_utf8_lossy(&any.stdout).trim().to_string();
    assert!(
        cwd.ends_with("muxterm-wiz-cwd-test") || cwd == tmp.to_string_lossy().as_ref(),
        "pane cwd 应为目标目录: {cwd}"
    );
    kill_server(&socket);
}
