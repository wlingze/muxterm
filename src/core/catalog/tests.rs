//! Catalog 单测。内置 Driver 未登记时 `with_builtins_*` 为红；mock 路径应绿。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::{Catalog, Reach};
use crate::core::catalog::connect::Connect;
use crate::core::catalog::driver::{RuntimeDriver, SessionCandidate};
use crate::core::catalog::transport::{TargetInfo, Transport};
use crate::core::model::backend::mock::MockRuntime;
use crate::core::model::backend::{Runtime, RuntimeCapability};
use crate::core::workspace::spec::WorkspaceSpec;

struct MockDriver {
    id: &'static str,
    name: &'static str,
    accepted: &'static [&'static str],
    support: &'static [RuntimeCapability],
    listed: Vec<SessionCandidate>,
    list_err: bool,
    opened: Arc<AtomicUsize>,
}

impl RuntimeDriver for MockDriver {
    fn id(&self) -> &'static str {
        self.id
    }
    fn name(&self) -> &'static str {
        self.name
    }
    fn support(&self) -> &'static [RuntimeCapability] {
        self.support
    }
    fn accepted_transports(&self) -> &'static [&'static str] {
        self.accepted
    }
    fn list(
        &self,
        connect: &Connect,
        _namespace: Option<&str>,
    ) -> anyhow::Result<Vec<SessionCandidate>> {
        if self.list_err {
            anyhow::bail!("mock list failed");
        }
        Ok(self
            .listed
            .iter()
            .cloned()
            .map(|mut row| {
                row.transport_id = connect.transport_id().to_string();
                row.target = connect.target().to_string();
                row
            })
            .collect())
    }
    fn open(
        &self,
        _connect: Arc<Connect>,
        spec: &WorkspaceSpec,
    ) -> anyhow::Result<Box<dyn Runtime>> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        let mut rt = MockRuntime::with_single_pane();
        rt.workspace_runtime = spec.runtime.clone();
        Ok(Box::new(rt))
    }
}

struct MockTransport {
    id: &'static str,
    name: &'static str,
    connects: Arc<AtomicUsize>,
    fail: bool,
    targets: Vec<TargetInfo>,
}

impl Transport for MockTransport {
    fn id(&self) -> &'static str {
        self.id
    }
    fn name(&self) -> &'static str {
        self.name
    }
    fn list_targets(&self) -> anyhow::Result<Vec<TargetInfo>> {
        Ok(self.targets.clone())
    }
    fn connect(&self, target: &str) -> anyhow::Result<Arc<Connect>> {
        if self.fail {
            anyhow::bail!("mock connect failed");
        }
        self.connects.fetch_add(1, Ordering::SeqCst);
        Ok(Connect::new(self.id, target))
    }
}

fn mock_spec(runtime: &str, transport: &str, alias: Option<&str>, session: &str) -> WorkspaceSpec {
    WorkspaceSpec {
        transport: transport.into(),
        alias: alias.map(str::to_string),
        session: session.into(),
        runtime: runtime.into(),
        path: String::new(),
        socket: None,
        create: false,
        scrollback_lines: 10_000,
    }
}

#[test]
fn list_order_follows_registration() {
    let mut cat = Catalog::new();
    cat.register_runtime(Box::new(MockDriver {
        id: "shell",
        name: "Shell",
        accepted: &["local"],
        support: &[],
        listed: vec![],
        list_err: false,
        opened: Arc::new(AtomicUsize::new(0)),
    }));
    cat.register_runtime(Box::new(MockDriver {
        id: "tmux",
        name: "tmux",
        accepted: &["local"],
        support: &[],
        listed: vec![],
        list_err: false,
        opened: Arc::new(AtomicUsize::new(0)),
    }));
    let ids: Vec<_> = cat.runtime_list().into_iter().map(|r| r.id).collect();
    assert_eq!(
        ids,
        ["shell", "tmux"],
        "不要按名字重排，登记顺序就是列表顺序"
    );
}

#[test]
fn with_builtins_runtime_list_is_tmux_herdr_shell() {
    let cat = Catalog::with_builtins();
    let ids: Vec<_> = cat.runtime_list().into_iter().map(|r| r.id).collect();
    assert_eq!(ids, ["tmux", "herdr", "shell"]);
    assert!(!ids.iter().any(|id| id == "daemon"));
}

#[test]
fn with_builtins_transport_list_is_local_ssh() {
    let cat = Catalog::with_builtins();
    let ids: Vec<_> = cat.transport_list().into_iter().map(|t| t.id).collect();
    assert_eq!(ids, ["local", "ssh"]);
}

#[test]
fn with_builtins_herdr_reports_worktree_caps() {
    let cat = Catalog::with_builtins();
    let herdr = cat
        .runtime_list()
        .into_iter()
        .find(|r| r.id == "herdr")
        .expect("with_builtins 必须登记 herdr");
    assert!(
        herdr.support.contains(&RuntimeCapability::WorktreeList),
        "Herdr 卡必须带 WorktreeList: {:?}",
        herdr.support
    );
    assert!(herdr.support.contains(&RuntimeCapability::WorktreeCreate));
}

#[test]
fn with_builtins_shell_accepts_local_and_ssh() {
    let cat = Catalog::with_builtins();
    let shell = cat
        .runtime_list()
        .into_iter()
        .find(|runtime| runtime.id == "shell")
        .expect("with_builtins 必须登记 shell");
    assert_eq!(shell.accepted_transports, ["local", "ssh"]);
}

#[test]
fn discover_sessions_fans_out_and_skips_driver_error() {
    let mut cat = Catalog::new();
    cat.register_transport(Box::new(MockTransport {
        id: "local",
        name: "Local",
        connects: Arc::new(AtomicUsize::new(0)),
        fail: false,
        targets: vec![TargetInfo::new("", "local")],
    }));
    cat.register_runtime(Box::new(MockDriver {
        id: "tmux",
        name: "tmux",
        accepted: &["local"],
        support: &[RuntimeCapability::Discover],
        listed: vec![SessionCandidate {
            runtime_id: "tmux".into(),
            transport_id: String::new(),
            target: String::new(),
            namespace: None,
            name: "mux".into(),
            extra: String::new(),
        }],
        list_err: false,
        opened: Arc::new(AtomicUsize::new(0)),
    }));
    cat.register_runtime(Box::new(MockDriver {
        id: "herdr",
        name: "Herdr",
        accepted: &["local"],
        support: &[RuntimeCapability::Discover],
        listed: vec![],
        list_err: true,
        opened: Arc::new(AtomicUsize::new(0)),
    }));
    let rows = cat
        .discover_sessions("local", "")
        .expect("扇出不应因单个 Driver 失败");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "mux");
    assert_eq!(rows[0].runtime_id, "tmux");
}

#[test]
fn connect_reuses_arc_for_same_target() {
    let mut cat = Catalog::new();
    let n = Arc::new(AtomicUsize::new(0));
    cat.register_transport(Box::new(MockTransport {
        id: "ssh",
        name: "SSH",
        connects: Arc::clone(&n),
        fail: false,
        targets: vec![TargetInfo::new("ryzen", "ryzen")],
    }));
    let a = cat.connect("ssh", "ryzen").unwrap();
    let b = cat.connect("ssh", "ryzen").unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    assert_eq!(n.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn two_opens_same_target_share_one_connect() {
    let mut cat = Catalog::new();
    let n = Arc::new(AtomicUsize::new(0));
    cat.register_transport(Box::new(MockTransport {
        id: "ssh",
        name: "SSH",
        connects: Arc::clone(&n),
        fail: false,
        targets: vec![],
    }));
    cat.register_runtime(Box::new(MockDriver {
        id: "tmux",
        name: "tmux",
        accepted: &["local", "ssh"],
        support: &[],
        listed: vec![],
        list_err: false,
        opened: Arc::new(AtomicUsize::new(0)),
    }));
    cat.open(&mock_spec("tmux", "ssh", Some("ryzen"), "a"))
        .await
        .unwrap();
    cat.open(&mock_spec("tmux", "ssh", Some("ryzen"), "b"))
        .await
        .unwrap();
    assert_eq!(
        n.load(Ordering::SeqCst),
        1,
        "同一 SSH target 只 connect 一次"
    );
    assert_eq!(cat.pool().len(), 2);
}

#[tokio::test]
async fn open_rejects_unknown_runtime() {
    let mut cat = Catalog::new();
    cat.register_transport(Box::new(MockTransport {
        id: "local",
        name: "Local",
        connects: Arc::new(AtomicUsize::new(0)),
        fail: false,
        targets: vec![],
    }));
    let err = cat
        .open(&mock_spec("unknown", "local", None, "x"))
        .await
        .map(|_| ())
        .expect_err("未知 runtime 必须 Err");
    assert!(
        err.to_string().contains("unknown runtime"),
        "禁止悄悄变成 shell: {err}"
    );
}

#[tokio::test]
async fn open_uses_driver_not_build_runtime() {
    let mut cat = Catalog::new();
    let opened = Arc::new(AtomicUsize::new(0));
    cat.register_transport(Box::new(MockTransport {
        id: "local",
        name: "Local",
        connects: Arc::new(AtomicUsize::new(0)),
        fail: false,
        targets: vec![],
    }));
    cat.register_runtime(Box::new(MockDriver {
        id: "mockrt",
        name: "mock",
        accepted: &["local"],
        support: &[],
        listed: vec![],
        list_err: false,
        opened: Arc::clone(&opened),
    }));
    let ws = cat
        .open(&mock_spec("mockrt", "local", None, "demo"))
        .await
        .unwrap();
    assert_eq!(ws.runtime().workspace_runtime(), "mockrt");
    assert_eq!(opened.load(Ordering::SeqCst), 1);
}

#[test]
fn refresh_inventory_marks_unreachable_without_opening() {
    let mut cat = Catalog::new();
    cat.register_transport(Box::new(MockTransport {
        id: "ssh",
        name: "SSH",
        connects: Arc::new(AtomicUsize::new(0)),
        fail: true,
        targets: vec![TargetInfo::new("dead", "dead")],
    }));
    cat.register_runtime(Box::new(MockDriver {
        id: "tmux",
        name: "tmux",
        accepted: &["ssh"],
        support: &[RuntimeCapability::Discover],
        listed: vec![],
        list_err: false,
        opened: Arc::new(AtomicUsize::new(0)),
    }));
    cat.refresh_inventory().expect("探活失败不应 panic");
    assert_eq!(
        cat.inventory_snapshot().reach("ssh", "dead"),
        Some(Reach::Err)
    );
    assert_eq!(cat.pool().len(), 0, "探活不得打开 Workspace");
}

#[test]
fn pool_must_not_special_case_herdr_runtime_string() {
    let src = include_str!("../workspace/pool.rs");
    assert!(
        !src.contains("if spec.runtime == \"herdr\""),
        "open_spec 禁止按 runtime 字符串走 Herdr 旁路；共享连接走 Catalog.connects"
    );
}

#[test]
fn pool_must_not_hold_herdr_sessions_sidecar() {
    let src = include_str!("../workspace/pool.rs");
    assert!(
        !src.contains("herdr_sessions"),
        "WorkspacePool 不再持有 herdr_sessions；Connect 表在 Catalog"
    );
}

/// C7：测试隔离远端 tmux 必须能通过 env 传给 TmuxDriver.list。
#[test]
fn tmux_driver_list_honors_test_remote_socket_env() {
    let src = include_str!("builtin/tmux.rs");
    assert!(
        src.contains("MUXTERM_TEST_REMOTE_TMUX_SOCKET"),
        "TmuxDriver::list SSH 分支必须读 MUXTERM_TEST_REMOTE_TMUX_SOCKET 传给 list_ssh_tmux_sessions，否则 Host local 测会打到用户默认 server"
    );
}

/// C9：本地 list 也必须能指到隔离 `-L`，才能和 ssh-self 对同一 session 出双份。
#[test]
fn tmux_driver_list_honors_test_local_socket_env() {
    let src = include_str!("builtin/tmux.rs");
    assert!(
        src.contains("MUXTERM_TEST_LOCAL_TMUX_SOCKET"),
        "TmuxDriver::list 本地分支必须读 MUXTERM_TEST_LOCAL_TMUX_SOCKET 传给 list_local_tmux_sessions，否则 all 的 local 半边会打到用户默认 server"
    );
}

/// C9：`all` 扇出 local 单例 + 每个 SSH target，同名 session 保留两行。
#[test]
fn discover_sessions_all_fans_out_local_and_ssh_targets() {
    let mut cat = Catalog::new();
    cat.register_transport(Box::new(MockTransport {
        id: "local",
        name: "Local",
        connects: Arc::new(AtomicUsize::new(0)),
        fail: false,
        targets: vec![TargetInfo::new("", "local")],
    }));
    cat.register_transport(Box::new(MockTransport {
        id: "ssh",
        name: "SSH",
        connects: Arc::new(AtomicUsize::new(0)),
        fail: false,
        targets: vec![TargetInfo::new("self", "self")],
    }));
    cat.register_runtime(Box::new(MockDriver {
        id: "tmux",
        name: "tmux",
        accepted: &["local", "ssh"],
        support: &[RuntimeCapability::Discover],
        listed: vec![SessionCandidate {
            runtime_id: "tmux".into(),
            transport_id: String::new(),
            target: String::new(),
            namespace: None,
            name: "mux-dup".into(),
            extra: String::new(),
        }],
        list_err: false,
        opened: Arc::new(AtomicUsize::new(0)),
    }));
    let rows = cat.discover_sessions("all", "").expect("all 不应 Err");
    assert!(
        rows.iter().any(|s| {
            s.runtime_id == "tmux" && s.transport_id == "local" && s.name == "mux-dup"
        }),
        "all 必须含 local 行 mux-dup: {rows:?}"
    );
    assert!(
        rows.iter().any(|s| {
            s.runtime_id == "tmux"
                && s.transport_id == "ssh"
                && s.target == "self"
                && s.name == "mux-dup"
        }),
        "all 必须含 ssh-self 行 mux-dup（双份，禁止去重）: {rows:?}"
    );
}

/// C9：`all` 禁止串行等每个 SSH host；慢 host 不能把整表拖成超时之和。
#[test]
fn discover_sessions_all_must_fan_out_in_parallel() {
    struct SlowListDriver {
        active: Arc<AtomicUsize>,
        max_active: Arc<AtomicUsize>,
    }

    impl RuntimeDriver for SlowListDriver {
        fn id(&self) -> &'static str {
            "slow"
        }

        fn name(&self) -> &'static str {
            "slow"
        }

        fn support(&self) -> &'static [RuntimeCapability] {
            &[RuntimeCapability::Discover]
        }

        fn accepted_transports(&self) -> &'static [&'static str] {
            &["local", "ssh"]
        }

        fn list(
            &self,
            connect: &Connect,
            _namespace: Option<&str>,
        ) -> anyhow::Result<Vec<SessionCandidate>> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(100));
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(vec![SessionCandidate {
                runtime_id: "slow".into(),
                transport_id: connect.transport_id().into(),
                target: connect.target().into(),
                namespace: None,
                name: "candidate".into(),
                extra: String::new(),
            }])
        }

        fn open(
            &self,
            _connect: Arc<Connect>,
            _spec: &WorkspaceSpec,
        ) -> anyhow::Result<Box<dyn Runtime>> {
            Ok(Box::new(MockRuntime::with_single_pane()))
        }
    }

    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let mut cat = Catalog::new();
    cat.register_transport(Box::new(MockTransport {
        id: "local",
        name: "Local",
        connects: Arc::new(AtomicUsize::new(0)),
        fail: false,
        targets: vec![TargetInfo::new("", "local")],
    }));
    cat.register_transport(Box::new(MockTransport {
        id: "ssh",
        name: "SSH",
        connects: Arc::new(AtomicUsize::new(0)),
        fail: false,
        targets: (0..4)
            .map(|n| TargetInfo::new(format!("host-{n}"), format!("host-{n}")))
            .collect(),
    }));
    cat.register_runtime(Box::new(SlowListDriver {
        active,
        max_active: max_active.clone(),
    }));

    let rows = cat.discover_sessions("all", "").expect("all 不应 Err");
    assert_eq!(rows.len(), 5, "local + 4 SSH host 都必须返回: {rows:?}");
    assert!(
        max_active.load(Ordering::SeqCst) >= 2,
        "必须观察到多个 connect 同时 list；源码里出现 thread/scope 不算行为证据"
    );
}
