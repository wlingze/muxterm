//! W14 SSH attach 契约：远端隔离 tmux **先**涂 token，再 SSH attach。
//!
//! 无 sshd 时 `#[ignore]`，默认门禁不跑。跑 `--ignored` 时必须有
//! `eval "$(./scripts/ci/setup-sshd.sh)"`，禁止用 MockRuntime 冒充。

mod support;

use std::time::{Duration, Instant};

use muxterm::core::runtime::TmuxRuntime;
use muxterm::core::workspace::id::WorkspaceId;
use muxterm::core::workspace::workspace::Workspace;
use support::sshd_test_support::{sshd_available, SshTestEnv};

const SSH_TIMEOUT: Duration = Duration::from_secs(12);

/// 远端 `/bin/cat` 先有画面，SSH attach 后 PaneBuf 必须能搜到 token。
#[test]
#[ignore = "需要 loopback sshd（scripts/ci/setup-sshd.sh）"]
fn ssh_attach_preexist_token_reaches_workspace() {
    assert!(
        sshd_available(),
        "跑 --ignored 时 sshd 必须在听；先 eval \"$(./scripts/ci/setup-sshd.sh)\""
    );
    let env = SshTestEnv::setup("feat-ssh").expect("SSH 测试环境");
    std::env::set_var("MUXTERM_SSH_CONFIG_PATH", &env.ssh_config_path);

    let session = "featssh";
    let token = format!(
        "SSH_LIVE_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(1)
    );
    let (ok, _, err) = env.remote_tmux(&format!(
        "new-session -d -s {session} -x 80 -y 24 -- /bin/cat"
    ));
    assert!(ok, "远端 new-session /bin/cat 失败: {err}");
    let (ok, _, err) = env.remote_tmux(&format!("send-keys -t {session} -l {token}"));
    assert!(ok, "远端 send-keys -l 失败: {err}");

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut painted = false;
    while Instant::now() < deadline {
        let (_, cap, _) = env.remote_tmux(&format!("capture-pane -p -t {session}"));
        if cap.contains(&token) {
            painted = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(painted, "attach 前远端 capture-pane 必须已有 {token}");

    let runtime = TmuxRuntime::new_ssh_attach(&env.alias, Some(&env.remote_tmux_socket), session);
    let mut ws = Workspace::new(
        WorkspaceId::new("ssh", Some(&env.alias), session, "tmux", ""),
        session.into(),
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
        if !ws.search_workspace(&token).is_empty() {
            let _ = env.remote_tmux("kill-server");
            let _ = rt.block_on(ws.shutdown());
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = env.remote_tmux("kill-server");
    panic!(
        "SSH attach 后 PaneBuf 必须含播种 token {token}。禁止另建 MockRuntime 喂字节冒充。hits={}",
        ws.search_workspace(&token).len()
    );
}
