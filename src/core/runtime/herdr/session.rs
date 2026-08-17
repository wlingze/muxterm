//! HerdrSession：一条 Herdr named session 的 socket 连接身份。
//!
//! - API socket（`herdr.sock`）：JSON `{"id","method","params"}` 请求/响应
//!   （ping / session.snapshot / pane.read / pane.send_input / worktree.*）。
//! - client socket（`herdr-client.sock`）：observe 流（H2 起用）。
//!
//! 一个 `HerdrSession` 可被多个 `HerdrRuntime` 共享（`Arc`），
//! 不要每个 Workspace 再开一条 API socket。

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
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

impl HerdrSession {
    /// 绑定 named session 名 + API socket 绝对路径。
    pub fn new(name: impl Into<String>, socket_path: impl Into<PathBuf>) -> Self {
        let socket_path = socket_path.into();
        let client_socket_path = socket_path
            .parent()
            .map(|d| d.join("herdr-client.sock"))
            .unwrap_or_else(|| PathBuf::from("herdr-client.sock"));
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

    /// `pane.read`：attach 快照（source=visible, format=ansi），返回原始 ANSI 字节。
    pub fn pane_read_ansi(&self, pane_id: &str) -> Result<Vec<u8>> {
        let result = self.call(
            "pane.read",
            serde_json::json!({
                "pane_id": pane_id,
                "source": "visible",
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

    /// `pane.send_input`：写原始文本（粘贴/WriteRaw）。
    pub fn pane_send_input(&self, pane_id: &str, text: &str) -> Result<()> {
        self.call(
            "pane.send_input",
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
}

/// `session.snapshot` 的产品视图（Herdr id 保持字符串，映射在 HerdrRuntime）。
#[derive(Debug, Clone, PartialEq, Eq)]
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
        })
    }
}

/// Herdr workspace 记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRecord {
    pub workspace_id: String,
    pub label: String,
    pub active_tab_id: Option<String>,
}

impl WorkspaceRecord {
    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            workspace_id: v.get("workspace_id")?.as_str()?.to_string(),
            label: v
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            active_tab_id: v
                .get("active_tab_id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        })
    }
}

/// Herdr tab 记录（`w1:t1`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabRecord {
    pub tab_id: String,
    pub workspace_id: String,
    pub label: String,
}

impl TabRecord {
    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            tab_id: v.get("tab_id")?.as_str()?.to_string(),
            workspace_id: v.get("workspace_id")?.as_str()?.to_string(),
            label: v
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
    }
}

/// Herdr pane 记录（`w1:p1`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneRecord {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub cwd: Option<String>,
}

impl PaneRecord {
    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            pane_id: v.get("pane_id")?.as_str()?.to_string(),
            workspace_id: v.get("workspace_id")?.as_str()?.to_string(),
            tab_id: v.get("tab_id")?.as_str()?.to_string(),
            cwd: v.get("cwd").and_then(Value::as_str).map(ToOwned::to_owned),
        })
    }
}

/// Herdr tab layout 记录（面积 + pane 列表）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutRecord {
    pub workspace_id: String,
    pub tab_id: String,
    pub width: u16,
    pub height: u16,
    pub panes: Vec<String>,
}

impl LayoutRecord {
    fn from_json(v: &Value) -> Option<Self> {
        let area = v.get("area")?;
        Some(Self {
            workspace_id: v.get("workspace_id")?.as_str()?.to_string(),
            tab_id: v.get("tab_id")?.as_str()?.to_string(),
            width: area.get("width").and_then(Value::as_u64).unwrap_or(80) as u16,
            height: area.get("height").and_then(Value::as_u64).unwrap_or(24) as u16,
            panes: v
                .get("panes")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|p| p.get("pane_id").and_then(Value::as_str))
                        .map(ToOwned::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
        })
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
    }

    #[test]
    fn snapshot_parses_known_shape() {
        let v: Value = serde_json::from_str(
            r#"{"version":"0.8.0","protocol":19,"focused_workspace_id":"w1","focused_tab_id":"w1:t1","focused_pane_id":"w1:p1","workspaces":[{"workspace_id":"w1","label":"probe1","active_tab_id":"w1:t1"}],"tabs":[{"tab_id":"w1:t1","workspace_id":"w1","label":"1"}],"panes":[{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","cwd":"/tmp"}],"layouts":[{"workspace_id":"w1","tab_id":"w1:t1","area":{"width":54,"height":23},"panes":[{"pane_id":"w1:p1"}]}]}"#,
        )
        .unwrap();
        let snap = SessionSnapshot::from_json(&v).unwrap();
        assert_eq!(snap.workspaces[0].workspace_id, "w1");
        assert_eq!(snap.panes[0].pane_id, "w1:p1");
        assert_eq!(snap.layouts[0].width, 54);
        assert_eq!(snap.layouts[0].panes, vec!["w1:p1".to_string()]);
    }
}
