//! TUI 连接向导 SSH FFI 回归：验证 `muxterm_new_connect("tmux-ssh", ...)` 重连路径。
//!
//! 向导选择 ssh + attach 时，app.rs 用 CoreBridge::new_connect 以 tmux-ssh 后端
//! 建连。这里用真实 loopback sshd + 显式 ssh config（tests/support）验证这条路径
//! 能成功建连并读到远端 tmux 的 tab。

#![cfg(feature = "ffi")]
#![cfg(unix)]

mod support;
use support::sshd_test_support::{sshd_available, SshTestEnv};

use std::ffi::CString;
use std::ptr;
use std::time::{Duration, Instant};

use muxterm::core::protocol::ffi::api::{
    muxterm_free, muxterm_get_tabs, muxterm_new_connect, muxterm_poll_events,
};
use muxterm::core::protocol::ffi::types::{CStateChange, CTab};

/// 用 tmux-ssh 后端 attach 远端 session（muxterm_new_connect）。
///
/// SSH config 通过 `MUXTERM_SSH_CONFIG_PATH` 环境变量传给 spawn_ssh（与 CLI 一致）。
fn ssh_connect_attach(
    alias: &str,
    ssh_config_path: &str,
    remote_socket: &str,
    session: &str,
) -> bool {
    std::env::set_var("MUXTERM_SSH_CONFIG_PATH", ssh_config_path);
    let bt = CString::new("tmux-ssh").unwrap();
    let sock = CString::new(remote_socket).unwrap();
    let sess = CString::new(session).unwrap();
    let host = CString::new(alias).unwrap();
    let h = muxterm_new_connect(
        bt.as_ptr(),
        sock.as_ptr(),
        sess.as_ptr(),
        host.as_ptr(),
        ptr::null(),
    );
    if h.is_null() {
        return false;
    }
    unsafe {
        let mut buf = [CStateChange::default(); 64];
        let deadline = Instant::now() + Duration::from_secs(10);
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
            std::thread::sleep(Duration::from_millis(200));
        }
        muxterm_free(h);
        ok
    }
}

/// 需要真实 loopback sshd（CI setup 或本地）。
#[test]
fn ssh_wizard_attach_remote_session() {
    if !sshd_available() {
        eprintln!("skip: 无 loopback sshd（未设置 MUXTERM_TEST_SSH_*）");
        return;
    }
    let env = SshTestEnv::setup("wizard-ssh").expect("SshTestEnv::setup 失败");
    let session_name = format!("wiz-ssh-{}", std::process::id());
    // 在远端创建独立 tmux session（独立 socket + 命名 session）
    let (ok, _, err) = env.remote_tmux(&format!("new-session -d -s {} -x 100 -y 30", session_name));
    assert!(ok, "远端 tmux new-session 失败: {err}");
    std::thread::sleep(Duration::from_millis(500));

    // 用 tmux-ssh 后端 attach（模拟向导 ssh+attach）
    let ok = ssh_connect_attach(
        &env.alias,
        &env.ssh_config_path.to_string_lossy(),
        &env.remote_tmux_socket,
        &session_name,
    );
    assert!(ok, "SSH 向导 attach 应成功建连并读到远端 tab");
}
