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

/// 检查 herdr 二进制是否可用。
pub fn herdr_available() -> bool {
    Command::new("herdr")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
        let name = unique_name(label);
        let base = std::env::var("HERDR_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/home/wlz".into());
                PathBuf::from(home).join(".config/herdr")
            });
        let session_dir = base.join("sessions").join(&name);
        let socket_path = session_dir.join("herdr.sock");
        let client_socket_path = session_dir.join("herdr-client.sock");

        // setsid：server 脱离测试进程的进程组，stdin/stdout/stderr 全丢。
        let child = Command::new("setsid")
            .args(["herdr", "--session", &name, "server"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn herdr server 失败");

        let deadline = Instant::now() + Duration::from_secs(5);
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

    /// `pane send-text` + `pane send-keys enter` 涂 token。
    pub fn paint(&self, pane_id: &str, token: &str) {
        let out = self
            .cli()
            .args(["pane", "send-text", pane_id, token])
            .output()
            .expect("pane send-text 失败");
        assert!(
            out.status.success(),
            "pane send-text 失败: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let out = self
            .cli()
            .args(["pane", "send-keys", pane_id, "enter"])
            .output()
            .expect("pane send-keys 失败");
        assert!(
            out.status.success(),
            "pane send-keys 失败: {}",
            String::from_utf8_lossy(&out.stderr)
        );
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
        let _ = self.cli().args(["session", "stop", &self.name]).output();
        let _ = self.cli().args(["session", "delete", &self.name]).output();
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
