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

/// macOS ProjectConnectFlow：discovery 创建 session 后再 FFI attach。
///
/// alias 与远端 `-L` 必须分开；禁止把 Host 名塞进 `tmux -L <alias>`。
#[cfg(feature = "ffi")]
#[test]
#[ignore = "需要 loopback sshd（scripts/ci/setup-sshd.sh）"]
fn ssh_create_then_ffi_attach_reaches_workspace() {
    use std::ffi::{CStr, CString};
    use std::ptr;

    use muxterm::core::protocol::ffi::api::{
        muxterm_create_tmux_session_json, muxterm_free, muxterm_free_string, muxterm_new_connect,
        muxterm_poll_events, muxterm_search_all,
    };
    use muxterm::core::protocol::ffi::types::CStateChange;

    assert!(
        sshd_available(),
        "跑 --ignored 时 sshd 必须在听；先 eval \"$(./scripts/ci/setup-sshd.sh)\""
    );
    let env = SshTestEnv::setup("create-ssh").expect("SSH 测试环境");
    std::env::set_var("MUXTERM_SSH_CONFIG_PATH", &env.ssh_config_path);

    let session = "created-ssh";
    let directory = std::env::temp_dir();
    let directory = directory.to_str().unwrap_or("/tmp");
    let token = format!(
        "SSH_CREATED_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(1)
    );

    let transport = CString::new("ssh").unwrap();
    let target = CString::new(env.alias.as_str()).unwrap();
    let sock = CString::new(env.remote_tmux_socket.as_str()).unwrap();
    let cfg = CString::new(env.ssh_config_path.to_string_lossy().as_ref()).unwrap();
    let sess = CString::new(session).unwrap();
    let dir = CString::new(directory).unwrap();
    let created = muxterm_create_tmux_session_json(
        transport.as_ptr(),
        target.as_ptr(),
        sock.as_ptr(),
        cfg.as_ptr(),
        sess.as_ptr(),
        dir.as_ptr(),
        10_000,
    );
    assert!(!created.is_null(), "create_tmux_session_json 应返回 JSON");
    let created_text = unsafe {
        let text = CStr::from_ptr(created).to_string_lossy().into_owned();
        muxterm_free_string(created);
        text
    };
    assert!(
        created_text.contains("\"ok\":true") || created_text.contains("\"ok\": true"),
        "discovery 创建远端 session 必须成功: {created_text}"
    );

    let (ok, _, err) = env.remote_tmux(&format!("respawn-pane -k -t {session} -- /bin/cat"));
    assert!(ok, "远端 respawn /bin/cat 失败: {err}");
    let (ok, _, err) = env.remote_tmux(&format!("send-keys -t {session} -l {token}"));
    assert!(ok, "远端 send-keys 失败: {err}");
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

    let h = muxterm_new_connect(
        transport.as_ptr(),
        sock.as_ptr(),
        sess.as_ptr(),
        target.as_ptr(),
        ptr::null(),
    );
    assert!(
        !h.is_null(),
        "muxterm_new_connect(ssh, socket=隔离 -L, alias) 必须 attach 成功（不能把 alias 当成 -L）"
    );

    let q = CString::new(token.as_str()).unwrap();
    let deadline = Instant::now() + SSH_TIMEOUT;
    let mut found = false;
    unsafe {
        let mut buf = [CStateChange::default(); 64];
        while Instant::now() < deadline {
            let _ = muxterm_poll_events(h, buf.as_mut_ptr(), 64);
            let raw = muxterm_search_all(h, q.as_ptr());
            if !raw.is_null() {
                let json = CStr::from_ptr(raw).to_string_lossy().into_owned();
                muxterm_free_string(raw);
                if json.contains(&token) {
                    found = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        muxterm_free(h);
    }
    let _ = env.remote_tmux("kill-server");
    assert!(found, "create 后再 FFI attach，PaneBuf 必须含 {token}");
}
