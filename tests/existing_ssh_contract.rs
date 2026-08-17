//! W20d/e：已有的连接发现契约。
//!
//! - W20d：IsolatedHerdr 的 workspace 出现在本地 discover，且不含用户默认 w2。
//! - W20e：LoopbackSshd 上远端 tmux session 与 Herdr workspace 都能列出。
//!
//! 无 sshd / 无 herdr 才 eprintln skip；禁止 #[ignore]。

mod support;

use std::time::Duration;

use muxterm::core::discovery::existing::{
    discover_local_herdr, discover_ssh_herdr, discover_ssh_tmux,
};
use muxterm::core::quickconnect::model::TargetRuntime;
use support::herdr_test_support::{herdr_available, IsolatedHerdr};
use support::sshd_test_support::{loopback_sshd_available, LoopbackSshd};
use support::tmux_test_support::{create_session, kill_server, tmux_available, unique_socket};

/// W20d：本地 Herdr discover 必须看到测试 workspace，且不得出现用户默认 w2。
#[test]
fn discover_local_herdr_sees_isolated_workspace_only() {
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }
    let herdr = IsolatedHerdr::start("disc");
    let (ws, _tab, _pane) = herdr.create_workspace("/tmp", "mux-w20d");

    // 注入测试 socket；config_dir 指向空临时目录，避免扫到用户默认。
    std::env::set_var(
        "HERDR_SOCKET_PATH",
        herdr.socket_path().to_string_lossy().to_string(),
    );
    let tmp = std::env::temp_dir().join(format!(
        "muxterm-test-herdr-disc-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let entries = discover_local_herdr(Some(&tmp));
    std::env::remove_var("HERDR_SOCKET_PATH");
    let _ = std::fs::remove_dir_all(&tmp);

    assert!(
        entries
            .iter()
            .any(|e| e.herdr_workspace_id.as_deref() == Some(ws.as_str())),
        "本地 discover 必须看到刚 create 的 workspace {ws}: {entries:?}"
    );
    assert!(
        entries
            .iter()
            .all(|e| e.herdr_workspace_id.as_deref() != Some("w2")),
        "不得出现用户默认 w2: {entries:?}"
    );
    assert!(
        entries.iter().all(|e| e.runtime == TargetRuntime::Herdr),
        "本地 discover 只应有 Herdr 行"
    );
}

/// W20e：LoopbackSshd 上远端 tmux + Herdr 都能列出。
#[test]
fn ssh_discover_lists_remote_tmux_and_herdr() {
    if !loopback_sshd_available() {
        eprintln!("skip: 无 sshd 二进制，无法自启 loopback sshd");
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux 二进制");
        return;
    }
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }
    let sshd = LoopbackSshd::start("existing-ssh").expect("启动 loopback sshd 失败");
    sshd.apply_ssh_config_env();

    // 远端（loopback 同机）隔离 tmux session。
    let socket = unique_socket("existing-ssh-tmux");
    create_session(&socket, "existing-ssh-sess", 80, 24);
    let tmux_guard = TmuxGuard {
        socket: socket.clone(),
    };

    // 远端（loopback 同机）隔离 Herdr named session。
    let herdr = IsolatedHerdr::start("ssh-disc");
    let (ws, _tab, _pane) = herdr.create_workspace("/tmp", "mux-w20e");

    let timeout = Duration::from_secs(5);
    let tmux_entries = discover_ssh_tmux(
        &sshd.alias,
        Some(&sshd.config_path.to_string_lossy()),
        Some(&socket),
        timeout,
    );
    assert!(
        tmux_entries
            .iter()
            .any(|e| e.tmux_session.as_deref() == Some("existing-ssh-sess")),
        "远端 tmux session 必须列出: {tmux_entries:?}"
    );

    let herdr_entries = discover_ssh_herdr(
        &sshd.alias,
        Some(&sshd.config_path.to_string_lossy()),
        timeout,
    );
    assert!(
        herdr_entries
            .iter()
            .any(|e| e.herdr_workspace_id.as_deref() == Some(ws.as_str())),
        "远端 Herdr workspace 必须列出: {herdr_entries:?}"
    );
    assert!(
        herdr_entries
            .iter()
            .all(|e| e.runtime == TargetRuntime::Herdr),
        "SSH discover 的 Herdr 行 runtime 必须是 Herdr"
    );
    drop(tmux_guard);
}

/// W20h SSH：远端 herdr.sock 转发到本机后，HerdrSession 能 attach。
#[test]
fn ssh_herdr_forward_attach_contract() {
    if !loopback_sshd_available() {
        eprintln!("skip: 无 sshd 二进制，无法自启 loopback sshd");
        return;
    }
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }
    let sshd = LoopbackSshd::start("herdr-fwd").expect("启动 loopback sshd 失败");
    sshd.apply_ssh_config_env();
    let herdr = IsolatedHerdr::start("fwd-attach");
    let (ws, _tab, _pane) = herdr.create_workspace("/tmp", "mux-fwd");

    let (local_socket, mut forward) =
        muxterm::core::runtime::herdr::forward::start_herdr_ssh_forward(
            &sshd.alias,
            &herdr.socket_path().to_string_lossy(),
            Some(&sshd.config_path.to_string_lossy()),
        )
        .expect("ssh socket 转发应就绪");

    let session = muxterm::core::runtime::HerdrSession::new(herdr.name(), &local_socket);
    session.ping().expect("转发后的 HerdrSession 应能 ping");
    let snap = session.snapshot().expect("转发后的 snapshot 应成功");
    assert!(
        snap.workspaces.iter().any(|w| w.workspace_id == ws),
        "转发后必须看到远端 workspace {ws}"
    );

    let _ = forward.kill();
    let _ = forward.wait();
}

/// 隔离 tmux 清理（只杀自己的 -L server）。
struct TmuxGuard {
    socket: String,
}

impl Drop for TmuxGuard {
    fn drop(&mut self) {
        kill_server(&self.socket);
    }
}
