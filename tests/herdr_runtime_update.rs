//! 真实隔离 Herdr 的更新/交接/重连。只替换下载步骤，绝不更新用户安装。
#![cfg(feature = "test-harness")]
mod support;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{ensure, Result};
use muxterm::test_support::core::{
    protocol::{task::Task, WorkspaceId},
    runtime::herdr::{HerdrRuntime, HerdrSession},
    transport::{
        connection::Connect, ByteChannel, ChannelRequest, CommandOutput, TargetConnection,
        TransportResult,
    },
    workspace::Workspace,
};
use serde_json::{json, Value};
use support::herdr_test_support::{herdr_available, seed_herdr_viewport, IsolatedHerdr};
use support::sshd_test_support::LoopbackSshd;

struct Installer {
    connection: Arc<dyn TargetConnection>,
    updated_binary: Option<PathBuf>,
    expected_protocol: u32,
    failed: AtomicBool,
    force_handoff: Arc<AtomicBool>,
    installs: AtomicUsize,
    handoffs: Arc<AtomicUsize>,
}
impl TargetConnection for Installer {
    fn transport_id(&self) -> &str {
        self.connection.transport_id()
    }
    fn target(&self) -> &str {
        self.connection.target()
    }
    fn probe(&self) -> TransportResult<()> {
        self.connection.probe()
    }
    fn exec_command(&self, request: ChannelRequest) -> TransportResult<CommandOutput> {
        // --version/schema/command -v 是真实目标命令；不执行 updater。
        if let (Some(binary), ChannelRequest::Exec { argv, .. }) = (&self.updated_binary, &request)
        {
            if !argv
                .iter()
                .any(|arg| arg == "muxterm-runtime-update" || arg.contains("command -v herdr"))
            {
                return self.connection.exec_command(request);
            }
            let binary = binary.to_string_lossy().into_owned();
            let argv = if argv.iter().any(|arg| arg.contains("command -v herdr")) {
                vec![
                    "sh".into(),
                    "-c".into(),
                    r#"printf '%s\n' "$1""#.into(),
                    "muxterm-test-update".into(),
                    binary,
                ]
            } else {
                let mut args = vec![binary];
                args.extend_from_slice(&argv[4..]);
                args
            };
            return self.connection.exec_command(ChannelRequest::Exec {
                argv,
                cwd: None,
                env: vec![],
                pty: None,
            });
        }
        self.connection.exec_command(request)
    }
    fn open_channel(&self, request: ChannelRequest) -> TransportResult<Box<dyn ByteChannel>> {
        if let ChannelRequest::Exec { argv, .. } = &request {
            assert!(
                argv.iter().any(|arg| arg.contains("MUXTERM_UPDATE_EXIT=")),
                "unexpected command: {argv:?}"
            );
            let script = &argv[2];
            assert!(script.contains("unset HERDR_ENV HERDR_SESSION"));
            assert!(script.contains("herdr update </dev/null"));
            assert!(!script.contains("--handoff") && !script.contains("server stop"));
            self.installs.fetch_add(1, Ordering::SeqCst);
            let final_chunk = if self.failed.load(Ordering::SeqCst) {
                b"1\r\n".to_vec()
            } else {
                b"0\r\n".to_vec()
            };
            return Ok(Box::new(InstallerChannel(VecDeque::from([
                b"test installer output\r\nMUXTERM_UPDATE_".to_vec(),
                b"EXIT=".to_vec(),
                final_chunk,
            ]))));
        }
        let is_api = matches!(&request, ChannelRequest::UnixSocket { path } if path.file_name().is_some_and(|name| name == "herdr.sock"));
        let channel = self.connection.open_channel(request)?;
        if is_api {
            Ok(Box::new(ApiChannel {
                channel,
                request: Vec::new(),
                response: Vec::new(),
                force_handoff: Arc::clone(&self.force_handoff),
                handoffs: Arc::clone(&self.handoffs),
                counted: false,
                expected_protocol: self.expected_protocol,
            }))
        } else {
            Ok(channel)
        }
    }
}
struct InstallerChannel(VecDeque<Vec<u8>>);
impl ByteChannel for InstallerChannel {
    fn read(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        Ok(self.0.pop_front())
    }
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        panic!("installer must not receive terminal input")
    }
    fn resize(&mut self, _: u16, _: u16) -> TransportResult<()> {
        Ok(())
    }
    fn shutdown(&mut self) -> TransportResult<()> {
        Ok(())
    }
}
struct ApiChannel {
    channel: Box<dyn ByteChannel>,
    request: Vec<u8>,
    response: Vec<u8>,
    force_handoff: Arc<AtomicBool>,
    handoffs: Arc<AtomicUsize>,
    counted: bool,
    expected_protocol: u32,
}
impl ByteChannel for ApiChannel {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let written = self.channel.write(data)?;
        self.request.extend_from_slice(&data[..written]);
        if !self.counted && self.request.contains(&b'\n') {
            let request: Value = serde_json::from_slice(&self.request)?;
            if request["method"] == "server.live_handoff" {
                assert!(request["params"]["import_exe"]
                    .as_str()
                    .unwrap()
                    .starts_with('/'));
                assert_eq!(
                    request["params"]["expected_protocol"],
                    self.expected_protocol
                );
                self.handoffs.fetch_add(1, Ordering::SeqCst);
            }
            assert!(request["method"] != "server.stop");
            self.counted = true;
        }
        Ok(written)
    }
    fn read(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        let request: Value = serde_json::from_slice(&self.request)?;
        let handoff = request["method"] == "server.live_handoff";
        if !handoff && (request["method"] != "ping" || !self.force_handoff.load(Ordering::SeqCst)) {
            return self.channel.read();
        }
        if let Some(bytes) = self.channel.read()? {
            self.response.extend(bytes);
        }
        if !self.response.contains(&b'\n') {
            return Ok(None);
        }
        let mut response: Value = serde_json::from_slice(&self.response)?;
        if handoff {
            assert!(
                response.get("error").is_none(),
                "real live handoff failed: {response}"
            );
        } else {
            // 强制走交接，import_exe 仍为本机同一二进制，不下载、不替换安装。
            response["result"]["version"] = json!("0.7.99-test");
        }
        let mut bytes = serde_json::to_vec(&response)?;
        bytes.push(b'\n');
        self.response.clear();
        Ok(Some(bytes))
    }
    fn resize(&mut self, cols: u16, rows: u16) -> TransportResult<()> {
        self.channel.resize(cols, rows)
    }
    fn shutdown(&mut self) -> TransportResult<()> {
        self.channel.shutdown()
    }
}

fn wait_workspaces(
    workspaces: &mut [Workspace],
    ready: impl Fn(&[Workspace]) -> bool,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        for workspace in workspaces.iter_mut() {
            workspace.refresh();
        }
        if ready(workspaces) {
            return Ok(());
        }
        ensure!(
            Instant::now() < deadline,
            "workspace update did not converge: {:?}, active surfaces: {:?}",
            workspaces
                .iter()
                .map(|ws| ws.runtime().update_status())
                .collect::<Vec<_>>(),
            workspaces
                .iter()
                .map(|ws| ws.state().active_pane().map(|p| (
                    p.id,
                    ws.pane_text(p.id),
                    ws.runtime().diagnostics(p.id)
                )))
                .collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn shell_pid(session: &HerdrSession, pane: &str) -> Result<Value> {
    Ok(
        session.call("pane.process_info", json!({"pane_id":pane}))?["process_info"]["shell_pid"]
            .clone(),
    )
}

#[test]
fn local_and_ssh_updates_preserve_shared_workspaces_and_input() -> Result<()> {
    ensure!(
        herdr_available(),
        "real Herdr binary is required for runtime-update regression"
    );
    let sshd = LoopbackSshd::start("runtime-update")?;
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(value) = &self.0 {
                std::env::set_var("MUXTERM_SSH_CONFIG_PATH", value);
            } else {
                std::env::remove_var("MUXTERM_SSH_CONFIG_PATH");
            }
        }
    }
    let _restore = Restore(std::env::var_os("MUXTERM_SSH_CONFIG_PATH"));
    sshd.apply_ssh_config_env();
    // CI 默认同一版本；显式指定官方二进制可验证 19 → 22 等跨版本热交接。
    let updated_binary = std::env::var_os("MUXTERM_TEST_HERDR_UPDATE_BINARY").map(PathBuf::from);
    let expected_protocol = if let Some(binary) = &updated_binary {
        let output = std::process::Command::new(binary)
            .args(["api", "schema", "--json"])
            .output()?;
        ensure!(output.status.success(), "cannot read test binary schema");
        let schema: Value = serde_json::from_slice(&output.stdout)?;
        u32::try_from(schema["protocol"].as_u64().unwrap())?
    } else {
        19
    };
    for (transport, target) in [("local", ""), ("ssh", sshd.alias.as_str())] {
        let herdr = IsolatedHerdr::start("runtime-update");
        let installer = Arc::new(Installer {
            connection: Connect::new(transport, target),
            updated_binary: updated_binary.clone(),
            expected_protocol,
            failed: AtomicBool::new(false),
            force_handoff: Arc::new(AtomicBool::new(false)),
            installs: AtomicUsize::new(0),
            handoffs: Arc::new(AtomicUsize::new(0)),
        });
        let session = Arc::new(HerdrSession::with_connection(
            installer.clone(),
            herdr.name(),
            herdr.socket_path(),
        ));
        let mut workspaces = Vec::new();
        let mut tokens = Vec::new();
        let mut active_targets = Vec::new();
        let rt = tokio::runtime::Runtime::new()?;
        for n in 0..2 {
            let record = session.workspace_create("/tmp", &format!("update-{n}"))?;
            session.call(
                "tab.create",
                json!({"workspace_id":record.workspace_id,"cwd":"/tmp","focus":false}),
            )?;
            let pane = session
                .snapshot()?
                .panes
                .into_iter()
                .find(|p| p.workspace_id == record.workspace_id)
                .unwrap()
                .pane_id;
            herdr.split_pane(&pane, "right");
            active_targets.push(pane);
            for pane in session
                .snapshot()?
                .panes
                .iter()
                .filter(|p| p.workspace_id == record.workspace_id)
            {
                herdr.wait_for_pane_process(&pane.pane_id);
                let token = format!(
                    "UPDATE_KEEP_{transport}_{n}_{}",
                    pane.pane_id.replace(':', "_")
                );
                herdr.paint_until_token(&pane.pane_id, &token);
                tokens.push((
                    pane.pane_id.clone(),
                    token,
                    shell_pid(&session, &pane.pane_id)?,
                ));
            }
            let mut workspace = Workspace::new(
                WorkspaceId::new(
                    transport,
                    Some(target),
                    herdr.name(),
                    "herdr",
                    &record.workspace_id,
                ),
                format!("update-{n}"),
                Box::new(HerdrRuntime::new(
                    Arc::clone(&session),
                    &record.workspace_id,
                )),
            );
            rt.block_on(workspace.connect())?;
            seed_herdr_viewport(&mut workspace, 100, 30)?;
            workspaces.push(workspace);
        }
        wait_workspaces(&mut workspaces, |all| {
            all.iter()
                .all(|ws| !ws.search_workspace("UPDATE_KEEP_").is_empty())
        })?;
        let before = workspaces
            .iter()
            .map(|ws| {
                ws.state()
                    .tabs()
                    .iter()
                    .map(|t| {
                        (
                            t.id,
                            ws.state()
                                .panes(&t.id)
                                .iter()
                                .map(|p| p.id)
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        installer.force_handoff.store(true, Ordering::SeqCst);
        workspaces[0].runtime_mut().start_update()?;
        wait_workspaces(&mut workspaces, |all| {
            all.iter().all(|ws| {
                ws.runtime()
                    .update_status()
                    .is_some_and(|s| s.phase == "succeeded")
            })
        })?;
        ensure!(
            installer.installs.load(Ordering::SeqCst) == 1,
            "one install for shared session"
        );
        ensure!(
            installer.handoffs.load(Ordering::SeqCst) == 1,
            "must exercise actual live handoff"
        );
        ensure!(
            session.snapshot()?.protocol == u64::from(expected_protocol),
            "server did not switch to the selected binary"
        );
        for (ws, expected) in workspaces.iter().zip(before) {
            let after = ws
                .state()
                .tabs()
                .iter()
                .map(|t| {
                    (
                        t.id,
                        ws.state()
                            .panes(&t.id)
                            .iter()
                            .map(|p| p.id)
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>();
            ensure!(after == expected, "tab/pane identity changed during update");
        }
        for (pane, token, pid) in &tokens {
            ensure!(
                shell_pid(&session, pane)? == *pid && !pid.is_null(),
                "pane process was restarted"
            );
            let content = session.pane_read_recent_ansi(pane)?;
            ensure!(
                String::from_utf8_lossy(&content)
                    .replace(['\r', '\n'], "")
                    .contains(token),
                "existing content lost on {transport} {pane}, expected {token}: {:?}",
                String::from_utf8_lossy(&content)
            );
        }
        for workspace in &mut workspaces {
            let pane = workspace.state().active_pane().unwrap().id;
            workspace.execute(Task::WriteRaw {
                target: pane,
                data: "更新中文".as_bytes().to_vec(),
            })?;
        }
        // 服务端原始回显检查 Unicode，不用 Index 的逐 cell 文本（宽字符含占位格）。
        wait_workspaces(&mut workspaces, |_| {
            active_targets.iter().all(|target| {
                session.pane_read_ansi(target).is_ok_and(|bytes| {
                    String::from_utf8_lossy(&bytes).matches("更新中文").count() == 1
                })
            })
        })?;
        installer.failed.store(true, Ordering::SeqCst);
        workspaces[0].runtime_mut().start_update()?;
        wait_workspaces(&mut workspaces, |all| {
            all.iter().all(|ws| {
                ws.runtime()
                    .update_status()
                    .is_some_and(|s| s.phase == "failed")
            })
        })?;
        ensure!(
            installer.handoffs.load(Ordering::SeqCst) == 1,
            "failed install must leave server alone"
        );
        for workspace in &mut workspaces {
            let pane = workspace.state().active_pane().unwrap().id;
            workspace.execute(Task::WriteRaw {
                target: pane,
                data: b"\x15printf 'AFTER_%s\\n' 'FAILED'\r".to_vec(),
            })?;
        }
        wait_workspaces(&mut workspaces, |all| {
            all.iter().all(|ws| {
                ws.pane_text(ws.state().active_pane().unwrap().id)
                    .matches("AFTER_FAILED")
                    .count()
                    == 1
            })
        })?;
        for (pane, _, pid) in &tokens {
            ensure!(
                shell_pid(&session, pane)? == *pid,
                "failed update changed shell PID"
            );
        }
    }
    Ok(())
}
