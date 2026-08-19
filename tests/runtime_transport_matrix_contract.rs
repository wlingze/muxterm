//! 所有内置 Runtime x Transport 的真实 2tab3pane 契约。
//!
//! 组合直接来自生产 Catalog 注册表；新增插件会自动产生新 case，若没有测试
//! fixture 或实现不接受该组合，本测试必须失败，禁止静默跳过。

mod support;

use std::any::Any;
use std::ffi::OsString;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::process::Command;

use anyhow::{ensure, Context, Result};

use muxterm::core::catalog::Catalog;
use muxterm::core::model::task::{Task, TaskOutcome};
use muxterm::core::workspace::spec::WorkspaceSpec;
use support::herdr_test_support::herdr_available;
use support::runtime_transport_matrix::{
    build_2tab3pane, verify_after_attach, verify_after_pool_switch, verify_fresh_workspace,
    verify_ssh_shell_transport, MatrixFixture,
};
use support::sshd_test_support::{loopback_sshd_available, LoopbackSshd};
use support::tmux_test_support::tmux_available;

struct EnvRestore {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvRestore {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        if let Some(value) = self.previous.take() {
            std::env::set_var(self.key, value);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

fn run_case(runtime_id: &str, transport_id: &str, sshd: &LoopbackSshd) -> Result<()> {
    let fixture = MatrixFixture::new(runtime_id, transport_id, sshd)?;
    let spec = fixture.spec.clone();
    let alternate_spec = fixture.alternate_spec.clone();
    let workspace_id = spec.id();
    let alternate_id = alternate_spec.id();
    ensure!(
        workspace_id != alternate_id,
        "{runtime_id} x {transport_id} 的两个 fixture 必须是不同 WorkspaceId"
    );
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .context("创建 Tokio runtime")?;

    let mut catalog = Catalog::with_builtins();
    let snapshot = {
        let workspace = rt
            .block_on(catalog.open(&spec))
            .with_context(|| format!("打开 {runtime_id} x {transport_id}"))?;
        ensure!(
            workspace.runtime().workspace_runtime() == runtime_id,
            "打开了错误 Runtime: expected={runtime_id}, actual={}",
            workspace.runtime().workspace_runtime()
        );
        if runtime_id == "shell" && transport_id == "ssh" {
            verify_ssh_shell_transport(workspace)?;
        }
        build_2tab3pane(workspace, runtime_id, transport_id).with_context(|| {
            if runtime_id == "tmux" {
                tmux_fixture_diagnostics(&spec)
            } else {
                "构造 2tab3pane 失败".to_string()
            }
        })?
    };

    let alternate_token = {
        let alternate = rt
            .block_on(catalog.open(&alternate_spec))
            .with_context(|| format!("创建第二个 {runtime_id} x {transport_id} Workspace"))?;
        ensure!(
            alternate.runtime().workspace_runtime() == runtime_id,
            "第二个 Workspace 打开了错误 Runtime: expected={runtime_id}, actual={}",
            alternate.runtime().workspace_runtime()
        );
        verify_fresh_workspace(alternate, runtime_id, transport_id)?
    };
    ensure!(
        catalog.pool().len() == 2,
        "{runtime_id} x {transport_id} 创建第二个 Workspace 后池中必须恰有两个，实际 {}",
        catalog.pool().len()
    );
    ensure!(
        catalog.pool().active_id() == Some(&alternate_id),
        "创建第二个 Workspace 后必须切到它"
    );
    {
        let workspace = catalog
            .pool_mut()
            .activate(&workspace_id)
            .context("WorkspacePool::activate 无法切回原 Workspace")?;
        verify_after_pool_switch(
            workspace,
            runtime_id,
            transport_id,
            &snapshot,
            &alternate_token,
        )?;
    }
    ensure!(
        catalog.pool().active_id() == Some(&workspace_id),
        "WorkspacePool::activate 后 active_id 必须是原 Workspace"
    );

    if snapshot.persistent {
        let workspace = catalog
            .pool_mut()
            .get_mut(&workspace_id)
            .context("detach 前原 Workspace 不应从池中消失")?;
        let outcome = workspace.execute(Task::Detach)?;
        ensure!(
            outcome == TaskOutcome::Done,
            "{runtime_id} x {transport_id} 声明 PersistDetach 后 detach 必须成功，实际 {outcome:?}"
        );
        if let Some(alternate) = catalog.pool_mut().get_mut(&alternate_id) {
            rt.block_on(alternate.shutdown())?;
        }
        // 真正丢掉旧 Runtime，再从相同 spec 新建实例 attach；不能依赖旧本地状态。
        drop(catalog);
        let mut attached_catalog = Catalog::with_builtins();
        let attached = rt
            .block_on(attached_catalog.open(&spec))
            .with_context(|| format!("重新 attach {runtime_id} x {transport_id}"))?;
        verify_after_attach(attached, runtime_id, transport_id, &snapshot)?;
        rt.block_on(attached.shutdown())?;
    } else {
        // 非持久 Runtime 不伪造 detach/attach；池内切走、切回和继续输入即是契约。
        let workspace = catalog
            .pool_mut()
            .get_mut(&workspace_id)
            .context("切回后 shell Workspace 不应从池中消失")?;
        ensure!(
            runtime_id == "shell"
                && !workspace
                    .runtime()
                    .support()
                    .contains(&muxterm::core::model::backend::RuntimeCapability::PersistDetach),
            "没有 PersistDetach 的内置 Runtime 必须是 shell"
        );
        ensure!(
            matches!(
                workspace.execute(Task::Detach)?,
                TaskOutcome::Rejected { .. }
            ),
            "shell 不得伪造 detach 成功"
        );
        rt.block_on(workspace.shutdown())?;
        if let Some(alternate) = catalog.pool_mut().get_mut(&alternate_id) {
            rt.block_on(alternate.shutdown())?;
        }
    }
    drop(fixture);
    Ok(())
}

fn tmux_fixture_diagnostics(spec: &WorkspaceSpec) -> String {
    let Some(socket) = spec.socket.as_deref() else {
        return "tmux fixture 缺隔离 socket".into();
    };
    let output = Command::new("tmux")
        .args([
            "-L",
            socket,
            "list-panes",
            "-a",
            "-F",
            "#{session_name} #{window_id} #{pane_id} #{pane_active} #{window_layout}",
        ])
        .output();
    match output {
        Ok(output) => format!(
            "隔离 tmux 原生 pane:\n{}stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
        Err(error) => format!("读取隔离 tmux 原生 pane 失败: {error}"),
    }
}

fn panic_text(payload: Box<dyn Any + Send>) -> String {
    if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else {
        "non-string panic".into()
    }
}

#[test]
fn every_registered_runtime_transport_passes_2tab3pane_pool_switch_and_supported_reattach() {
    assert!(tmux_available(), "六格矩阵要求 tmux fixture 可用");
    assert!(herdr_available(), "六格矩阵要求 Herdr fixture 可用");
    assert!(
        loopback_sshd_available(),
        "六格矩阵要求可自启的 loopback sshd"
    );
    let sshd = LoopbackSshd::start("runtime-matrix").expect("启动隔离 loopback sshd");
    let _ssh_config = EnvRestore::set("MUXTERM_SSH_CONFIG_PATH", &sshd.config_path);

    let registry = Catalog::with_builtins();
    let runtimes = registry.runtime_list();
    let transports = registry.transport_list();
    for expected in ["tmux", "herdr", "shell"] {
        assert!(
            runtimes.iter().any(|runtime| runtime.id == expected),
            "runtime_list 缺内置 {expected}"
        );
    }
    for expected in ["local", "ssh"] {
        assert!(
            transports.iter().any(|transport| transport.id == expected),
            "transport_list 缺内置 {expected}"
        );
    }

    let mut failures = Vec::new();
    let mut executed = 0usize;
    for runtime in &runtimes {
        for transport in &transports {
            executed += 1;
            if !runtime.accepted_transports.contains(&transport.id) {
                failures.push(format!(
                    "{} x {}: RuntimeInfo.accepted_transports 未声明该组合",
                    runtime.id, transport.id
                ));
                continue;
            }
            match catch_unwind(AssertUnwindSafe(|| {
                run_case(&runtime.id, &transport.id, &sshd)
            })) {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    failures.push(format!("{} x {}: {error:#}", runtime.id, transport.id))
                }
                Err(payload) => failures.push(format!(
                    "{} x {} panic: {}",
                    runtime.id,
                    transport.id,
                    panic_text(payload)
                )),
            }
        }
    }

    assert_eq!(
        executed,
        runtimes.len() * transports.len(),
        "必须执行 runtime_list x transport_list 完整笛卡尔积"
    );
    assert!(
        failures.is_empty(),
        "Runtime x Transport 2tab3pane 矩阵失败（{executed} cases）:\n{}",
        failures.join("\n\n")
    );
}
