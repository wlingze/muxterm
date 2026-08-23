//! W6 §11.1/§11.2：Project 与 Existing Connection 统一身份契约。
//!
//! 本地 + loopback SSH 都要跑 parity：
//! - 保存后重载的 Project 与 discovery 出的 Existing 产生相同 identity key、
//!   attach spec 身份字段和 WorkspaceId；存在同 identity Project 元数据时
//!   ResolvedTarget 相同；
//! - Catalog::resolve_target 是唯一 TargetConfig→ResolvedTarget resolver；
//! - AttachOnly 无匹配不创建；CreateIfMissing 只在显式 local named session
//!   已运行时创建，SSH 两意图零创建命令；
//! - identity key 由 transport target/runtime/session/socket/workspace_id
//!   构成，name/path 变更不改变身份。

mod support;

use std::time::{Duration, Instant};

use muxterm::core::catalog::{Catalog, ResolveIntent};
use muxterm::core::quickconnect::model::{TargetConfig, TargetRuntime, TargetTransport};
use muxterm::core::workspace::id::WorkspaceId;
use support::herdr_test_support::{herdr_available, IsolatedHerdr};
use support::sshd_test_support::{loopback_sshd_available, LoopbackSshd};

const TIMEOUT: Duration = Duration::from_secs(30);

fn wait_until(mut predicate: impl FnMut() -> bool, label: &str) -> bool {
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    eprintln!("等待 {label} 超时");
    false
}

/// 本地 herdr：Project 保存 → 重载，与 discovery Existing 同一身份。
#[test]
fn local_project_reload_matches_existing_identity() {
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }
    let herdr = IsolatedHerdr::start("w6-local");
    let (workspace_id, _tab, _pane) = herdr.create_workspace("/tmp", "w6-project-identity");

    // 模拟“从 discovery Existing 构造的 TargetConfig”。
    let existing = TargetConfig {
        name: "w6-project-identity".into(),
        runtime: TargetRuntime::Herdr,
        transport: TargetTransport::Local,
        path: "/tmp".into(),
        socket: Some(herdr.socket_path().to_string_lossy().to_string()),
        session: Some(herdr.name().to_string()),
        workspace_id: Some(workspace_id.clone()),
    };

    // 模拟“保存后重载的 Project”（TOML round-trip：字段应无损）。
    let mut store = muxterm::core::quickconnect::store::QuickConnectStore::new(None);
    store.upsert_project(&existing);
    let text = store.encode();
    let mut reloaded = muxterm::core::quickconnect::store::QuickConnectStore::new(None);
    reloaded.decode(&text);
    assert_eq!(reloaded.projects.len(), 1);
    let project = &reloaded.projects[0];

    // identity key / spec 身份字段 / WorkspaceId 一致。
    assert_eq!(existing.identity_key(), project.identity_key());
    let spec_a = muxterm::core::catalog::config_to_spec(&existing);
    let spec_b = muxterm::core::catalog::config_to_spec(project);
    assert_eq!(spec_a.id(), spec_b.id());

    // Catalog::resolve_target：AttachOnly 命中（不创建）。
    std::env::set_var(
        "HERDR_SOCKET_PATH",
        herdr.socket_path().to_string_lossy().to_string(),
    );
    let mut catalog = muxterm::core::catalog::Catalog::with_builtins();
    let resolved = catalog
        .resolve_target(&existing, ResolveIntent::AttachOnly)
        .expect("AttachOnly 必须命中已存在 workspace");
    assert_eq!(resolved.workspace_id(), spec_a.id());
    assert_eq!(
        resolved.canonical.workspace_id.as_deref(),
        Some(workspace_id.as_str())
    );
    assert_eq!(resolved.canonical.name, "w6-project-identity");
    let _ = std::env::remove_var("HERDR_SOCKET_PATH");
}

/// 本地：AttachOnly 无匹配不创建；CreateIfMissing 只创建显式运行中的
/// named session。
#[test]
fn local_attach_only_never_creates_and_create_requires_running_session() {
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }
    // 未启动任何 server：AttachOnly 必须失败（零创建命令）。
    let missing = TargetConfig {
        name: "w6-ghost".into(),
        runtime: TargetRuntime::Herdr,
        transport: TargetTransport::Local,
        path: "/tmp".into(),
        socket: None,
        session: Some("w6-never-started".into()),
        workspace_id: None,
    };
    let mut catalog = muxterm::core::catalog::Catalog::with_builtins();
    let err = catalog
        .resolve_target(&missing, ResolveIntent::AttachOnly)
        .expect_err("AttachOnly 无匹配必须失败");
    assert!(
        err.to_string().contains("无匹配"),
        "AttachOnly 错误应说明无匹配: {err}"
    );
    // CreateIfMissing 无 socket → choice-required，不能偷偷换 default。
    let err = catalog
        .resolve_target(&missing, ResolveIntent::CreateIfMissing)
        .expect_err("CreateIfMissing 无显式 socket 必须失败");
    assert!(
        err.to_string().contains("socket"),
        "CreateIfMissing 需要显式 socket: {err}"
    );
}

/// SSH：两意图都零创建命令；AttachOnly 无匹配即失败。
#[test]
fn ssh_herdr_attach_only_never_creates() {
    if !herdr_available() || !loopback_sshd_available() {
        eprintln!("skip: 无 herdr 或 sshd");
        return;
    }
    let sshd = LoopbackSshd::start("w6-ssh").expect("启动 loopback sshd");
    std::env::set_var("MUXTERM_SSH_CONFIG_PATH", &sshd.config_path);

    let ssh_target = TargetConfig {
        name: "w6-remote-project".into(),
        runtime: TargetRuntime::Herdr,
        transport: TargetTransport::Ssh {
            name: sshd.alias.clone(),
        },
        path: "/srv/w6".into(),
        socket: Some("/tmp/remote-herdr.sock".into()),
        session: Some("default".into()),
        workspace_id: Some("w1".into()),
    };
    let mut catalog = muxterm::core::catalog::Catalog::with_builtins();
    let err = catalog
        .resolve_target(&ssh_target, ResolveIntent::AttachOnly)
        .expect_err("SSH 未命中必须失败（零创建命令）");
    assert!(
        err.to_string().contains("无匹配") || err.to_string().contains("AttachOnly"),
        "SSH AttachOnly 错误语义: {err}"
    );
    let err = catalog
        .resolve_target(&ssh_target, ResolveIntent::CreateIfMissing)
        .expect_err("SSH CreateIfMissing 必须禁止创建命令");
    assert!(
        err.to_string().contains("SSH"),
        "SSH CreateIfMissing 应报禁止创建: {err}"
    );
    let _ = std::env::remove_var("MUXTERM_SSH_CONFIG_PATH");
}

/// identity key 只含身份字段：同 name/path 不同 session/socket/workspace_id
/// 是不同身份；name/path 变更不改变身份。
#[test]
fn identity_key_is_identity_only() {
    let base = TargetConfig {
        name: "proj".into(),
        runtime: TargetRuntime::Herdr,
        transport: TargetTransport::Ssh {
            name: "ryzen".into(),
        },
        path: "/srv/proj".into(),
        socket: Some("/remote/herdr.sock".into()),
        session: Some("dev".into()),
        workspace_id: Some("w3".into()),
    };
    let mut renamed = base.clone();
    renamed.name = "renamed".into();
    renamed.path = "/elsewhere".into();
    assert_eq!(base.identity_key(), renamed.identity_key());

    let mut other_socket = base.clone();
    other_socket.socket = Some("/other.sock".into());
    assert_ne!(base.identity_key(), other_socket.identity_key());
    let mut other_ws = base.clone();
    other_ws.workspace_id = Some("w9".into());
    assert_ne!(base.identity_key(), other_ws.identity_key());
    let mut other_session = base.clone();
    other_session.session = Some("prod".into());
    assert_ne!(base.identity_key(), other_session.identity_key());

    // WorkspaceId 用 spec 五段；SSH 含 alias。
    let spec = muxterm::core::catalog::config_to_spec(&base);
    let wid: WorkspaceId = spec.id();
    assert_eq!(wid.transport, "ssh");
    assert_eq!(wid.alias.as_deref(), Some("ryzen"));
    assert_eq!(wid.session, "dev");
    assert_eq!(wid.runtime, "herdr");
    assert_eq!(wid.path, "w3", "Herdr 的 path 段是 workspace_id");
}
