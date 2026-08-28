#![allow(dead_code)]
//! Herdr 测试支持：独立 named session（`muxterm-test-*`），自动清理。
//!
//! **纪律**：
//! - 每条 CLI 必须带 `--session <name>`（本环境常有 `HERDR_ENV=1`，不带会打到用户默认 session）。
//! - 清理只许 `session stop` + `session delete`，**禁止** `herdr server stop`。
//! - socket 路径等于用户默认 `herdr.sock` 时拒绝清理。

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use muxterm::core::runtime::herdr::observe::{channel, ObserveStream, StreamMode};
use muxterm::core::runtime::herdr::session::HerdrSession;
use muxterm::core::types::PaneId;
use serde_json::Value;

/// Remote runners can spend several seconds creating the repeated split/close
/// fixture before a newly created pane's shell has a visible screen. Keep this
/// bound finite, but allow a loaded CI runner's split-child startup tail.
const HERDR_FIXTURE_TIMEOUT: Duration = Duration::from_secs(30);

fn pane_process_ready(session: &HerdrSession, pane_id: &str) -> bool {
    let Ok(result) = session.call(
        "pane.process_info",
        serde_json::json!({ "pane_id": pane_id }),
    ) else {
        return false;
    };
    let Some(info) = result.get("process_info") else {
        return false;
    };
    let shell_pid = info.get("shell_pid").and_then(Value::as_u64).unwrap_or(0);
    let foreground_count = info
        .get("foreground_processes")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    shell_pid > 0 && foreground_count > 0
}

/// 检查 herdr 二进制是否可用。
pub fn herdr_available() -> bool {
    Command::new("herdr")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// §13.1：测试开头校验 Herdr 版本 == 0.8.0（protocol 19）。
/// 版本不符是 **required failure**，不是 skip。
pub fn assert_herdr_version_ok() {
    let version = Command::new("herdr")
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    assert!(
        version.contains("0.8.0"),
        "herdr 版本必须为 0.8.0（protocol 19），实际: {version}"
    );
}

/// 唯一 named session 名：`muxterm-test-herdr-{label}-{nanos}`。
pub fn unique_name(label: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("muxterm-test-herdr-{label}-{nanos}")
}

/// 用户默认 herdr socket（测试永远不许连 / 清理它）。
pub fn default_socket_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/wlz".into());
    PathBuf::from(home).join(".config/herdr/herdr.sock")
}

/// 在系统临时目录创建一个可被 Herdr 真实进程检测识别的短命 agent。
/// Drop 只删除自己创建的 `muxterm-test-herdr-agent-*` 目录。
pub struct TempAgentCommand {
    dir: PathBuf,
    invocation: String,
    done_file: PathBuf,
    stop_file: PathBuf,
}

impl TempAgentCommand {
    pub fn pi(label: &str) -> Self {
        let dir = std::env::temp_dir().join(unique_name(&format!("agent-{label}")));
        std::fs::create_dir_all(&dir).expect("创建临时 agent 目录失败");
        let command = dir.join("pi");
        let done_file = dir.join("agent-done");
        let stop_file = dir.join("agent-stop");
        std::fs::write(
            &command,
            format!(
                "#!/bin/sh\nprintf 'Working...\\n'\nwhile [ ! -f '{}' ]; do sleep 0.05; done\nprintf '\\033[2J\\033[Hdone\\n'\nwhile [ ! -f '{}' ]; do sleep 0.05; done\n",
                done_file.display(),
                stop_file.display(),
            ),
        )
        .expect("写临时 pi agent 失败");
        #[cfg(unix)]
        {
            let mut permissions = std::fs::metadata(&command)
                .expect("读取临时 pi agent 权限失败")
                .permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(&command, permissions).expect("设置临时 pi agent 权限失败");
        }
        Self {
            dir,
            invocation: "./pi".into(),
            done_file,
            stop_file,
        }
    }

    pub fn cwd(&self) -> &Path {
        &self.dir
    }

    pub fn invocation(&self) -> &str {
        &self.invocation
    }

    /// 让真实 pi 进程画出 Herdr 的 Done 识别帧，但保持进程存活。
    pub fn mark_done(&self) {
        std::fs::write(&self.done_file, "done\n").expect("触发临时 pi done 失败");
    }

    /// 让临时 pi 正常退出。
    pub fn stop(&self) {
        std::fs::write(&self.stop_file, "stop\n").expect("停止临时 pi 失败");
    }
}

impl Drop for TempAgentCommand {
    fn drop(&mut self) {
        let temp_dir = std::env::temp_dir();
        let safe = self.dir.parent() == Some(temp_dir.as_path())
            && self
                .dir
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("muxterm-test-herdr-agent-"));
        if safe {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// 独立 Herdr named session 夹具。
pub struct IsolatedHerdr {
    name: String,
    socket_path: PathBuf,
    client_socket_path: PathBuf,
    child: Option<Child>,
}

impl IsolatedHerdr {
    /// 启动 `herdr --session NAME server` 并等 socket 出现（~5s）。
    pub fn start(label: &str) -> Self {
        Self::start_named(unique_name(label))
    }

    /// 用预分配的精确名称启动（W7 parent 兜底清理用：名称可预测）。
    pub fn start_named(name: String) -> Self {
        // §13.1：任何 herdr 测试开始前校验 protocol 19（版本不符 = required failure）。
        assert_herdr_version_ok();
        if !name.starts_with("muxterm-test-") {
            panic!("herdr fixture 名称必须以 muxterm-test- 开头: {name}");
        }
        // herdr server 固定用 ~/.config/herdr（不认 HERDR_CONFIG_DIR），
        // 夹具必须按真实位置等 socket。
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/wlz".into());
        let base = PathBuf::from(home).join(".config/herdr");
        let session_dir = base.join("sessions").join(&name);
        let socket_path = session_dir.join("herdr.sock");
        let client_socket_path = session_dir.join("herdr-client.sock");

        // Linux 用 setsid 脱离测试进程组；macOS 没有 setsid 可执行文件，
        // 直接持有 named-session server 子进程并在 Drop 精确 stop/delete。
        #[cfg(target_os = "linux")]
        let mut server = {
            let mut command = Command::new("setsid");
            command.args(["herdr", "--session", &name, "server"]);
            command
        };
        #[cfg(not(target_os = "linux"))]
        let mut server = {
            let mut command = Command::new("herdr");
            command.args(["--session", &name, "server"]);
            command
        };
        let child = server
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn herdr server 失败");

        let deadline = Instant::now() + HERDR_FIXTURE_TIMEOUT;
        while Instant::now() < deadline {
            if socket_path.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            socket_path.exists(),
            "herdr server socket 未在 5s 内出现: {}",
            socket_path.display()
        );

        Self {
            name,
            socket_path,
            client_socket_path,
            child: Some(child),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn client_socket_path(&self) -> &Path {
        &self.client_socket_path
    }

    /// 永远带 `--session self.name` 的 CLI。
    pub fn cli(&self) -> Command {
        let mut c = Command::new("herdr");
        c.args(["--session", &self.name]);
        c
    }

    /// `workspace create --cwd <dir> --label <label>`，解析 JSON 返回
    /// `(workspace_id, tab_id, pane_id)`。兼容 0.8.0 的
    /// `result.workspace.workspace_id` / `result.tab.tab_id` / `result.root_pane.pane_id`。
    pub fn create_workspace(&self, cwd: &str, label: &str) -> (String, String, String) {
        let out = self
            .cli()
            .args(["workspace", "create", "--cwd", cwd, "--label", label])
            .output()
            .expect("workspace create 失败");
        assert!(
            out.status.success(),
            "workspace create 失败: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v: serde_json::Value =
            serde_json::from_slice(&out.stdout).expect("workspace create 输出不是 JSON");
        let result = &v["result"];
        let ws = result["workspace"]["workspace_id"]
            .as_str()
            .or_else(|| result["workspace_id"].as_str())
            .expect("缺 workspace_id");
        let tab = result["tab"]["tab_id"]
            .as_str()
            .or_else(|| result["tab_id"].as_str())
            .expect("缺 tab_id");
        let pane = result["root_pane"]["pane_id"]
            .as_str()
            .or_else(|| result["pane_id"].as_str())
            .expect("缺 pane_id");
        (ws.to_string(), tab.to_string(), pane.to_string())
    }

    /// 用一条原始 `pane send-text`（含 CR）涂 token。
    ///
    /// 把文本和 Enter 拆成两个 API 请求会在慢 runner 的新 shell 启动窗口
    /// 中产生额外的跨请求排序点；单条 raw payload 保持产品 WriteRaw 的
    /// 字节语义，也让 fixture readiness 只观察一个有序写入。
    pub fn paint(&self, pane_id: &str, token: &str) {
        let payload = format!("{token}\r");
        let out = self
            .cli()
            .args(["pane", "send-text", pane_id, &payload])
            .output()
            .expect("pane send-text 失败");
        assert!(
            out.status.success(),
            "pane send-text 失败: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// 用 Herdr 的原子 `pane run`（命令 + Enter）在夹具中产生标记。
    ///
    /// 预置 attach 内容不是产品逐字输入测试；使用原子 API 避免新 shell
    /// 刚进入交互态时 `pane.send-text` 的跨请求/行规程竞态。产品 raw 输入
    /// 仍由 `herdr_local_write_raw_executes_echo_command` 等契约覆盖。
    /// `printf` 把 token 写成命令输出，避免把 token 当可执行文件名。
    fn run_marker(&self, pane_id: &str, token: &str) {
        let out = self
            .cli()
            .args(["pane", "run", pane_id, "printf", "%s\n", token])
            .output()
            .expect("pane run 失败");
        assert!(
            out.status.success(),
            "pane run 失败: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// 挂一个有尺寸的 Control client，让 `pane.read(visible)` 在没有 GUI
    /// client 的 CI runner 上仍有非空 viewport。Hello 的 cols/rows 是 session
    /// 级 client 尺寸；本地若已有默认 viewport 也无害。
    fn attach_sized_client(&self, pane_id: &str, cols: u16, rows: u16) -> ObserveStream {
        let (tx, _rx) = channel();
        let stream = ObserveStream::start(
            self.client_socket_path(),
            pane_id,
            PaneId(1),
            1,
            StreamMode::Control,
            false,
            cols,
            rows,
            tx,
        )
        .unwrap_or_else(|error| panic!("启动 Herdr sizing client 失败: {error:#}"));
        let session = HerdrSession::new(self.name(), self.socket_path());
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if session
                .pane_read_ansi(pane_id)
                .ok()
                .is_some_and(|bytes| !bytes.is_empty())
            {
                return stream;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        stream
    }

    fn snapshot_preview(session: &HerdrSession, pane_id: &str, source: &str) -> String {
        let bytes = if source == "recent" {
            session.pane_read_recent_ansi_lines(pane_id, 2000)
        } else {
            session.pane_read_ansi(pane_id)
        };
        match bytes {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                text.chars()
                    .rev()
                    .take(800)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<String>()
                    .escape_debug()
                    .to_string()
            }
            Err(error) => format!("error: {error:#}"),
        }
    }

    /// 等待 Herdr 为新 pane 建好真实 shell 进程。
    ///
    /// `pane.split` 的响应只代表拓扑已经变更；pty/shell 是随后异步启动的。
    /// 不能用一次 `pane.send-text` 的回显来推断 ready：慢 runner 可能在 shell
    /// 启动窗口丢掉写入，之后再重放也不一定能恢复。process-info 是 Herdr
    /// 服务端对进程生命周期的权威观察面，和 attach 后 Runtime 使用的真实
    /// pane 一致。
    pub fn wait_for_pane_process(&self, pane_id: &str) {
        let session = HerdrSession::new(self.name(), self.socket_path());
        let deadline = Instant::now() + HERDR_FIXTURE_TIMEOUT;
        while Instant::now() < deadline {
            if pane_process_ready(&session, pane_id) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let info = session
            .call(
                "pane.process_info",
                serde_json::json!({ "pane_id": pane_id }),
            )
            .map(|value| value.to_string())
            .unwrap_or_else(|error| format!("error: {error:#}"));
        let recent = session
            .pane_read_recent_ansi_lines(pane_id, 2000)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_else(|error| format!("error: {error:#}"));
        panic!("Herdr pane shell 未就绪: pane={pane_id} process_info={info} recent={recent:?}");
    }

    /// 重试运行并等待服务端 recent snapshot 看到 token，再把它作为 attach
    /// 前置条件。新建 pane 的 shell 可能尚未 ready；重试只解决 fixture
    /// readiness，不改变 attach 后 Runtime 的快照断言。
    pub fn paint_until_token(&self, pane_id: &str, token: &str) {
        let session = HerdrSession::new(self.name(), self.socket_path());
        let deadline = Instant::now() + HERDR_FIXTURE_TIMEOUT;
        let mut next_send = Instant::now();
        while Instant::now() < deadline {
            if pane_process_ready(&session, pane_id) && Instant::now() >= next_send {
                self.run_marker(pane_id, token);
                next_send = Instant::now() + Duration::from_millis(250);
            }
            if let Ok(bytes) = session.pane_read_recent_ansi_lines(pane_id, 2000) {
                if String::from_utf8_lossy(&bytes).contains(token) {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let observed = session
            .pane_read_recent_ansi_lines(pane_id, 2000)
            .map(|bytes| String::from_utf8_lossy(&bytes).contains(token))
            .unwrap_or(false);
        let process_info = session
            .call(
                "pane.process_info",
                serde_json::json!({ "pane_id": pane_id }),
            )
            .map(|value| value.to_string())
            .unwrap_or_else(|error| format!("error: {error:#}"));
        panic!(
            "Herdr pane token 未在 attach 前落到服务端 snapshot: pane={pane_id} token={token} observed={observed} process_info={process_info}"
        );
    }

    /// 重试运行并等待服务端 visible snapshot 看到 token。
    ///
    /// GTK/VTE attach 断言的是当前可见终端，而不是 scrollback；使用 recent
    /// 会把已经滚出屏幕的 token 误当作可渲染的前置条件。CI runner 没有
    /// 预先连接的 Herdr GUI client 时 visible viewport 可能为空，所以先
    /// 挂一个有尺寸的 Control client（函数返回后 drop，GTK 再 attach）。
    pub fn paint_until_visible_token(&self, pane_id: &str, token: &str) {
        let session = HerdrSession::new(self.name(), self.socket_path());
        let _client = self.attach_sized_client(pane_id, 120, 40);
        let deadline = Instant::now() + HERDR_FIXTURE_TIMEOUT;
        let mut next_send = Instant::now();
        while Instant::now() < deadline {
            if pane_process_ready(&session, pane_id) && Instant::now() >= next_send {
                self.run_marker(pane_id, token);
                next_send = Instant::now() + Duration::from_millis(250);
            }
            if let Ok(bytes) = session.pane_read_ansi(pane_id) {
                if String::from_utf8_lossy(&bytes).contains(token) {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let observed = session
            .pane_read_ansi(pane_id)
            .map(|bytes| String::from_utf8_lossy(&bytes).contains(token))
            .unwrap_or(false);
        let process_info = session
            .call(
                "pane.process_info",
                serde_json::json!({ "pane_id": pane_id }),
            )
            .map(|value| value.to_string())
            .unwrap_or_else(|error| format!("error: {error:#}"));
        panic!(
            "Herdr pane token 未在 visible snapshot 落到前置条件: pane={pane_id} token={token} observed={observed} process_info={process_info} visible={} recent={}",
            Self::snapshot_preview(&session, pane_id, "visible"),
            Self::snapshot_preview(&session, pane_id, "recent"),
        );
    }

    /// 在指定 pane 上真实执行 `pane split`，返回新 pane 的 public id。
    pub fn split_pane(&self, pane_id: &str, direction: &str) -> String {
        let out = self
            .cli()
            .args([
                "pane",
                "split",
                pane_id,
                "--direction",
                direction,
                "--no-focus",
            ])
            .output()
            .expect("pane split 失败");
        assert!(
            out.status.success(),
            "pane split 失败: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v: serde_json::Value =
            serde_json::from_slice(&out.stdout).expect("pane split 输出不是 JSON");
        v["result"]["pane"]["pane_id"]
            .as_str()
            .expect("pane split 缺 result.pane.pane_id")
            .to_string()
    }

    /// 关闭隔离 session 里的指定 pane（只供 public-id 夹具推进计数）。
    pub fn close_pane(&self, pane_id: &str) {
        let out = self
            .cli()
            .args(["pane", "close", pane_id])
            .output()
            .expect("pane close 失败");
        assert!(
            out.status.success(),
            "pane close 失败: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// 在同一 workspace 内把 public pane id 推进到 pP，再保留 pP/pQ/pR
    /// 三个真实 pane。对应用户日志里出现字母后缀后的布局场景。
    pub fn create_alpha_split_workspace(
        &self,
        cwd: &str,
        label: &str,
    ) -> (String, String, [String; 3]) {
        let (workspace, tab, mut current) = self.create_workspace(cwd, label);
        // p1 -> ... -> pP：每轮 split 后关闭旧 pane，保持布局浅且计数继续。
        for _ in 1..22 {
            let next = self.split_pane(&current, "right");
            // `pane.split` returns after the layout mutation, not after the
            // child shell has attached its PTY.  On a loaded CI runner,
            // closing the parent immediately can leave the new pane unable to
            // receive the first test command.  Wait for the server's process
            // lifecycle signal before retiring the old pane; command output is
            // intentionally reserved for the final attach fixture below.
            self.wait_for_pane_process(&next);
            self.close_pane(&current);
            current = next;
        }
        let pane_p = current;
        let pane_q = self.split_pane(&pane_p, "right");
        let pane_r = self.split_pane(&pane_p, "down");
        assert!(pane_p.ends_with(":pP"), "应推进到 pP，实际 {pane_p}");
        assert!(
            pane_q.ends_with(":pQ"),
            "第一个保留 split 应是 pQ，实际 {pane_q}"
        );
        assert!(
            pane_r.ends_with(":pR"),
            "第二个保留 split 应是 pR，实际 {pane_r}"
        );
        (workspace, tab, [pane_p, pane_q, pane_r])
    }
}

impl Drop for IsolatedHerdr {
    fn drop(&mut self) {
        // 只清理自己的 muxterm-test-* session；绝不碰用户默认 herdr.sock。
        if !self.name.starts_with("muxterm-test-") {
            return;
        }
        if self.socket_path == default_socket_path() {
            return;
        }
        let _ = Command::new("herdr")
            .args(["session", "stop", &self.name])
            .output();
        let _ = Command::new("herdr")
            .args(["session", "delete", &self.name])
            .output();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// 临时 git 仓库（H4 worktree 夹具）：路径只许 `/tmp/muxterm-test-herdr-*`。
///
/// Drop：先 `git worktree remove` 已建 linked worktree，再删整个临时目录。
pub struct TempGitRepo {
    path: PathBuf,
    linked_worktrees: Vec<PathBuf>,
}

impl TempGitRepo {
    /// `git init` + 一次 empty commit。
    pub fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("muxterm-test-herdr-{label}-{nanos}"));
        std::fs::create_dir_all(&path).expect("创建临时 git 仓库失败");
        let ok = |args: &[&str]| {
            let out = Command::new("git")
                .args(["-C", path.to_str().unwrap()])
                .args(args)
                .output()
                .expect("git 命令失败");
            assert!(
                out.status.success(),
                "git {args:?} 失败: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        ok(&["init", "-q"]);
        ok(&["config", "user.email", "muxterm-test@example.com"]);
        ok(&["config", "user.name", "muxterm-test"]);
        ok(&["commit", "--allow-empty", "-qm", "init"]);
        Self {
            path,
            linked_worktrees: Vec::new(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 唯一分支名：`muxterm-test-wt-{label}-{nanos}`。
    pub fn unique_branch(&self, label: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        format!("muxterm-test-wt-{label}-{nanos}")
    }

    /// 唯一 linked worktree 路径：`/tmp/muxterm-test-herdr-wt-{label}-{nanos}`。
    pub fn unique_worktree_path(&self, label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("muxterm-test-herdr-wt-{label}-{nanos}"))
    }

    /// 记录 herdr 建出的 linked worktree（Drop 时先 remove 再删目录）。
    pub fn track_worktree(&mut self, path: impl Into<PathBuf>) {
        self.linked_worktrees.push(path.into());
    }
}

impl Drop for TempGitRepo {
    fn drop(&mut self) {
        for wt in &self.linked_worktrees {
            let _ = Command::new("git")
                .args([
                    "-C",
                    self.path.to_str().unwrap(),
                    "worktree",
                    "remove",
                    "--force",
                ])
                .arg(wt)
                .output();
        }
        let _ = std::fs::remove_dir_all(&self.path);
        for wt in &self.linked_worktrees {
            let _ = std::fs::remove_dir_all(wt);
        }
    }
}
