//! W18 SSH attach 契约：loopback sshd + 远端隔离 tmux **先**涂 token，再 SSH attach。
//!
//! 测试自己拉起 sshd（随机端口）。无 sshd 二进制时 skip。禁止 MockRuntime。
//! 远端 tmux 一律 `-L muxterm-test-*`。

mod support;

use std::time::Instant;

use muxterm::core::runtime::TmuxRuntime;
use muxterm::core::workspace::id::WorkspaceId;
use muxterm::core::workspace::workspace::Workspace;
use support::ssh_tmux_contract::{build_remote_one_pane, ssh_tmux_available, SSH_TIMEOUT};
use support::tmux_test_support::tmux_available;

/// 远端 `/bin/cat` 先有画面，SSH attach 后 PaneBuf 必须能搜到 token（与本地 feature 契约一致）。
#[test]
fn ssh_attach_preexist_token_reaches_workspace() {
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    if !ssh_tmux_available() {
        eprintln!("skip: 无 sshd 二进制，无法自启 loopback sshd");
        return;
    }
    let fx = build_remote_one_pane("feat-ssh");
    fx.apply_ssh_config_env();

    let runtime = TmuxRuntime::new_ssh_attach(&fx.sshd.alias, Some(&fx.socket), &fx.session);
    let mut ws = Workspace::new(
        WorkspaceId::new("ssh", Some(&fx.sshd.alias), &fx.session, "tmux", ""),
        fx.session.clone(),
        Box::new(runtime),
    );
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("tokio");
    rt.block_on(ws.connect())
        .expect("SSH attach 失败（隔离远端 -L，不是用户默认 tmux）");

    let deadline = Instant::now() + SSH_TIMEOUT;
    while Instant::now() < deadline {
        let _ = ws.refresh();
        if !ws.search_workspace(&fx.token).is_empty() {
            let _ = rt.block_on(ws.shutdown());
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let hits = ws.search_workspace(&fx.token).len();
    let _ = rt.block_on(ws.shutdown());
    panic!(
        "SSH attach 后 PaneBuf 必须含播种 token {}。禁止另建 MockRuntime 喂字节冒充。hits={hits}",
        fx.token
    );
}
