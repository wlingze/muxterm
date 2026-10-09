//! Herdr 安装与热交接。禁止 fallback 到 server.stop 或终端命令注入。
use super::session::HerdrSession;
use super::session::SessionSnapshot;
use super::wire_compat::WireProtocol;
use crate::runtime::update::RuntimeUpdateStatus;
use crate::transport::ChannelRequest;
use anyhow::{bail, ensure, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::{mpsc, Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

trait UpdateBackend {
    fn ping(&self) -> Result<Value>;
    fn install(&self) -> Result<()>;
    fn installed_version(&self) -> Result<String>;
    fn installed_protocol(&self) -> Result<u32>;
    fn handoff(&self, version: &str, protocol: u32) -> Result<()>;
    fn wait_snapshot(&self, version: &str) -> Result<SessionSnapshot>;
}

fn update(
    backend: &dyn UpdateBackend,
    progress: impl Fn(RuntimeUpdateStatus),
) -> Result<UpdateFinish> {
    let ping = backend.ping()?;
    ensure!(
        ping["capabilities"]["live_handoff"] == true,
        "This server does not support live update. Its panes were left running."
    );
    progress(RuntimeUpdateStatus::new(
        "installing",
        "Installing runtime update…",
    ));
    backend.install()?;
    let version = backend.installed_version()?;
    ensure!(
        !version.is_empty(),
        "Installed runtime did not report a version"
    );
    let protocol = backend.installed_protocol()?;
    ensure!(WireProtocol::from_number(protocol).is_ok(),
        "Installed Herdr protocol {protocol} is not supported by this Muxterm. The existing server was left running; update Muxterm before reconnecting.");
    // plain `herdr update` 只换磁盘上的二进制，正在跑的 server 仍是旧进程。
    // 版本或 protocol 任一不同，就是这次更新要做的 live handoff。
    // 只有 ping 已经同时报出安装后的版本和 protocol，才说明这个进程就是那份二进制。
    let running_version = ping["version"].as_str().unwrap_or_default();
    let running_protocol = ping["protocol"].as_u64();
    if running_version == version && running_protocol == Some(u64::from(protocol)) {
        return Ok(UpdateFinish::AlreadyCurrent(version));
    }
    let previous = if running_version.is_empty() {
        "the running server".to_owned()
    } else {
        running_version.to_owned()
    };
    progress(RuntimeUpdateStatus::new(
        "handoff",
        format!("Handing the running server {previous} to {version}…"),
    ));
    // 交接可能已经成功但回复连接提前断开；snapshot 才是最终事实。
    let handoff_error = backend.handoff(&version, protocol).err();
    progress(RuntimeUpdateStatus::new(
        "reconnecting",
        "Keeping panes on the updated server…",
    ));
    let snapshot = backend
        .wait_snapshot(&version)
        .map_err(|error| match handoff_error {
            Some(handoff) => anyhow::anyhow!(
                "Runtime handoff failed: {handoff:#}. Reconnect verification: {error:#}"
            ),
            None => error,
        })?;
    ensure!(
        !snapshot.workspaces.is_empty(),
        "Updated server snapshot has no workspaces; the running workspace was left unchanged"
    );
    ensure!(
        snapshot.protocol == u64::from(protocol),
        "Updated server protocol {} differs from installed protocol {protocol}",
        snapshot.protocol
    );
    Ok(UpdateFinish::Updated {
        version,
        previous,
        snapshot: Box::new(snapshot),
    })
}

static INSTALLING: LazyLock<Mutex<HashSet<(String, String)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
struct InstallGuard((String, String));
impl Drop for InstallGuard {
    fn drop(&mut self) {
        if let Ok(mut targets) = INSTALLING.lock() {
            targets.remove(&self.0);
        }
    }
}
#[derive(Debug)]
enum UpdateFinish {
    /// 安装后的版本和正在运行的 server 相同。不要再重开 pane 流。
    AlreadyCurrent(String),
    /// 旧进程已交给安装后的二进制。`previous` 是交接前 ping 到的 server 版本。
    Updated {
        version: String,
        previous: String,
        snapshot: Box<SessionSnapshot>,
    },
}
enum UpdateEvent {
    Progress(RuntimeUpdateStatus),
    Finished(Result<UpdateFinish>),
}
pub(super) struct UpdateState {
    status: RuntimeUpdateStatus,
    receiver: Option<mpsc::Receiver<UpdateEvent>>,
    revision: u64,
    snapshot: Option<SessionSnapshot>,
}
impl Default for UpdateState {
    fn default() -> Self {
        Self {
            status: RuntimeUpdateStatus::new("idle", "Update runtime"),
            receiver: None,
            revision: 0,
            snapshot: None,
        }
    }
}
impl UpdateState {
    fn poll(&mut self) {
        loop {
            let event = match self.receiver.as_ref().map(|rx| rx.try_recv()) {
                Some(Ok(e)) => e,
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    self.status = RuntimeUpdateStatus::new("failed", "Update worker disconnected");
                    self.receiver = None;
                    break;
                }
                _ => break,
            };
            match event {
                UpdateEvent::Progress(s) => self.status = s,
                UpdateEvent::Finished(result) => {
                    self.receiver = None;
                    match result {
                        Ok(UpdateFinish::AlreadyCurrent(version)) => {
                            self.status = RuntimeUpdateStatus::new(
                                "succeeded",
                                format!("Runtime {version} is already current"),
                            );
                            self.status.version = Some(version);
                        }
                        Ok(UpdateFinish::Updated {
                            version,
                            previous,
                            snapshot,
                        }) => {
                            self.revision += 1;
                            self.snapshot = Some(*snapshot);
                            self.status = RuntimeUpdateStatus::new(
                                "updated",
                                format!(
                                    "Runtime {previous} handed off to {version}; pane processes kept"
                                ),
                            );
                            self.status.version = Some(version);
                        }
                        Err(e) => {
                            self.status = RuntimeUpdateStatus::new("failed", format!("{e:#}"))
                        }
                    }
                    break;
                }
            }
        }
    }
}

impl HerdrSession {
    pub(super) fn start_update(self: &Arc<Self>) -> Result<()> {
        let mut state = self
            .update
            .lock()
            .map_err(|_| anyhow::anyhow!("update state poisoned"))?;
        state.poll();
        ensure!(!state.status.busy(), "Runtime update already in progress");
        let key = (self.transport_id().to_owned(), self.target().to_owned());
        ensure!(
            INSTALLING
                .lock()
                .map_err(|_| anyhow::anyhow!("update registry poisoned"))?
                .insert(key.clone()),
            "Another runtime update is already running on this target"
        );
        let guard = InstallGuard(key);
        let session = Arc::clone(self);
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("muxterm-runtime-update".into())
            .spawn(move || {
                let _guard = guard;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    update(session.as_ref(), |status| {
                        let _ = tx.send(UpdateEvent::Progress(status));
                    })
                }))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Runtime update worker panicked")));
                let _ = tx.send(UpdateEvent::Finished(result));
            })?;
        state.status = RuntimeUpdateStatus::new("checking", "Checking runtime update support…");
        state.receiver = Some(rx);
        Ok(())
    }
    pub(super) fn runtime_update_status(&self) -> RuntimeUpdateStatus {
        let mut state = self.update.lock().unwrap_or_else(|e| e.into_inner());
        state.poll();
        state.status.clone()
    }
    pub(super) fn runtime_update_revision(&self) -> u64 {
        let mut state = self.update.lock().unwrap_or_else(|e| e.into_inner());
        state.poll();
        state.revision
    }
    pub(super) fn update_snapshot_after(&self, revision: u64) -> Option<(u64, SessionSnapshot)> {
        let mut state = self.update.lock().unwrap_or_else(|e| e.into_inner());
        state.poll();
        (state.revision > revision)
            .then(|| state.snapshot.clone().map(|s| (state.revision, s)))
            .flatten()
    }

    #[cfg(test)]
    pub(super) fn publish_test_handoff(
        &self,
        version: &str,
        previous: &str,
        snapshot: SessionSnapshot,
    ) {
        let mut state = self
            .update
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.revision += 1;
        state.snapshot = Some(snapshot);
        state.status = RuntimeUpdateStatus::new(
            "updated",
            format!("Runtime {previous} handed off to {version}; pane processes kept"),
        );
        state.status.version = Some(version.to_owned());
    }
}

fn parse_version(output: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(output)?.trim();
    let version = text
        .strip_prefix("herdr ")
        .and_then(|v| v.split_whitespace().next())
        .ok_or_else(|| anyhow::anyhow!("Unexpected Herdr version response: {text}"))?;
    Ok(version.to_owned())
}

// 与安装脚本共用 PATH，避免 GUI 启动缺少 ~/.local/bin 时读到另一份二进制。
const HERDR_EXEC: &str = r#"PATH="$HOME/.local/bin:$HOME/.nix-profile/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"; export PATH; exec herdr "$@""#;
impl HerdrSession {
    fn update_exec(&self, args: &[&str]) -> Result<crate::transport::CommandOutput> {
        let mut argv = vec![
            "sh".into(),
            "-c".into(),
            HERDR_EXEC.into(),
            "muxterm-runtime-update".into(),
        ];
        argv.extend(args.iter().map(|arg| (*arg).to_owned()));
        Ok(self.connection().exec_command(ChannelRequest::Exec {
            argv,
            cwd: None,
            env: vec![],
            pty: None,
        })?)
    }
}
impl UpdateBackend for HerdrSession {
    fn ping(&self) -> Result<Value> {
        self.call("ping", json!({}))
    }
    fn install(&self) -> Result<()> {
        // 独立 Exec，不向 pane 写字节。stdin=/dev/null 确保 CLI 不能确认停止任何 server。
        const SCRIPT: &str = r#"
unset HERDR_ENV HERDR_SESSION
PATH="$HOME/.local/bin:$HOME/.nix-profile/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
export PATH
herdr update </dev/null
result=$?
printf '\nMUXTERM_UPDATE_EXIT=%s\n' "$result"
"#;
        let mut channel = self.connection().open_channel(ChannelRequest::Exec {
            argv: vec!["sh".into(), "-c".into(), SCRIPT.into()],
            cwd: None,
            env: Vec::new(),
            pty: None,
        })?;
        let result = (|| {
            let deadline = Instant::now() + Duration::from_secs(300);
            let mut bytes = Vec::new();
            loop {
                if let Some(chunk) = channel.read()? {
                    bytes.extend(chunk);
                    if bytes.len() > 65536 {
                        bytes.drain(..bytes.len() - 65536);
                    }
                    let text = String::from_utf8_lossy(&bytes);
                    if let Some((_, code)) = text.rsplit_once("MUXTERM_UPDATE_EXIT=") {
                        if code.contains('\n') {
                            ensure!(
                                code.trim() == "0",
                                "Runtime installation failed: {}",
                                text.split("MUXTERM_UPDATE_EXIT=")
                                    .next()
                                    .unwrap_or_default()
                                    .trim()
                            );
                            return Ok(());
                        }
                    }
                }
                ensure!(
                    Instant::now() < deadline,
                    "Runtime installation timed out; existing server was not stopped"
                );
                std::thread::sleep(Duration::from_millis(40));
            }
        })();
        let _ = channel.shutdown();
        result
    }
    fn installed_version(&self) -> Result<String> {
        let out = self.update_exec(&["--version"])?;
        ensure!(
            out.status == 0,
            "Cannot read installed Herdr version: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        parse_version(&out.stdout)
    }
    fn installed_protocol(&self) -> Result<u32> {
        let out = self.update_exec(&["api", "schema", "--json"])?;
        ensure!(
            out.status == 0,
            "Cannot read installed Herdr API schema: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let schema: Value = serde_json::from_slice(&out.stdout)?;
        schema["protocol"]
            .as_u64()
            .and_then(|p| u32::try_from(p).ok())
            .ok_or_else(|| anyhow::anyhow!("Installed Herdr API schema has no valid protocol"))
    }
    fn handoff(&self, version: &str, protocol: u32) -> Result<()> {
        let out = self.connection().exec_command(ChannelRequest::Exec {
            argv: vec!["sh".into(), "-c".into(),
                r#"PATH="$HOME/.local/bin:$HOME/.nix-profile/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"; command -v herdr"#.into()],
            cwd: None,
            env: vec![],
            pty: None,
        })?;
        let executable = String::from_utf8(out.stdout)?.trim().to_owned();
        ensure!(
            out.status == 0 && executable.starts_with('/'),
            "Cannot locate updated Herdr executable"
        );
        self.call_with_timeout(
            "server.live_handoff",
            json!({"import_exe":executable,"expected_version":version,"expected_protocol":protocol}),
            Duration::from_secs(90),
        )?;
        Ok(())
    }
    fn wait_snapshot(&self, version: &str) -> Result<SessionSnapshot> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(snap) = self.snapshot() {
                // 新 server 刚起来时可能先报出版本、workspace 还是空的。
                // 空列表不能当成交接完成，否则上层会把 space 关掉。
                if snap.version == version && !snap.workspaces.is_empty() {
                    return Ok(snap);
                }
            }
            if Instant::now() >= deadline {
                bail!("Updated server {version} did not become ready; installation may have completed. Retry reconnecting.");
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    struct Fake {
        calls: RefCell<Vec<&'static str>>,
        supported: bool,
        install_fails: bool,
        same_version: bool,
        handoff_reply_lost: bool,
        protocol: u32,
        ping_protocol: u32,
        empty_workspaces: bool,
    }
    impl UpdateBackend for Fake {
        fn ping(&self) -> Result<Value> {
            self.calls.borrow_mut().push("ping");
            Ok(json!({
                "version": "0.8.0",
                "protocol": self.ping_protocol,
                "capabilities": {"live_handoff": self.supported}
            }))
        }
        fn install(&self) -> Result<()> {
            self.calls.borrow_mut().push("install");
            if self.install_fails {
                bail!("package manager update required")
            }
            Ok(())
        }
        fn installed_version(&self) -> Result<String> {
            self.calls.borrow_mut().push("version");
            Ok(if self.same_version { "0.8.0" } else { "0.9.1" }.into())
        }
        fn installed_protocol(&self) -> Result<u32> {
            self.calls.borrow_mut().push("protocol");
            Ok(self.protocol)
        }
        fn handoff(&self, _: &str, _: u32) -> Result<()> {
            self.calls.borrow_mut().push("handoff");
            if self.handoff_reply_lost {
                bail!("connection closed while handing off")
            }
            Ok(())
        }
        fn wait_snapshot(&self, _: &str) -> Result<SessionSnapshot> {
            self.calls.borrow_mut().push("snapshot");
            let workspaces = if self.empty_workspaces {
                json!([])
            } else {
                json!([{"workspace_id":"w1","label":"kept"}])
            };
            SessionSnapshot::from_json(&json!({
                "version": if self.same_version { "0.8.0" } else { "0.9.1" },
                "protocol": self.protocol,
                "workspaces": workspaces,
                "tabs": [],
                "panes": []
            }))
        }
    }
    fn fake() -> Fake {
        Fake {
            calls: RefCell::new(vec![]),
            supported: true,
            install_fails: false,
            same_version: false,
            handoff_reply_lost: false,
            protocol: 22,
            ping_protocol: 22,
            empty_workspaces: false,
        }
    }
    #[test]
    fn update_handoffs_then_reads_authoritative_snapshot() {
        let f = fake();
        let finish = update(&f, |_| {}).unwrap();
        match finish {
            UpdateFinish::Updated {
                version,
                previous,
                snapshot,
            } => {
                assert_eq!(version, "0.9.1");
                assert_eq!(previous, "0.8.0");
                assert!(!snapshot.workspaces.is_empty());
            }
            other => panic!("old server must hand off, got {other:?}"),
        }
        assert_eq!(
            *f.calls.borrow(),
            ["ping", "install", "version", "protocol", "handoff", "snapshot"]
        );
    }
    #[test]
    fn old_running_protocol_hands_off_even_when_the_version_string_matches() {
        let f = Fake {
            same_version: true,
            ping_protocol: 19,
            protocol: 22,
            ..fake()
        };
        let finish = update(&f, |_| {}).unwrap();
        assert!(
            matches!(
                &finish,
                UpdateFinish::Updated { version, previous, .. }
                    if version == "0.8.0" && previous == "0.8.0"
            ),
            "{finish:?}"
        );
        assert_eq!(
            *f.calls.borrow(),
            ["ping", "install", "version", "protocol", "handoff", "snapshot"]
        );
    }
    #[test]
    fn empty_handoff_snapshot_does_not_count_as_an_update() {
        let f = Fake {
            empty_workspaces: true,
            ..fake()
        };
        let error = update(&f, |_| {}).unwrap_err().to_string();
        assert!(error.contains("no workspaces"), "{error}");
        assert_eq!(
            *f.calls.borrow(),
            ["ping", "install", "version", "protocol", "handoff", "snapshot"]
        );
    }
    #[test]
    fn unsupported_server_is_never_installed_or_stopped() {
        let f = Fake {
            supported: false,
            ..fake()
        };
        assert!(update(&f, |_| {}).is_err());
        assert_eq!(*f.calls.borrow(), ["ping"]);
    }
    #[test]
    fn failed_install_leaves_existing_server_alone() {
        let f = Fake {
            install_fails: true,
            ..fake()
        };
        assert!(update(&f, |_| {}).is_err());
        assert_eq!(*f.calls.borrow(), ["ping", "install"]);
    }
    #[test]
    fn current_server_does_not_restart_the_workspace() {
        let f = Fake {
            same_version: true,
            ..fake()
        };
        let finish = update(&f, |_| {}).unwrap();
        assert!(matches!(finish, UpdateFinish::AlreadyCurrent(version) if version == "0.8.0"));
        assert_eq!(
            *f.calls.borrow(),
            ["ping", "install", "version", "protocol"]
        );
    }
    #[test]
    fn already_current_does_not_publish_a_reconnect_snapshot() {
        let (tx, rx) = mpsc::channel();
        let mut state = UpdateState {
            receiver: Some(rx),
            ..UpdateState::default()
        };
        tx.send(UpdateEvent::Finished(Ok(UpdateFinish::AlreadyCurrent(
            "0.8.0".into(),
        ))))
        .unwrap();
        state.poll();
        assert_eq!(state.revision, 0);
        assert!(state.snapshot.is_none());
        assert_eq!(state.status.phase, "succeeded");
        assert_eq!(state.status.message, "Runtime 0.8.0 is already current");
        assert!(!state.status.busy());
    }
    #[test]
    fn version_change_publishes_one_reconnect_snapshot() {
        let (tx, rx) = mpsc::channel();
        let mut state = UpdateState {
            receiver: Some(rx),
            ..UpdateState::default()
        };
        let snapshot = SessionSnapshot::from_json(&json!({
            "version": "0.9.1",
            "protocol": 22,
            "workspaces": [],
            "tabs": [],
            "panes": []
        }))
        .unwrap();
        tx.send(UpdateEvent::Finished(Ok(UpdateFinish::Updated {
            version: "0.9.1".into(),
            previous: "0.8.0".into(),
            snapshot: Box::new(snapshot),
        })))
        .unwrap();
        state.poll();
        assert_eq!(state.revision, 1);
        assert!(state.snapshot.is_some());
        assert_eq!(state.status.phase, "updated");
        assert_eq!(state.status.version.as_deref(), Some("0.9.1"));
        assert_eq!(
            state.status.message,
            "Runtime 0.8.0 handed off to 0.9.1; pane processes kept"
        );
    }
    #[test]
    fn successful_handoff_with_lost_reply_uses_snapshot_to_confirm_completion() {
        let f = Fake {
            handoff_reply_lost: true,
            ..fake()
        };
        assert!(update(&f, |_| {}).is_ok());
        assert_eq!(f.calls.borrow().last(), Some(&"snapshot"));
    }
    #[test]
    fn unsupported_installed_protocol_keeps_running_server_and_streams() {
        let f = Fake {
            protocol: 23,
            ..fake()
        };
        let error = update(&f, |_| {}).unwrap_err().to_string();
        assert!(error.contains("protocol 23"), "{error}");
        assert_eq!(
            *f.calls.borrow(),
            ["ping", "install", "version", "protocol"]
        );
    }
}

#[cfg(test)]
mod capability_tests {
    use crate::runtime::{
        herdr::{HerdrRuntime, HerdrSession},
        Runtime, RuntimeCapability,
    };
    use std::sync::Arc;
    #[test]
    fn workspace_offers_update_without_connecting_or_installing() {
        let runtime = HerdrRuntime::new(
            Arc::new(HerdrSession::new("muxterm-test-update", "/unused")),
            "w1",
        );
        assert!(runtime
            .support()
            .contains(&RuntimeCapability::RuntimeUpdate));
        assert_eq!(runtime.update_status().unwrap().phase, "idle");
    }

    #[test]
    fn newly_opened_workspace_does_not_replay_an_old_update_snapshot() {
        let session = Arc::new(HerdrSession::new("muxterm-test-update", "/unused"));
        {
            let mut state = session.update.lock().unwrap();
            state.revision = 1;
            state.status = super::RuntimeUpdateStatus::new("updated", "Server updated");
            state.snapshot = Some(
                super::SessionSnapshot::from_json(
                    &serde_json::json!({"protocol":19,"workspaces":[],"tabs":[],"panes":[]}),
                )
                .unwrap(),
            );
        }
        let runtime = HerdrRuntime::new(session, "w2");
        assert_ne!(
            runtime.update_status().unwrap().phase,
            "reconnecting",
            "new connect reads fresh authority, never a stale update snapshot"
        );
    }
}
