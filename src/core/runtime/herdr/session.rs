//! HerdrSession：一条 Herdr named session 的 socket 连接身份。
//!
//! - API socket（`herdr.sock`）：JSON `{"id","method","params"}` 请求/响应
//!   （ping / session.snapshot / pane.read / pane.send_text / worktree.*）。
//! - client socket（`herdr-client.sock`）：observe 流（H2 起用）。
//!
//! 一个 `HerdrSession` 可被多个 `HerdrRuntime` 共享（`Arc`），
//! 不要每个 Workspace 再开一条 API socket。

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

/// 单次请求超时。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// 一条 Herdr named session 的连接身份（不是产品 Session 类型）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrSession {
    name: String,
    socket_path: PathBuf,
    client_socket_path: PathBuf,
}

/// 进程内共享的 HerdrSession 缓存（同一 named session + socket 一份 Arc）。
///
/// 旧 WorkspacePool.herdr_sessions 旁路表迁到这里：Catalog 的 Connect /
/// Driver.open 与 WorkspaceSpec::build_runtime 都从这里拿，语义相同、位置不同。
/// 共享 session 缓存类型：(named session, socket) → Arc。
type SharedSessionMap = std::collections::HashMap<(String, String), Arc<HerdrSession>>;

static SHARED_SESSIONS: std::sync::LazyLock<std::sync::Mutex<SharedSessionMap>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

impl HerdrSession {
    /// 取（或建）共享 session：同一 `(name, socket)` 返回同一 `Arc`。
    pub fn shared(name: impl Into<String>, socket_path: impl Into<PathBuf>) -> Arc<Self> {
        let name = name.into();
        let socket = socket_path.into().to_string_lossy().to_string();
        let key = (name.clone(), socket.clone());
        if let Ok(mut cache) = SHARED_SESSIONS.lock() {
            if let Some(existing) = cache.get(&key) {
                return Arc::clone(existing);
            }
            let session = Arc::new(Self::new(name, socket));
            cache.insert(key, Arc::clone(&session));
            return session;
        }
        Arc::new(Self::new(name, socket))
    }

    /// 绑定 named session 名 + API socket 绝对路径。
    pub fn new(name: impl Into<String>, socket_path: impl Into<PathBuf>) -> Self {
        let socket_path = socket_path.into();
        let client_socket_path = client_socket_path_from_api(&socket_path);
        Self {
            name: name.into(),
            socket_path,
            client_socket_path,
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

    /// 发一条 JSON 请求并取 `result`（error 直接 bail）。
    ///
    /// 每次请求一条新连接（与 herdr CLI 每次调用同构）；响应是单行 JSON。
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        let mut stream = UnixStream::connect(&self.socket_path).with_context(|| {
            format!(
                "连接 Herdr socket 失败（session={} path={}）",
                self.name,
                self.socket_path.display()
            )
        })?;
        stream
            .set_read_timeout(Some(REQUEST_TIMEOUT))
            .context("设置 Herdr socket 读超时失败")?;
        let req = serde_json::json!({
            "id": format!("muxterm-{}", self.name),
            "method": method,
            "params": params,
        });
        stream
            .write_all((req.to_string() + "\n").as_bytes())
            .with_context(|| format!("写 Herdr 请求失败（{method}）"))?;
        stream.flush().ok();

        let mut line = String::new();
        let mut reader = BufReader::new(stream);
        reader
            .read_line(&mut line)
            .with_context(|| format!("读 Herdr 响应失败（{method}）"))?;
        let resp: Value = serde_json::from_str(&line)
            .with_context(|| format!("解析 Herdr 响应失败（{method}）: {line}"))?;
        if let Some(err) = resp.get("error") {
            bail!("Herdr {method} 失败: {err}");
        }
        resp.get("result")
            .cloned()
            .ok_or_else(|| anyhow!("Herdr {method} 响应缺 result: {resp}"))
    }

    /// `ping`：确认 server 活着且协议可对话。
    pub fn ping(&self) -> Result<()> {
        let result = self.call("ping", serde_json::json!({}))?;
        if result.get("type").and_then(Value::as_str) != Some("pong") {
            bail!("ping 响应不是 pong: {result}");
        }
        Ok(())
    }

    /// `session.snapshot`：一次性 bootstrap（workspace/tab/pane/layout）。
    pub fn snapshot(&self) -> Result<SessionSnapshot> {
        let result = self.call("session.snapshot", serde_json::json!({}))?;
        let snap = result
            .get("snapshot")
            .ok_or_else(|| anyhow!("session.snapshot 缺 snapshot: {result}"))?;
        SessionSnapshot::from_json(snap)
    }

    /// `pane.layout`：取得 pane 所在 tab 的权威布局快照。
    pub fn pane_layout(&self, pane_id: &str) -> Result<LayoutRecord> {
        let result = self.call("pane.layout", serde_json::json!({ "pane_id": pane_id }))?;
        let layout = result
            .get("layout")
            .ok_or_else(|| anyhow!("pane.layout 缺 layout: {result}"))?;
        LayoutRecord::from_json(layout).ok_or_else(|| anyhow!("pane.layout 布局解析失败: {result}"))
    }

    /// `pane.read`：attach 快照（source=visible, format=ansi），返回原始 ANSI 字节。
    pub fn pane_read_ansi(&self, pane_id: &str) -> Result<Vec<u8>> {
        self.pane_read_with_source(pane_id, "visible")
    }

    /// `pane.read`：最近 80 行（source=recent, format=ansi）。
    ///
    /// 可见屏幕之外的 scrollback 尾部也包含在内；测试断言「服务端仍持有
    /// token」时用它，避免 token 滚出可见区后误报丢失。
    pub fn pane_read_recent_ansi(&self, pane_id: &str) -> Result<Vec<u8>> {
        self.pane_read_recent_ansi_lines(pane_id, 80)
    }

    /// `pane.read`：最近 N 行（source=recent, format=ansi）。
    ///
    /// CI 的 pane 行数可能比本地多（字体/窗口差异），早期 token 会滚出
    /// 默认 80 行；断言历史 token 时传大 N 覆盖整个 scrollback 尾部。
    pub fn pane_read_recent_ansi_lines(&self, pane_id: &str, lines: u32) -> Result<Vec<u8>> {
        let result = self.call(
            "pane.read",
            serde_json::json!({
                "pane_id": pane_id,
                "source": "recent",
                "lines": lines,
                "format": "ansi",
                "strip_ansi": false,
            }),
        )?;
        let text = result
            .get("read")
            .and_then(|r| r.get("text"))
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("pane.read 缺 text: {result}"))?;
        Ok(text.as_bytes().to_vec())
    }

    fn pane_read_with_source(&self, pane_id: &str, source: &str) -> Result<Vec<u8>> {
        let result = self.call(
            "pane.read",
            serde_json::json!({
                "pane_id": pane_id,
                "source": source,
                "format": "ansi",
                "strip_ansi": false,
            }),
        )?;
        let text = result
            .get("read")
            .and_then(|r| r.get("text"))
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("pane.read 缺 text: {result}"))?;
        Ok(text.as_bytes().to_vec())
    }

    /// `pane.send_text`：原样写终端字节对应的文本（键盘 commit / WriteRaw）。
    ///
    /// 不能用 `pane.send_input` 代替：后者会在终端启用 bracketed-paste 时
    /// 自动包 `ESC[200~...ESC[201~`，逐键调用会让 Enter 变成粘贴换行而不执行。
    pub fn pane_send_text(&self, pane_id: &str, text: &str) -> Result<()> {
        self.call(
            "pane.send_text",
            serde_json::json!({ "pane_id": pane_id, "text": text }),
        )?;
        Ok(())
    }

    /// `pane.send_keys`：发 herdr key-combo 字符串（enter / ctrl+c / f1 …）。
    pub fn pane_send_keys(&self, pane_id: &str, keys: &[String]) -> Result<()> {
        self.call(
            "pane.send_keys",
            serde_json::json!({ "pane_id": pane_id, "keys": keys }),
        )?;
        Ok(())
    }

    /// `workspace.list`：全部 Herdr workspace 记录。
    pub fn workspace_list(&self) -> Result<Vec<WorkspaceRecord>> {
        let result = self.call("workspace.list", serde_json::json!({}))?;
        Ok(result
            .get("workspaces")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(WorkspaceRecord::from_json).collect())
            .unwrap_or_default())
    }

    /// `workspace.create`：新建 Herdr workspace（New Project 用）。
    pub fn workspace_create(&self, cwd: &str, label: &str) -> Result<WorkspaceRecord> {
        let result = self.call(
            "workspace.create",
            serde_json::json!({ "cwd": cwd, "label": label, "focus": false }),
        )?;
        let ws = result
            .get("workspace")
            .ok_or_else(|| anyhow!("workspace.create 缺 workspace: {result}"))?;
        WorkspaceRecord::from_json(ws).ok_or_else(|| anyhow!("workspace.create 解析失败: {result}"))
    }

    /// `worktree.list`：当前仓库全部 checkout（需 WorktreeList）。
    pub fn worktree_list(&self, workspace_id: &str) -> Result<HerdrWorktreeList> {
        let result = self.call(
            "worktree.list",
            serde_json::json!({ "workspace_id": workspace_id }),
        )?;
        HerdrWorktreeList::from_json(&result)
    }

    /// `worktree.create`：git worktree add + 打开成新 Herdr workspace。
    pub fn worktree_create(
        &self,
        workspace_id: &str,
        branch: &str,
        path: &str,
        base: Option<&str>,
        label: Option<&str>,
    ) -> Result<HerdrWorktreeRecord> {
        let result = self.call(
            "worktree.create",
            serde_json::json!({
                "workspace_id": workspace_id,
                "branch": branch,
                "path": path,
                "base": base,
                "label": label,
                "focus": false,
            }),
        )?;
        HerdrWorktreeRecord::from_json(&result)
    }

    /// `worktree.open`：打开已有 checkout；已打开就返回那格。
    pub fn worktree_open(&self, workspace_id: &str, path: &str) -> Result<HerdrWorktreeRecord> {
        let result = self.call(
            "worktree.open",
            serde_json::json!({
                "workspace_id": workspace_id,
                "path": path,
                "focus": false,
            }),
        )?;
        HerdrWorktreeRecord::from_json(&result)
    }
}

/// Herdr 的 client socket 与 API socket 同目录，并在 stem 后插入 `-client`。
///
/// 标准路径 `herdr.sock` → `herdr-client.sock`；SSH 转发的唯一 API 路径
/// `muxterm-herdr-fwd-X.sock` → `muxterm-herdr-fwd-X-client.sock`。不能只取
/// parent 后硬编码 `herdr-client.sock`，否则所有 SSH 连接都会撞到 `/tmp`。
pub(crate) fn client_socket_path_from_api(api_socket_path: &Path) -> PathBuf {
    let stem = api_socket_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("herdr");
    let parent = api_socket_path.parent().unwrap_or_else(|| Path::new(""));
    parent.join(format!("{stem}-client.sock"))
}

/// `session.snapshot` 的产品视图（Herdr id 保持字符串，映射在 HerdrRuntime）。
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSnapshot {
    pub version: String,
    pub protocol: u64,
    pub focused_workspace_id: Option<String>,
    pub focused_tab_id: Option<String>,
    pub focused_pane_id: Option<String>,
    pub workspaces: Vec<WorkspaceRecord>,
    pub tabs: Vec<TabRecord>,
    pub panes: Vec<PaneRecord>,
    pub layouts: Vec<LayoutRecord>,
    pub agents: Vec<AgentRecord>,
}

impl SessionSnapshot {
    fn from_json(v: &Value) -> Result<Self> {
        let workspaces = v
            .get("workspaces")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(WorkspaceRecord::from_json).collect())
            .unwrap_or_default();
        let tabs = v
            .get("tabs")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(TabRecord::from_json).collect())
            .unwrap_or_default();
        let panes = v
            .get("panes")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(PaneRecord::from_json).collect())
            .unwrap_or_default();
        let layouts = v
            .get("layouts")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(LayoutRecord::from_json).collect())
            .unwrap_or_default();
        let agents = v
            .get("agents")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(AgentRecord::from_json).collect())
            .unwrap_or_default();
        Ok(Self {
            version: v
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            protocol: v
                .get("protocol")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            focused_workspace_id: v
                .get("focused_workspace_id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            focused_tab_id: v
                .get("focused_tab_id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            focused_pane_id: v
                .get("focused_pane_id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            workspaces,
            tabs,
            panes,
            layouts,
            agents,
        })
    }
}

/// Herdr workspace 记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRecord {
    pub workspace_id: String,
    pub number: usize,
    pub label: String,
    pub focused: bool,
    pub pane_count: usize,
    pub tab_count: usize,
    pub active_tab_id: Option<String>,
    pub agent_status: HerdrAgentStatus,
    pub tokens: BTreeMap<String, String>,
    pub worktree: Option<WorkspaceWorktreeRecord>,
}

impl WorkspaceRecord {
    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            workspace_id: v.get("workspace_id")?.as_str()?.to_string(),
            number: v.get("number").and_then(Value::as_u64).unwrap_or(0) as usize,
            label: v
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            focused: v.get("focused").and_then(Value::as_bool).unwrap_or(false),
            pane_count: v.get("pane_count").and_then(Value::as_u64).unwrap_or(0) as usize,
            tab_count: v.get("tab_count").and_then(Value::as_u64).unwrap_or(0) as usize,
            active_tab_id: v
                .get("active_tab_id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            agent_status: HerdrAgentStatus::from_value(v.get("agent_status")),
            tokens: string_map(v.get("tokens")),
            worktree: v
                .get("worktree")
                .and_then(WorkspaceWorktreeRecord::from_json),
        })
    }
}

/// Herdr workspace 上的 worktree provenance（不是产品树的新层级）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceWorktreeRecord {
    pub repo_key: String,
    pub repo_name: String,
    pub repo_root: String,
    pub checkout_path: String,
    pub linked: bool,
}

impl WorkspaceWorktreeRecord {
    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            repo_key: v.get("repo_key")?.as_str()?.to_string(),
            repo_name: v
                .get("repo_name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            repo_root: v
                .get("repo_root")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            checkout_path: v
                .get("checkout_path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            linked: v
                .get("is_linked_worktree")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }
}

/// Herdr tab 记录（`w1:t1`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabRecord {
    pub tab_id: String,
    pub workspace_id: String,
    pub number: usize,
    pub label: String,
    pub focused: bool,
    pub pane_count: usize,
    pub agent_status: HerdrAgentStatus,
}

impl TabRecord {
    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            tab_id: v.get("tab_id")?.as_str()?.to_string(),
            workspace_id: v.get("workspace_id")?.as_str()?.to_string(),
            number: v.get("number").and_then(Value::as_u64).unwrap_or(0) as usize,
            label: v
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            focused: v.get("focused").and_then(Value::as_bool).unwrap_or(false),
            pane_count: v.get("pane_count").and_then(Value::as_u64).unwrap_or(0) as usize,
            agent_status: HerdrAgentStatus::from_value(v.get("agent_status")),
        })
    }
}

/// Herdr pane 记录（`w1:p1`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneRecord {
    pub pane_id: String,
    pub terminal_id: Option<String>,
    pub workspace_id: String,
    pub tab_id: String,
    pub focused: bool,
    pub cwd: Option<String>,
    pub foreground_cwd: Option<String>,
    pub label: Option<String>,
    pub agent: Option<String>,
    pub title: Option<String>,
    pub terminal_title: Option<String>,
    pub terminal_title_stripped: Option<String>,
    pub display_agent: Option<String>,
    pub agent_status: HerdrAgentStatus,
    pub state_labels: BTreeMap<String, String>,
    pub tokens: BTreeMap<String, String>,
    pub agent_session: Option<AgentSessionRecord>,
    pub scroll: Option<PaneScrollRecord>,
    pub revision: u64,
}

impl PaneRecord {
    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            pane_id: v.get("pane_id")?.as_str()?.to_string(),
            terminal_id: optional_string(v, "terminal_id"),
            workspace_id: v.get("workspace_id")?.as_str()?.to_string(),
            tab_id: v.get("tab_id")?.as_str()?.to_string(),
            focused: v.get("focused").and_then(Value::as_bool).unwrap_or(false),
            cwd: optional_string(v, "cwd"),
            foreground_cwd: optional_string(v, "foreground_cwd"),
            label: optional_string(v, "label"),
            agent: optional_string(v, "agent"),
            title: optional_string(v, "title"),
            terminal_title: optional_string(v, "terminal_title"),
            terminal_title_stripped: optional_string(v, "terminal_title_stripped"),
            display_agent: optional_string(v, "display_agent"),
            agent_status: HerdrAgentStatus::from_value(v.get("agent_status")),
            state_labels: string_map(v.get("state_labels")),
            tokens: string_map(v.get("tokens")),
            agent_session: v
                .get("agent_session")
                .and_then(AgentSessionRecord::from_json),
            scroll: v.get("scroll").and_then(PaneScrollRecord::from_json),
            revision: v.get("revision").and_then(Value::as_u64).unwrap_or(0),
        })
    }
}

/// Herdr agent status wire 值。未知值保留原文，产品层统一映射为 Unknown。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HerdrAgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown(String),
}

impl HerdrAgentStatus {
    fn from_value(value: Option<&Value>) -> Self {
        match value.and_then(Value::as_str).unwrap_or("unknown") {
            "idle" => Self::Idle,
            "working" => Self::Working,
            "blocked" => Self::Blocked,
            "done" => Self::Done,
            other => Self::Unknown(other.to_string()),
        }
    }
}

/// Herdr agent session 引用；kind 仍保留未知 wire 值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSessionRecord {
    pub source: String,
    pub agent: String,
    pub kind: String,
    pub value: String,
}

impl AgentSessionRecord {
    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            source: v.get("source")?.as_str()?.to_string(),
            agent: v.get("agent")?.as_str()?.to_string(),
            kind: v.get("kind")?.as_str()?.to_string(),
            value: v.get("value")?.as_str()?.to_string(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneScrollRecord {
    pub offset_from_bottom: u64,
    pub max_offset_from_bottom: u64,
    pub viewport_rows: u64,
}

impl PaneScrollRecord {
    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            offset_from_bottom: v.get("offset_from_bottom")?.as_u64()?,
            max_offset_from_bottom: v.get("max_offset_from_bottom")?.as_u64()?,
            viewport_rows: v.get("viewport_rows")?.as_u64()?,
        })
    }
}

/// `session.snapshot.agents[]` 的完整协议 19 记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRecord {
    pub terminal_id: Option<String>,
    pub name: Option<String>,
    pub agent: Option<String>,
    pub title: Option<String>,
    pub terminal_title: Option<String>,
    pub terminal_title_stripped: Option<String>,
    pub display_agent: Option<String>,
    pub agent_status: HerdrAgentStatus,
    pub screen_detection_skipped: bool,
    pub state_labels: BTreeMap<String, String>,
    pub tokens: BTreeMap<String, String>,
    pub agent_session: Option<AgentSessionRecord>,
    pub workspace_id: String,
    pub tab_id: String,
    pub pane_id: String,
    pub focused: bool,
    pub launch_pending: bool,
    pub interactive_ready: bool,
    pub state_change_seq: u64,
    pub cwd: Option<String>,
    pub foreground_cwd: Option<String>,
    pub revision: u64,
}

impl AgentRecord {
    pub(crate) fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            terminal_id: optional_string(v, "terminal_id"),
            name: optional_string(v, "name"),
            agent: optional_string(v, "agent"),
            title: optional_string(v, "title"),
            terminal_title: optional_string(v, "terminal_title"),
            terminal_title_stripped: optional_string(v, "terminal_title_stripped"),
            display_agent: optional_string(v, "display_agent"),
            agent_status: HerdrAgentStatus::from_value(v.get("agent_status")),
            screen_detection_skipped: v
                .get("screen_detection_skipped")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            state_labels: string_map(v.get("state_labels")),
            tokens: string_map(v.get("tokens")),
            agent_session: v
                .get("agent_session")
                .and_then(AgentSessionRecord::from_json),
            workspace_id: v.get("workspace_id")?.as_str()?.to_string(),
            tab_id: v.get("tab_id")?.as_str()?.to_string(),
            pane_id: v.get("pane_id")?.as_str()?.to_string(),
            focused: v.get("focused").and_then(Value::as_bool).unwrap_or(false),
            launch_pending: v
                .get("launch_pending")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            interactive_ready: v
                .get("interactive_ready")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            state_change_seq: v
                .get("state_change_seq")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cwd: optional_string(v, "cwd"),
            foreground_cwd: optional_string(v, "foreground_cwd"),
            revision: v.get("revision").and_then(Value::as_u64).unwrap_or(0),
        })
    }
}

fn optional_string(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(ToOwned::to_owned)
}

fn string_map(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Herdr 布局矩形（字符格坐标）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl LayoutRect {
    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            x: v.get("x").and_then(Value::as_u64).unwrap_or(0) as u16,
            y: v.get("y").and_then(Value::as_u64).unwrap_or(0) as u16,
            width: v.get("width")?.as_u64()? as u16,
            height: v.get("height")?.as_u64()? as u16,
        })
    }
}

/// Herdr layout 中一个 pane 的位置与焦点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutPaneRecord {
    pub pane_id: String,
    pub focused: bool,
    pub rect: LayoutRect,
}

/// Herdr wire 的 split 方向。未知值保留原文，避免静默误判为左右分割。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutSplitDirection {
    Right,
    Down,
    Unknown(String),
}

impl LayoutSplitDirection {
    fn from_wire(value: &str) -> Self {
        match value {
            "right" => Self::Right,
            "down" => Self::Down,
            other => Self::Unknown(other.to_string()),
        }
    }
}

/// Herdr layout 中一个 BSP split；`path` 的 false/true 分别表示 first/second。
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutSplitRecord {
    pub id: String,
    pub path: Vec<bool>,
    pub direction: LayoutSplitDirection,
    pub ratio: f32,
    pub rect: LayoutRect,
}

/// Herdr tab 的完整 `PaneLayoutSnapshot`。
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutRecord {
    pub workspace_id: String,
    pub tab_id: String,
    pub zoomed: bool,
    pub area: LayoutRect,
    pub focused_pane_id: String,
    pub panes: Vec<LayoutPaneRecord>,
    pub splits: Vec<LayoutSplitRecord>,
}

impl LayoutRecord {
    pub(crate) fn from_json(v: &Value) -> Option<Self> {
        let area = LayoutRect::from_json(v.get("area")?)?;
        let panes: Vec<LayoutPaneRecord> = v
            .get("panes")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|pane| {
                        Some(LayoutPaneRecord {
                            pane_id: pane.get("pane_id")?.as_str()?.to_string(),
                            focused: pane
                                .get("focused")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                            // 协议 19 总有 rect；旧录制夹具缺失时用整个 area
                            // 保持兼容，但 Runtime 只在现代快照上按 rect 分配。
                            rect: pane
                                .get("rect")
                                .and_then(LayoutRect::from_json)
                                .unwrap_or(area),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let focused_pane_id = v
            .get("focused_pane_id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| {
                panes
                    .iter()
                    .find(|pane| pane.focused)
                    .map(|pane| pane.pane_id.clone())
            })
            .or_else(|| panes.first().map(|pane| pane.pane_id.clone()))
            .unwrap_or_default();
        let splits = v
            .get("splits")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|split| {
                        let id = split.get("id")?.as_str()?.to_string();
                        Some(LayoutSplitRecord {
                            path: split_path_from_id(&id)?,
                            id,
                            direction: LayoutSplitDirection::from_wire(
                                split.get("direction")?.as_str()?,
                            ),
                            ratio: split.get("ratio").and_then(Value::as_f64).unwrap_or(0.5) as f32,
                            rect: split
                                .get("rect")
                                .and_then(LayoutRect::from_json)
                                .unwrap_or(area),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            workspace_id: v.get("workspace_id")?.as_str()?.to_string(),
            tab_id: v.get("tab_id")?.as_str()?.to_string(),
            zoomed: v.get("zoomed").and_then(Value::as_bool).unwrap_or(false),
            area,
            focused_pane_id,
            panes,
            splits,
        })
    }
}

/// `split_<idx>_root` 或 `split_<idx>_<01 path>` → BSP path。
fn split_path_from_id(id: &str) -> Option<Vec<bool>> {
    let mut parts = id.splitn(3, '_');
    if parts.next()? != "split" {
        return None;
    }
    parts.next()?.parse::<usize>().ok()?;
    let encoded = parts.next()?;
    if encoded == "root" {
        return Some(Vec::new());
    }
    encoded
        .bytes()
        .map(|byte| match byte {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        })
        .collect()
}

/// `worktree.list` 的完整响应（source + worktrees）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrWorktreeList {
    pub repo_root: String,
    pub worktrees: Vec<HerdrWorktreeRecord>,
}

impl HerdrWorktreeList {
    fn from_json(v: &Value) -> Result<Self> {
        let source = v.get("source").cloned().unwrap_or_default();
        let worktrees = v
            .get("worktrees")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(HerdrWorktreeRecord::from_worktree_json)
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            repo_root: source
                .get("repo_root")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            worktrees,
        })
    }
}

/// 一个 Herdr worktree checkout（Herdr id 保持字符串，池层映射产品 WorkspaceId）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrWorktreeRecord {
    pub path: String,
    pub branch: String,
    pub repo_root: String,
    pub linked: bool,
    pub open_workspace_id: Option<String>,
}

impl HerdrWorktreeRecord {
    /// 从 `worktree.list` 的 worktrees 数组项解析。
    fn from_worktree_json(v: &Value) -> Option<Self> {
        Some(Self {
            path: v.get("path")?.as_str()?.to_string(),
            branch: v
                .get("branch")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            repo_root: String::new(),
            linked: v
                .get("is_linked_worktree")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            open_workspace_id: v
                .get("open_workspace_id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        })
    }

    /// 从 `worktree.create` / `worktree.open` 的 result 解析（repo_root 在 workspace.worktree）。
    fn from_json(v: &Value) -> Result<Self> {
        let worktree = v
            .get("worktree")
            .ok_or_else(|| anyhow!("worktree 响应缺 worktree: {v}"))?;
        let mut record = Self::from_worktree_json(worktree)
            .ok_or_else(|| anyhow!("worktree 响应解析失败: {v}"))?;
        record.repo_root = v
            .get("workspace")
            .and_then(|w| w.get("worktree"))
            .and_then(|w| w.get("repo_root"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_derives_client_socket_from_api_socket() {
        let s = HerdrSession::new(
            "muxterm-test-herdr-x-1",
            "/home/wlz/.config/herdr/sessions/muxterm-test-herdr-x-1/herdr.sock",
        );
        assert_eq!(
            s.client_socket_path(),
            Path::new("/home/wlz/.config/herdr/sessions/muxterm-test-herdr-x-1/herdr-client.sock")
        );
        assert_eq!(s.name(), "muxterm-test-herdr-x-1");

        let forwarded = HerdrSession::new("default", "/tmp/muxterm-herdr-fwd-loopback-123.sock");
        assert_eq!(
            forwarded.client_socket_path(),
            Path::new("/tmp/muxterm-herdr-fwd-loopback-123-client.sock")
        );
    }

    #[test]
    fn snapshot_parses_known_shape() {
        let v: Value = serde_json::from_str(
            r#"{"version":"0.8.0","protocol":19,"focused_workspace_id":"w1","focused_tab_id":"w1:t1","focused_pane_id":"w1:p1","workspaces":[{"workspace_id":"w1","label":"probe1","active_tab_id":"w1:t1"}],"tabs":[{"tab_id":"w1:t1","workspace_id":"w1","label":"1"}],"panes":[{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","cwd":"/tmp"},{"pane_id":"w1:p2","workspace_id":"w1","tab_id":"w1:t1","cwd":"/tmp"}],"layouts":[{"workspace_id":"w1","tab_id":"w1:t1","zoomed":false,"area":{"x":2,"y":3,"width":54,"height":23},"focused_pane_id":"w1:p2","panes":[{"pane_id":"w1:p1","focused":false,"rect":{"x":2,"y":3,"width":54,"height":11}},{"pane_id":"w1:p2","focused":true,"rect":{"x":2,"y":14,"width":54,"height":12}}],"splits":[{"id":"split_0_root","direction":"down","ratio":0.48,"rect":{"x":2,"y":3,"width":54,"height":23}}]}]}"#,
        )
        .unwrap();
        let snap = SessionSnapshot::from_json(&v).unwrap();
        assert_eq!(snap.workspaces[0].workspace_id, "w1");
        assert_eq!(snap.panes[0].pane_id, "w1:p1");
        assert_eq!(
            snap.layouts[0].area,
            LayoutRect {
                x: 2,
                y: 3,
                width: 54,
                height: 23,
            }
        );
        assert_eq!(snap.layouts[0].focused_pane_id, "w1:p2");
        assert_eq!(snap.layouts[0].panes[0].rect.height, 11);
        assert_eq!(snap.layouts[0].panes[1].rect.y, 14);
        assert_eq!(snap.layouts[0].splits[0].path, Vec::<bool>::new());
        assert_eq!(
            snap.layouts[0].splits[0].direction,
            LayoutSplitDirection::Down
        );
        assert!((snap.layouts[0].splits[0].ratio - 0.48).abs() < f32::EPSILON);
    }

    #[test]
    fn snapshot_preserves_every_protocol_19_agent_field() {
        let v = serde_json::json!({
            "version": "0.8.0",
            "protocol": 19,
            "workspaces": [{
                "workspace_id": "w7",
                "number": 7,
                "label": "agent-workspace",
                "focused": true,
                "pane_count": 1,
                "tab_count": 1,
                "active_tab_id": "w7:tA",
                "agent_status": "blocked",
                "tokens": { "context": "73%" },
                "worktree": {
                    "repo_key": "/repo/.git",
                    "repo_name": "repo",
                    "repo_root": "/repo",
                    "checkout_path": "/repo-wt",
                    "is_linked_worktree": true
                }
            }],
            "tabs": [{
                "tab_id": "w7:tA",
                "workspace_id": "w7",
                "number": 10,
                "label": "agent-tab",
                "focused": true,
                "pane_count": 1,
                "agent_status": "blocked"
            }],
            "panes": [{
                "pane_id": "w7:pR",
                "terminal_id": "term-agent",
                "workspace_id": "w7",
                "tab_id": "w7:tA",
                "focused": true,
                "cwd": "/repo-wt",
                "foreground_cwd": "/repo-wt/src",
                "label": "review",
                "agent": "codex",
                "title": "Approve command",
                "terminal_title": "codex --full-auto",
                "terminal_title_stripped": "codex",
                "display_agent": "Codex reviewer",
                "agent_status": "blocked",
                "state_labels": { "blocked": "Needs approval" },
                "tokens": { "context": "73%", "model": "gpt-5" },
                "agent_session": {
                    "source": "herdr:codex",
                    "agent": "codex",
                    "kind": "id",
                    "value": "session-123"
                },
                "scroll": {
                    "offset_from_bottom": 2,
                    "max_offset_from_bottom": 20,
                    "viewport_rows": 42
                },
                "revision": 91
            }],
            "layouts": [],
            "agents": [{
                "terminal_id": "term-agent",
                "name": "reviewer-1",
                "agent": "codex",
                "title": "Approve command",
                "terminal_title": "codex --full-auto",
                "terminal_title_stripped": "codex",
                "display_agent": "Codex reviewer",
                "agent_status": "blocked",
                "screen_detection_skipped": true,
                "state_labels": {
                    "working": "Implementing",
                    "blocked": "Needs approval"
                },
                "tokens": {
                    "context": "73%",
                    "model": "gpt-5"
                },
                "agent_session": {
                    "source": "herdr:codex",
                    "agent": "codex",
                    "kind": "id",
                    "value": "session-123"
                },
                "workspace_id": "w7",
                "tab_id": "w7:tA",
                "pane_id": "w7:pR",
                "focused": true,
                "launch_pending": true,
                "interactive_ready": true,
                "state_change_seq": 44,
                "cwd": "/repo-wt",
                "foreground_cwd": "/repo-wt/src",
                "revision": 91
            }]
        });

        let snap = SessionSnapshot::from_json(&v).unwrap();
        let workspace = &snap.workspaces[0];
        assert_eq!(workspace.number, 7);
        assert!(workspace.focused);
        assert_eq!(workspace.pane_count, 1);
        assert_eq!(workspace.tab_count, 1);
        assert_eq!(workspace.agent_status, HerdrAgentStatus::Blocked);
        assert_eq!(workspace.tokens["context"], "73%");
        let worktree = workspace.worktree.as_ref().unwrap();
        assert_eq!(worktree.repo_key, "/repo/.git");
        assert_eq!(worktree.repo_name, "repo");
        assert_eq!(worktree.repo_root, "/repo");
        assert_eq!(worktree.checkout_path, "/repo-wt");
        assert!(worktree.linked);

        let pane = &snap.panes[0];
        assert_eq!(pane.terminal_id.as_deref(), Some("term-agent"));
        assert_eq!(pane.foreground_cwd.as_deref(), Some("/repo-wt/src"));
        assert_eq!(pane.display_agent.as_deref(), Some("Codex reviewer"));
        assert_eq!(pane.state_labels["blocked"], "Needs approval");
        assert_eq!(pane.tokens["model"], "gpt-5");
        assert_eq!(pane.agent_session.as_ref().unwrap().kind, "id");
        assert_eq!(pane.scroll.unwrap().viewport_rows, 42);
        assert_eq!(pane.revision, 91);

        let agent = &snap.agents[0];
        assert_eq!(agent.terminal_id.as_deref(), Some("term-agent"));
        assert_eq!(agent.name.as_deref(), Some("reviewer-1"));
        assert_eq!(agent.agent.as_deref(), Some("codex"));
        assert_eq!(agent.title.as_deref(), Some("Approve command"));
        assert_eq!(agent.terminal_title.as_deref(), Some("codex --full-auto"));
        assert_eq!(agent.terminal_title_stripped.as_deref(), Some("codex"));
        assert_eq!(agent.display_agent.as_deref(), Some("Codex reviewer"));
        assert_eq!(agent.agent_status, HerdrAgentStatus::Blocked);
        assert!(agent.screen_detection_skipped);
        assert_eq!(agent.state_labels["working"], "Implementing");
        assert_eq!(agent.tokens["context"], "73%");
        let session = agent.agent_session.as_ref().unwrap();
        assert_eq!(session.source, "herdr:codex");
        assert_eq!(session.agent, "codex");
        assert_eq!(session.kind, "id");
        assert_eq!(session.value, "session-123");
        assert_eq!(agent.workspace_id, "w7");
        assert_eq!(agent.tab_id, "w7:tA");
        assert_eq!(agent.pane_id, "w7:pR");
        assert!(agent.focused);
        assert!(agent.launch_pending);
        assert!(agent.interactive_ready);
        assert_eq!(agent.state_change_seq, 44);
        assert_eq!(agent.cwd.as_deref(), Some("/repo-wt"));
        assert_eq!(agent.foreground_cwd.as_deref(), Some("/repo-wt/src"));
        assert_eq!(agent.revision, 91);
    }

    #[test]
    fn agent_parser_preserves_unknown_wire_status_and_session_kind() {
        let agent = AgentRecord::from_json(&serde_json::json!({
            "terminal_id": "term-future",
            "agent_status": "paused_for_policy",
            "agent_session": {
                "source": "future:agent",
                "agent": "future",
                "kind": "uri",
                "value": "agent://session/1"
            },
            "workspace_id": "w1",
            "tab_id": "w1:t1",
            "pane_id": "w1:p1",
            "focused": false,
            "revision": 1
        }))
        .unwrap();
        assert_eq!(
            agent.agent_status,
            HerdrAgentStatus::Unknown("paused_for_policy".into())
        );
        assert_eq!(agent.agent_session.unwrap().kind, "uri");
    }

    #[test]
    fn split_ids_decode_root_and_binary_paths() {
        assert_eq!(split_path_from_id("split_0_root"), Some(vec![]));
        assert_eq!(split_path_from_id("split_1_0"), Some(vec![false]));
        assert_eq!(
            split_path_from_id("split_12_011"),
            Some(vec![false, true, true])
        );
        assert_eq!(split_path_from_id("split_bad_root"), None);
        assert_eq!(split_path_from_id("split_1_02"), None);
    }
}
