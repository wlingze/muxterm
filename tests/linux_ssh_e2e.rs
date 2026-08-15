//! loopback SSH + 远端隔离 tmux e2e（LINUX-PLAN §5.4 S12）。
//!
//! 需要 sshd：先 `eval "$(./scripts/ci/setup-sshd.sh)"`。无 sshd 时 `#[ignore]`，
//! 默认门禁不跑，不算失败。远端 tmux 一律 `-L <remote_socket>`。

#![cfg(feature = "gtk")]

mod support;

use std::time::{Duration, Instant};

use support::sshd_test_support::{sshd_available, SshTestEnv};

use muxterm::core::replica::ReplicaStore;
use muxterm::platform::linux::ffi_bridge::CoreBridge;

/// S12：远端隔离 tmux echo 到达 replica；status summary 是 ssh。
#[test]
#[ignore = "需要 loopback sshd（scripts/ci/setup-sshd.sh）"]
fn loopback_ssh_isolated_tmux_echo() {
    if !sshd_available() {
        eprintln!("skip: 无 sshd（先 eval \"$(./scripts/ci/setup-sshd.sh)\"）");
        return;
    }
    let env = SshTestEnv::setup("s12").expect("SSH 测试环境");
    // 远端隔离 tmux：new-session -d -s s（带 -L）。
    let (ok, _, err) = env.remote_tmux("new-session -d -s s -x 80 -y 24");
    assert!(ok, "远端 tmux new-session 失败: {err}");

    // Muxterm SSH attach（CoreBridge 等价路径；ssh config 走 MUXTERM_SSH_CONFIG_PATH）。
    std::env::set_var("MUXTERM_SSH_CONFIG_PATH", &env.ssh_config_path);
    let bridge = CoreBridge::connect(
        "tmux-ssh",
        Some(&env.remote_tmux_socket),
        Some("s"),
        Some(&env.alias),
        Some("/tmp"),
    )
    .expect("SSH attach 应成功");
    let _ = bridge.poll_events();

    // 远端 echo token。
    let (ok, _, err) = env.remote_tmux("send-keys -t s 'echo MUXTERM_SSH_TOKEN' Enter");
    assert!(ok, "远端 send-keys 失败: {err}");

    // 轮询：replica 与远端 capture-pane 都含 token。
    let mut store = ReplicaStore::new(10_000);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut ok = false;
    while Instant::now() < deadline {
        for ev in bridge.poll_events() {
            if ev.type_ == muxterm::core::protocol::ffi::types::STATE_PANE_OUTPUT {
                store.feed("ssh@local", ev.pane_id, &ev.data, 80, 24);
            }
        }
        let replica = store.last_n_lines("ssh@local", 0, 5).join("\n");
        let (_, capture, _) = env.remote_tmux("capture-pane -p -t s");
        if replica.contains("MUXTERM_SSH_TOKEN") && capture.contains("MUXTERM_SSH_TOKEN") {
            ok = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ok, "10s 内 SSH echo 应到达 replica 与远端 capture-pane");

    // 清理：远端 tmux -L kill-server（禁止不带 -L）。
    let (ok, _, err) = env.remote_tmux("kill-server");
    assert!(ok, "远端 kill-server 失败: {err}");
    drop(bridge);
}
