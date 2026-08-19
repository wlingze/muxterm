//! TUI 连接向导 SSH FFI 回归：验证 `muxterm_new_connect("tmux-ssh", ...)` 重连路径。
//!
//! 向导选择 ssh + attach 时，app.rs 用 CoreBridge::new_connect 以 tmux-ssh 后端
//! 建连。这里用真实 loopback sshd + 显式 ssh config（tests/support）验证这条路径
//! 能成功建连并读到远端 tmux 的 tab。

#![cfg(feature = "ffi")]
#![cfg(unix)]

mod support;
use support::ssh_tmux_contract::{build_remote_one_pane, ssh_tmux_available};

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

/// 使用测试进程自建的 loopback sshd 与远端隔离 tmux socket。
#[test]
fn ssh_wizard_attach_remote_session() {
    if !ssh_tmux_available() {
        eprintln!("skip: 无法启动 loopback sshd 或缺少 tmux");
        return;
    }
    let fixture = build_remote_one_pane("wizard-ssh");

    // 用 tmux-ssh 后端 attach（模拟向导 ssh+attach）
    let ok = ssh_connect_attach(
        &fixture.sshd.alias,
        &fixture.sshd.config_path.to_string_lossy(),
        &fixture.socket,
        &fixture.session,
    );
    assert!(ok, "SSH 向导 attach 应成功建连并读到远端 tab");
}
