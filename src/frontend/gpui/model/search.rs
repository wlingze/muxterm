//! 搜索快照与搜索列表纯逻辑。
//!
//! 移植自 `chrome/SearchModel.swift` 与 `chrome/ConnectProgress.swift`。

/// 一次搜索命中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub workspace_id: String,
    pub tab_id: u32,
    pub pane_id: u32,
    pub seq: u64,
    pub line: String,
}

/// 搜索快照（Core `muxterm_search_all` JSON 的视图）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchSnapshot {
    pub hits: Vec<SearchHit>,
}

impl SearchSnapshot {
    /// 解析 Core JSON；失败返回 `None`，不 panic。
    pub fn decode(data: &[u8]) -> Option<Self> {
        let json: serde_json::Value = serde_json::from_slice(data).ok()?;
        if json.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            return None;
        }
        let hits = json
            .get("hits")
            .and_then(|v| v.as_array())
            .map(|items| items.iter().filter_map(decode_hit).collect())
            .unwrap_or_default();
        Some(Self { hits })
    }
}

fn decode_hit(value: &serde_json::Value) -> Option<SearchHit> {
    Some(SearchHit {
        workspace_id: value.get("workspace_id")?.as_str()?.to_string(),
        // tab_id == 0 也是合法 tab（整棵布局的单叶编号）。
        tab_id: value.get("tab_id")?.as_u64()? as u32,
        pane_id: value.get("pane_id")?.as_u64()? as u32,
        seq: value.get("seq").and_then(|v| v.as_u64()).unwrap_or(0),
        line: value
            .get("line")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

/// 搜索列表纯逻辑：按 query 过滤命中行，空结果标记占位。
pub fn search_rows(snapshot: &SearchSnapshot, query: &str) -> (Vec<SearchHit>, bool) {
    let q = query.trim().to_lowercase();
    let rows: Vec<SearchHit> = snapshot
        .hits
        .iter()
        .filter(|hit| {
            q.is_empty()
                || hit.line.to_lowercase().contains(&q)
                || hit.workspace_id.to_lowercase().contains(&q)
        })
        .cloned()
        .collect();
    let is_empty = rows.is_empty();
    (rows, is_empty)
}

/// SSH / 本地 attach 的进度阶段。
///
/// 阶段名对齐生产日志：resolving / ssh / list-sessions / attach / capture。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectStage {
    Resolving,
    Ssh,
    ListSessions,
    Attach,
    Capture,
}

impl ConnectStage {
    /// 全部阶段，顺序稳定。
    pub const ALL: [ConnectStage; 5] = [
        ConnectStage::Resolving,
        ConnectStage::Ssh,
        ConnectStage::ListSessions,
        ConnectStage::Attach,
        ConnectStage::Capture,
    ];

    /// 中文注释：与日志、无障碍值一致的字面量。
    pub fn raw_value(self) -> &'static str {
        match self {
            ConnectStage::Resolving => "resolving",
            ConnectStage::Ssh => "ssh",
            ConnectStage::ListSessions => "list-sessions",
            ConnectStage::Attach => "attach",
            ConnectStage::Capture => "capture",
        }
    }
}

/// 连接进度面板的稳定标识。
pub const CONNECT_PROGRESS_IDENTIFIER: &str = "muxterm.connectProgress";
/// 连接进度面板里 spinner 的标识。
pub const CONNECT_PROGRESS_SPINNER_IDENTIFIER: &str = "muxterm.connectProgress.spinner";

#[cfg(test)]
mod tests {
    use super::*;

    // === ConnectProgressTests ===

    /// 对照 `testStagesMatchSshAttachLog`。
    #[test]
    fn stages_match_ssh_attach_log() {
        assert_eq!(
            ConnectStage::ALL.map(ConnectStage::raw_value),
            ["resolving", "ssh", "list-sessions", "attach", "capture"]
        );
    }

    /// 对照 `testOverlayIdentifierIsStable`。
    #[test]
    fn overlay_identifier_is_stable() {
        assert_eq!(CONNECT_PROGRESS_IDENTIFIER, "muxterm.connectProgress");
        assert_eq!(ConnectStage::ListSessions.raw_value(), "list-sessions");
    }

    // === SearchModelTests ===

    /// 对照 `testDecodesHits`。
    #[test]
    fn decodes_hits() {
        let json = r#"
        {
          "ok": true,
          "hits": [
            {"workspace_id": "legion@local", "tab_id": 1, "pane_id": 1, "seq": 3, "line": "TOKEN_BODY example"},
            {"workspace_id": "other@local", "tab_id": 2, "pane_id": 2, "seq": 7, "line": "build ok"}
          ]
        }
        "#;
        let snapshot = SearchSnapshot::decode(json.as_bytes()).expect("应当能解析");
        assert_eq!(snapshot.hits.len(), 2);
        assert_eq!(snapshot.hits[0].workspace_id, "legion@local");
        assert_eq!(snapshot.hits[0].tab_id, 1);
        assert_eq!(snapshot.hits[0].pane_id, 1);
        assert_eq!(snapshot.hits[0].seq, 3);
        assert_eq!(snapshot.hits[0].line, "TOKEN_BODY example");
    }

    /// 对照 `testDecodesTabIdZero`：tab 0 是合法编号，不能被当成缺失。
    #[test]
    fn decodes_tab_id_zero() {
        let json = r#"
        {
          "ok": true,
          "hits": [
            {"workspace_id": "ws@local", "tab_id": 0, "pane_id": 1, "seq": 9, "line": "on-tab-zero"}
          ]
        }
        "#;
        let snapshot = SearchSnapshot::decode(json.as_bytes()).expect("应当能解析");
        assert_eq!(snapshot.hits.len(), 1);
        assert_eq!(snapshot.hits[0].tab_id, 0);
        assert_eq!(snapshot.hits[0].seq, 9);
    }

    /// 对照 `testDecodeFailureReturnsNil`。
    #[test]
    fn decode_failure_returns_none() {
        assert!(SearchSnapshot::decode(b"bad").is_none());
        assert!(SearchSnapshot::decode(br#"{"ok": false}"#).is_none());
    }

    /// 对照 `SearchListTests.testFiltersByLineAndWorkspace`。
    #[test]
    fn filters_by_line_and_workspace() {
        let snapshot = SearchSnapshot {
            hits: vec![
                SearchHit {
                    workspace_id: "legion@local".to_string(),
                    tab_id: 1,
                    pane_id: 1,
                    seq: 3,
                    line: "TOKEN_BODY example".to_string(),
                },
                SearchHit {
                    workspace_id: "other@local".to_string(),
                    tab_id: 2,
                    pane_id: 2,
                    seq: 7,
                    line: "build ok".to_string(),
                },
            ],
        };
        let (rows, is_empty) = search_rows(&snapshot, "TOKEN_BODY");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pane_id, 1);
        assert!(!is_empty);

        let (empty, empty_flag) = search_rows(&snapshot, "missing");
        assert!(empty.is_empty());
        assert!(empty_flag);
    }

    /// 空 query 返回全部命中，并保持 Core 给的顺序。
    #[test]
    fn empty_query_keeps_all_hits_in_order() {
        let snapshot = SearchSnapshot {
            hits: vec![
                SearchHit {
                    workspace_id: "a@local".to_string(),
                    tab_id: 0,
                    pane_id: 2,
                    seq: 5,
                    line: "second".to_string(),
                },
                SearchHit {
                    workspace_id: "a@local".to_string(),
                    tab_id: 0,
                    pane_id: 1,
                    seq: 4,
                    line: "first".to_string(),
                },
            ],
        };
        let (rows, is_empty) = search_rows(&snapshot, "   ");
        assert_eq!(rows.iter().map(|h| h.seq).collect::<Vec<_>>(), vec![5, 4]);
        assert!(!is_empty);
    }
}
