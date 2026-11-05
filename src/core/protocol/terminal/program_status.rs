//! OSC 7501 Program Status protocol.
//!
//! 程序通过 `OSC 7501;key=value…` 告诉终端自己的生命周期。这里只做规范解析
//! 和按 `id` 合并，不涉及 Attention / Activity；超时由 AttentionEngine 用
//! 可注入时钟判断，避免把 `Instant` 塞进参与指纹的终端状态。

use std::cell::Cell;
use std::collections::BTreeMap;

const MAX_TITLE_BYTES: usize = 64;
const MAX_STATUS_BYTES: usize = 127;

/// 程序自报状态。优先级用于多个 `id` 并存时合并显示。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProgramState {
    Idle,
    Working,
    Done,
    Error,
    Blocked,
}

impl ProgramState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Done => "done",
            Self::Error => "error",
            Self::Blocked => "blocked",
        }
    }

    /// `done` 与 `error` 同级：互不覆盖。
    pub fn priority(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Working => 1,
            Self::Done | Self::Error => 2,
            Self::Blocked => 3,
        }
    }

    fn parse(value: &[u8]) -> Option<Self> {
        match value {
            b"idle" => Some(Self::Idle),
            b"working" => Some(Self::Working),
            b"done" => Some(Self::Done),
            b"error" => Some(Self::Error),
            b"blocked" => Some(Self::Blocked),
            _ => None,
        }
    }
}

/// 一个 `id` 槽位的完整状态。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProgramStatusEntry {
    pub id: String,
    pub state: ProgramState,
    /// `None` 表示已清除；字段省略时保留上一份。
    pub title: Option<String>,
    pub status: Option<String>,
    pub sticky: bool,
    /// `None` 表示由终端决定；`Some(0)` 表示不过期。
    pub ttl_secs: Option<u32>,
}

/// 某个 pane 当前的 7501 快照。`entries` 按 `id` 排序，稳定可测。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramStatusSnapshot {
    pub revision: u64,
    pub entries: Vec<ProgramStatusEntry>,
}

impl ProgramStatusSnapshot {
    /// 合并后的显示条目：最高优先级；`done` / `error` 同级时保留上一份
    /// 显示的终态，避免两个 id 互相闪烁。
    pub fn dominant(&self, preferred: Option<ProgramState>) -> Option<&ProgramStatusEntry> {
        let highest = self
            .entries
            .iter()
            .map(|entry| entry.state.priority())
            .max()?;
        let candidates = self
            .entries
            .iter()
            .filter(|entry| entry.state.priority() == highest)
            .collect::<Vec<_>>();
        if let Some(preferred) = preferred {
            if let Some(entry) = candidates.iter().find(|entry| entry.state == preferred) {
                return Some(*entry);
            }
        }
        candidates.first().copied()
    }
}

/// 解析 + 合并 7501 的 `key=value` 参数。
///
/// 非法帧整条丢弃，保留上一份状态。
#[derive(Default)]
struct Frame {
    id: String,
    state: Option<ProgramState>,
    title: Option<Option<String>>,
    status: Option<Option<String>>,
    ttl: Option<u32>,
    sticky: Option<bool>,
    clear: bool,
    seen_id: bool,
    seen_state: bool,
    seen_title: bool,
    seen_status: bool,
    seen_ttl: bool,
    seen_sticky: bool,
    seen_clear: bool,
}

impl Frame {
    fn parse(params: &[&[u8]]) -> Option<Self> {
        let mut frame = Self::default();
        for raw in params {
            let eq = raw.iter().position(|byte| *byte == b'=')?;
            let (key, value) = (&raw[..eq], &raw[eq + 1..]);
            match key {
                b"id" => {
                    if frame.seen_id {
                        return None;
                    }
                    frame.seen_id = true;
                    frame.id = std::str::from_utf8(value).ok()?.to_string();
                }
                b"state" => {
                    if frame.seen_state {
                        return None;
                    }
                    frame.seen_state = true;
                    frame.state = Some(ProgramState::parse(value)?);
                }
                b"title" => {
                    if frame.seen_title {
                        return None;
                    }
                    frame.seen_title = true;
                    frame.title = Some(parse_text(value, MAX_TITLE_BYTES)?);
                }
                b"status" => {
                    if frame.seen_status {
                        return None;
                    }
                    frame.seen_status = true;
                    frame.status = Some(parse_text(value, MAX_STATUS_BYTES)?);
                }
                b"ttl" => {
                    if frame.seen_ttl {
                        return None;
                    }
                    frame.seen_ttl = true;
                    frame.ttl = Some(std::str::from_utf8(value).ok()?.parse::<u32>().ok()?);
                }
                b"sticky" => {
                    if frame.seen_sticky {
                        return None;
                    }
                    frame.seen_sticky = true;
                    frame.sticky = Some(match value {
                        b"1" => true,
                        b"0" | b"" => false,
                        _ => return None,
                    });
                }
                b"clear" => {
                    if frame.seen_clear {
                        return None;
                    }
                    frame.seen_clear = true;
                    frame.clear = match value {
                        b"1" => true,
                        b"0" => false,
                        _ => return None,
                    };
                }
                _ => return None,
            }
        }
        if frame.clear {
            // `clear=1` 只清 title/status；与 state 或其它字段混用视为坏帧。
            if frame.state.is_some()
                || frame.title.is_some()
                || frame.status.is_some()
                || frame.ttl.is_some()
                || frame.sticky.is_some()
            {
                return None;
            }
        } else if frame.state.is_none() {
            return None;
        }
        Some(frame)
    }
}

/// `title` / `status` 的 UTF-8 字节截断；空值表示清除。
fn parse_text(value: &[u8], max_bytes: usize) -> Option<Option<String>> {
    if value.is_empty() {
        return Some(None);
    }
    let text = std::str::from_utf8(value).ok()?;
    let mut end = max_bytes.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(Some(text[..end].to_string()))
}

/// `TerminalState` 内的 7501 状态仓库。
#[derive(Debug, Default)]
pub struct ProgramStatusStore {
    entries: BTreeMap<String, ProgramStatusEntry>,
    revision: u64,
    exported_revision: Cell<u64>,
}

impl ProgramStatusStore {
    /// 应用一帧 `7501;...`（参数不含 `7501`）。返回内容是否变化。
    pub fn apply_params(&mut self, params: &[&[u8]]) -> bool {
        let Some(frame) = Frame::parse(params) else {
            return false;
        };
        if frame.clear {
            let Some(existing) = self.entries.get(&frame.id) else {
                return false;
            };
            let mut next = existing.clone();
            next.title = None;
            next.status = None;
            if next == *existing {
                return false;
            }
            self.entries.insert(frame.id, next);
            self.bump_revision();
            return true;
        }

        let state = frame.state.expect("非 clear 帧必须带 state");
        let Some(existing) = self.entries.get(&frame.id).cloned() else {
            // sticky=1 只允许 done/error；否则整帧无效。
            if frame.sticky == Some(true)
                && !matches!(state, ProgramState::Done | ProgramState::Error)
            {
                return false;
            }
            self.entries.insert(
                frame.id.clone(),
                ProgramStatusEntry {
                    id: frame.id,
                    state,
                    title: frame.title.flatten(),
                    status: frame.status.flatten(),
                    sticky: frame.sticky.unwrap_or(false)
                        && matches!(state, ProgramState::Done | ProgramState::Error),
                    ttl_secs: frame.ttl,
                },
            );
            self.bump_revision();
            return true;
        };

        let mut next = existing.clone();
        next.state = state;
        if let Some(title) = frame.title {
            next.title = title;
        }
        if let Some(status) = frame.status {
            next.status = status;
        }
        if let Some(ttl) = frame.ttl {
            next.ttl_secs = Some(ttl);
        }
        if let Some(sticky) = frame.sticky {
            next.sticky = sticky;
        }
        if frame.sticky == Some(true) && !matches!(state, ProgramState::Done | ProgramState::Error)
        {
            return false;
        }
        // 状态离开 done/error 后，旧 sticky 不能继续把 pane 钉住。
        if !matches!(state, ProgramState::Done | ProgramState::Error) {
            next.sticky = false;
        }
        self.entries.insert(frame.id, next);
        // 即使内容相同也要前进 revision：重复帧是 TTL 的心跳，
        // engine 用「快照到达时刻」重新计时。
        self.bump_revision();
        true
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }

    /// 内容未变化时返回 `None`，避免每次轮询都跨 FFI 搬运。
    pub fn snapshot(&self) -> Option<ProgramStatusSnapshot> {
        if self.revision == self.exported_revision.get() {
            return None;
        }
        self.exported_revision.set(self.revision);
        Some(self.current_snapshot())
    }

    /// 强制读取当前快照（测试/诊断用，不影响导出水位）。
    pub fn current_snapshot(&self) -> ProgramStatusSnapshot {
        ProgramStatusSnapshot {
            revision: self.revision,
            entries: self.entries.values().cloned().collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        self.entries.clear();
        self.bump_revision();
    }

    /// PaneBuf 被整屏替换时，即使旧状态为空也要让 engine 收到一次空快照，
    /// 否则 resync 前遗留的 blocked/done 会一直挂到 TTL。
    pub fn invalidate(&mut self) {
        self.entries.clear();
        self.bump_revision();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(store: &mut ProgramStatusStore, text: &str) -> bool {
        let params = text.split(';').map(str::as_bytes).collect::<Vec<&[u8]>>();
        store.apply_params(&params)
    }

    fn one(store: &ProgramStatusStore) -> ProgramStatusEntry {
        let entries = store.current_snapshot().entries;
        assert_eq!(entries.len(), 1);
        entries[0].clone()
    }

    #[test]
    fn parses_state_title_status_and_revision() {
        let mut store = ProgramStatusStore::default();
        assert!(apply(
            &mut store,
            "state=working;title=claude;status=thinking"
        ));
        let entry = one(&store);
        assert_eq!(entry.id, "");
        assert_eq!(entry.state, ProgramState::Working);
        assert_eq!(entry.title.as_deref(), Some("claude"));
        assert_eq!(entry.status.as_deref(), Some("thinking"));
        assert_eq!(store.current_snapshot().revision, 1);
        assert!(store.snapshot().is_some());
        assert!(store.snapshot().is_none(), "未变化不重复导出");
    }

    #[test]
    fn omitted_fields_retain_and_empty_fields_clear() {
        let mut store = ProgramStatusStore::default();
        assert!(apply(
            &mut store,
            "state=working;title=claude;status=thinking"
        ));
        assert!(apply(&mut store, "state=blocked"));
        let entry = one(&store);
        assert_eq!(entry.state, ProgramState::Blocked);
        assert_eq!(entry.title.as_deref(), Some("claude"));
        assert_eq!(entry.status.as_deref(), Some("thinking"));

        assert!(apply(&mut store, "state=blocked;title=;status="));
        let entry = one(&store);
        assert_eq!(entry.title, None);
        assert_eq!(entry.status, None);
        assert_eq!(entry.state, ProgramState::Blocked);
    }

    #[test]
    fn clear_only_clears_text_not_ttl_or_sticky() {
        let mut store = ProgramStatusStore::default();
        assert!(apply(
            &mut store,
            "state=done;title=claude;status=ok;ttl=30;sticky=1"
        ));
        assert!(apply(&mut store, "clear=1"));
        let entry = one(&store);
        assert_eq!(entry.state, ProgramState::Done);
        assert_eq!(entry.title, None);
        assert_eq!(entry.status, None);
        assert_eq!(entry.ttl_secs, Some(30));
        assert!(entry.sticky);
    }

    #[test]
    fn invalid_frames_are_dropped_without_losing_state() {
        let mut store = ProgramStatusStore::default();
        assert!(apply(&mut store, "state=working;title=claude"));
        for bad in [
            "state=bogus",
            "state=",
            "state=working;state=blocked",
            "state=working;unknown=1",
            "state=working;sticky=1",
            "state=working;ttl=abc",
            "clear=1;state=working",
            "clear=1;title=claude",
            "state=working;title",
        ] {
            assert!(!apply(&mut store, bad), "应丢弃坏帧：{bad}");
            let entry = one(&store);
            assert_eq!(entry.state, ProgramState::Working, "坏帧后保留上一份");
            assert_eq!(entry.title.as_deref(), Some("claude"));
        }
    }

    #[test]
    fn sticky_is_only_allowed_for_done_and_error() {
        let mut store = ProgramStatusStore::default();
        assert!(apply(&mut store, "state=done;sticky=1"));
        assert!(one(&store).sticky);
        assert!(apply(&mut store, "state=error"));
        assert!(one(&store).sticky, "省略 sticky 时保留");

        assert!(apply(&mut store, "state=working"));
        assert!(!one(&store).sticky, "离开终态后必须清掉 sticky");
    }

    #[test]
    fn multiple_ids_merge_by_priority_and_keep_done_error_tie() {
        let mut store = ProgramStatusStore::default();
        assert!(apply(&mut store, "id=a;state=working"));
        assert!(apply(&mut store, "id=b;state=blocked"));
        let snapshot = store.current_snapshot();
        assert_eq!(
            snapshot.dominant(None).map(|entry| entry.id.as_str()),
            Some("b")
        );

        assert!(apply(&mut store, "id=b;state=done"));
        assert!(apply(&mut store, "id=c;state=error"));
        let snapshot = store.current_snapshot();
        let first = snapshot.dominant(None).map(|entry| entry.state).unwrap();
        assert!(matches!(first, ProgramState::Done | ProgramState::Error));
        let preferred = snapshot
            .dominant(Some(ProgramState::Error))
            .map(|entry| entry.state);
        assert_eq!(preferred, Some(ProgramState::Error));
    }

    #[test]
    fn title_and_status_truncate_on_utf8_boundaries() {
        let mut store = ProgramStatusStore::default();
        let title = "中".repeat(40); // 120 bytes
        let status = "文".repeat(60); // 180 bytes
        assert!(apply(
            &mut store,
            &format!("state=working;title={title};status={status}")
        ));
        let entry = one(&store);
        let title = entry.title.as_deref().unwrap();
        let status = entry.status.as_deref().unwrap();
        assert!(title.len() <= 64);
        assert!(status.len() <= 127);
        assert!(title.chars().all(|ch| ch == '中'));
        assert!(status.chars().all(|ch| ch == '文'));
    }

    #[test]
    fn snapshot_exports_once_but_current_snapshot_always_reads() {
        let mut store = ProgramStatusStore::default();
        assert!(apply(&mut store, "state=working"));
        assert!(store.snapshot().is_some());
        assert!(store.snapshot().is_none());
        assert_eq!(store.current_snapshot().entries.len(), 1);
        assert!(apply(&mut store, "state=blocked"));
        assert!(store.snapshot().is_some());
    }
}
