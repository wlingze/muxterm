//! GTK 测试注入用的前端注意力状态适配器。
//!
//! 真实 Runtime 的 activity 状态始终来自 `FfiClient`。这个小状态机只保留
//! 旧 GTK 测试钩子直接喂字节/agent 状态所需的兼容行为，避免测试钩子把
//! Core 类型带进 Linux frontend。

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use regex::Regex;

use crate::ffi_client::{
    ClientActivityNotification, ClientAttentionConfig, ClientAttentionPane, ClientAttentionStatus,
    ClientWorkspaceAttention,
};

const KNOWN_AGENTS: &[&str] = &[
    "codex", "cursor", "claude", "gemini", "aider", "opencode", "copilot", "cline", "goose", "amp",
    "grok", "windsurf", "kiro", "pi", "hermes", "droid",
];

#[derive(Debug, Clone, Copy)]
enum CompatEvent {
    CommandStart,
    CommandDone,
    AttentionRequest,
    UserInput,
    BecameVisible,
    RegexMatch,
}

#[derive(Debug)]
struct CompatibilityPane {
    pane: ClientAttentionPane,
    authoritative: bool,
    foreground_process: Option<String>,
    muted_until: Option<Instant>,
    last_regex_eval: Option<Instant>,
}

/// 前端仅为旧测试注入保留的 activity 视图。
#[derive(Debug)]
pub(crate) struct CompatibilityActivity {
    panes: HashMap<(String, u32), CompatibilityPane>,
    notified_blocked: HashSet<(String, u32)>,
    notified_done: HashSet<(String, u32)>,
    config: ClientAttentionConfig,
    regex_cache: HashMap<String, Option<Regex>>,
    next_seq: u64,
}

impl CompatibilityActivity {
    pub(crate) fn new(config: ClientAttentionConfig) -> Self {
        Self {
            panes: HashMap::new(),
            notified_blocked: HashSet::new(),
            notified_done: HashSet::new(),
            config,
            regex_cache: HashMap::new(),
            next_seq: 0,
        }
    }

    pub(crate) fn set_config(&mut self, config: ClientAttentionConfig) {
        self.config = config;
        self.regex_cache.clear();
    }

    fn entry_mut(&mut self, workspace_id: &str, pane_id: u32) -> &mut CompatibilityPane {
        self.panes
            .entry((workspace_id.to_string(), pane_id))
            .or_insert_with(|| CompatibilityPane {
                pane: ClientAttentionPane {
                    workspace_id: workspace_id.to_string(),
                    pane_id,
                    status: status_name(ClientAttentionStatus::Unknown).to_string(),
                    acknowledged: true,
                    last_line: String::new(),
                    seq: 0,
                    process_name: None,
                    process_is_agent: false,
                    agent_name: None,
                    shell_name: None,
                },
                authoritative: false,
                foreground_process: None,
                muted_until: None,
                last_regex_eval: None,
            })
    }

    /// 直接喂一批测试输出，模拟旧 GTK 注入钩子的最小 activity 语义。
    pub(crate) fn apply_output(
        &mut self,
        workspace_id: &str,
        pane_id: u32,
        bytes: &[u8],
        visible: bool,
    ) {
        self.next_seq = self.next_seq.saturating_add(1);
        let seq = self.next_seq;
        let text = String::from_utf8_lossy(bytes);
        let last_line = text
            .split('\n')
            .rev()
            .map(|line| line.trim_matches(|ch: char| ch == '\r' || ch.is_control()))
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default()
            .to_string();
        {
            let entry = self.entry_mut(workspace_id, pane_id);
            entry.pane.last_line = last_line.clone();
            entry.pane.seq = seq;
        }

        let command_start = b"\x1b]133;B\x07";
        let command_end = b"\x1b]133;C\x07";
        if let (Some(start), Some(end)) = (
            find_bytes(bytes, command_start).map(|index| index + command_start.len()),
            find_bytes(bytes, command_end),
        ) {
            if start <= end {
                let command = String::from_utf8_lossy(&bytes[start..end]);
                if !command.trim().is_empty() {
                    self.set_process_name(workspace_id, pane_id, Some(command.trim().to_string()));
                }
            }
        }

        let has_osc133 = find_bytes(bytes, b"\x1b]133;").is_some();
        if find_bytes(bytes, b"\x1b]133;D").is_some() {
            self.apply_event(workspace_id, pane_id, CompatEvent::CommandDone);
        } else if !has_osc133 && bytes.contains(&0x07) {
            self.apply_event(workspace_id, pane_id, CompatEvent::AttentionRequest);
        }

        self.maybe_eval_regex(workspace_id, pane_id, &last_line);
        if visible {
            self.on_became_visible(workspace_id, pane_id);
        }
    }

    pub(crate) fn set_agent_attention(
        &mut self,
        workspace_id: &str,
        pane_id: u32,
        process_name: &str,
        status: ClientAttentionStatus,
    ) {
        self.set_process_name(workspace_id, pane_id, Some(process_name.to_string()));
        let key = (workspace_id.to_string(), pane_id);
        let entry = self.entry_mut(workspace_id, pane_id);
        entry.authoritative = true;
        entry.pane.process_is_agent = true;
        entry.pane.agent_name = Some(process_name.to_string());
        self.set_status(&key, status);
    }

    pub(crate) fn on_user_input(&mut self, workspace_id: &str, pane_id: u32) {
        let key = (workspace_id.to_string(), pane_id);
        let Some(entry) = self.panes.get_mut(&key) else {
            return;
        };
        entry.pane.acknowledged = true;
        if !entry.authoritative {
            self.apply_event(workspace_id, pane_id, CompatEvent::UserInput);
        }
    }

    pub(crate) fn on_became_visible(&mut self, workspace_id: &str, pane_id: u32) {
        let key = (workspace_id.to_string(), pane_id);
        let Some(entry) = self.panes.get_mut(&key) else {
            return;
        };
        entry.pane.acknowledged = true;
        if !entry.authoritative {
            self.apply_event(workspace_id, pane_id, CompatEvent::BecameVisible);
        }
    }

    pub(crate) fn acknowledge(&mut self, workspace_id: &str, pane_id: u32) {
        let key = (workspace_id.to_string(), pane_id);
        let Some(entry) = self.panes.get_mut(&key) else {
            return;
        };
        entry.pane.acknowledged = true;
        if entry.authoritative {
            return;
        }
        match entry.pane.status_kind() {
            ClientAttentionStatus::Blocked | ClientAttentionStatus::Done => {
                self.set_status(&key, ClientAttentionStatus::Idle);
            }
            _ => {}
        }
    }

    pub(crate) fn mute_for(&mut self, workspace_id: &str, pane_id: u32, seconds: u64) {
        if let Some(entry) = self.panes.get_mut(&(workspace_id.to_string(), pane_id)) {
            entry.muted_until = Some(Instant::now() + Duration::from_secs(seconds));
        }
    }

    pub(crate) fn snapshot(&self) -> Vec<ClientWorkspaceAttention> {
        let mut groups: HashMap<String, Vec<(ClientAttentionPane, bool)>> = HashMap::new();
        for entry in self.panes.values() {
            groups
                .entry(entry.pane.workspace_id.clone())
                .or_default()
                .push((entry.pane.clone(), is_muted(entry)));
        }

        let mut workspaces = groups
            .into_iter()
            .map(|(workspace_id, mut panes)| {
                panes.sort_by_key(|(pane, _)| (pane.pane_id, pane.seq));
                let blocked = panes
                    .iter()
                    .filter(|(pane, muted)| {
                        !muted
                            && pane.status_kind() == ClientAttentionStatus::Blocked
                            && !pane.acknowledged
                    })
                    .count();
                let done = panes
                    .iter()
                    .filter(|(pane, muted)| {
                        !muted
                            && pane.status_kind() == ClientAttentionStatus::Done
                            && !pane.acknowledged
                    })
                    .count();
                let working = panes
                    .iter()
                    .filter(|(pane, _)| pane.status_kind() == ClientAttentionStatus::Working)
                    .count();
                ClientWorkspaceAttention {
                    workspace_id,
                    path: String::new(),
                    blocked,
                    done,
                    working,
                    panes: panes.into_iter().map(|(pane, _)| pane).collect(),
                }
            })
            .collect::<Vec<_>>();
        workspaces.sort_by(|left, right| left.workspace_id.cmp(&right.workspace_id));
        workspaces
    }

    pub(crate) fn take_notifications(&mut self) -> Vec<ClientActivityNotification> {
        let keys = self.panes.keys().cloned().collect::<Vec<_>>();
        let mut notifications = Vec::new();
        for key in keys {
            let Some(entry) = self.panes.get(&key) else {
                continue;
            };
            let muted = is_muted(entry);
            let status = entry.pane.status_kind();
            if status != ClientAttentionStatus::Blocked {
                self.notified_blocked.remove(&key);
            }
            if status != ClientAttentionStatus::Done {
                self.notified_done.remove(&key);
            }
            let (kind, notified) = match status {
                ClientAttentionStatus::Blocked => ("blocked", &mut self.notified_blocked),
                ClientAttentionStatus::Done => ("done", &mut self.notified_done),
                _ => continue,
            };
            if muted || entry.pane.acknowledged || !notified.insert(key.clone()) {
                continue;
            }
            notifications.push(ClientActivityNotification {
                workspace_id: entry.pane.workspace_id.clone(),
                pane_id: entry.pane.pane_id,
                kind: kind.to_string(),
                process_name: entry.pane.process_name.clone(),
                last_line: entry.pane.last_line.clone(),
                seq: entry.pane.seq,
            });
        }
        notifications.sort_by(|left, right| {
            left.workspace_id
                .cmp(&right.workspace_id)
                .then(left.pane_id.cmp(&right.pane_id))
                .then(left.seq.cmp(&right.seq))
        });
        notifications
    }

    fn apply_event(&mut self, workspace_id: &str, pane_id: u32, event: CompatEvent) {
        let key = (workspace_id.to_string(), pane_id);
        let current = self
            .panes
            .get(&key)
            .map(|entry| entry.pane.status_kind())
            .unwrap_or(ClientAttentionStatus::Unknown);
        let next = transition(current, event);
        self.set_status(&key, next);
    }

    fn set_status(&mut self, key: &(String, u32), status: ClientAttentionStatus) {
        let Some(entry) = self.panes.get_mut(key) else {
            return;
        };
        let previous = entry.pane.status_kind();
        entry.pane.status = status_name(status).to_string();
        if previous != status {
            entry.pane.acknowledged = !matches!(
                status,
                ClientAttentionStatus::Blocked | ClientAttentionStatus::Done
            );
        }
    }

    fn set_process_name(&mut self, workspace_id: &str, pane_id: u32, name: Option<String>) {
        let key = (workspace_id.to_string(), pane_id);
        let normalized = name.and_then(|value| normalize_process_name(&value));
        let previous = self
            .panes
            .get(&key)
            .and_then(|entry| entry.foreground_process.clone());
        let Some(next_process) = normalized.as_ref() else {
            if let Some(entry) = self.panes.get_mut(&key) {
                entry.foreground_process = None;
                if !entry.pane.process_is_agent {
                    entry.pane.process_name = None;
                }
            }
            return;
        };

        let shell = is_shell(next_process);
        let detected_agent = known_agent_process_name(next_process);
        let process_is_agent = detected_agent.is_some();
        {
            let entry = self.entry_mut(workspace_id, pane_id);
            entry.foreground_process = Some(next_process.clone());
            let initial_process = entry.pane.process_name.is_none();
            if shell {
                entry.pane.shell_name = Some(next_process.clone());
            }
            if !shell || initial_process {
                entry.pane.process_name = Some(next_process.clone());
                entry.pane.process_is_agent = process_is_agent;
            }
            if process_is_agent {
                entry.pane.agent_name = detected_agent
                    .map(str::to_string)
                    .or_else(|| Some(next_process.clone()));
            }
        }

        let previous_was_shell = previous.as_deref().map(is_shell).unwrap_or(true);
        let current_status = self
            .panes
            .get(&key)
            .map(|entry| entry.pane.status_kind())
            .unwrap_or(ClientAttentionStatus::Unknown);
        if !shell
            && (previous_was_shell
                || matches!(
                    current_status,
                    ClientAttentionStatus::Unknown
                        | ClientAttentionStatus::Idle
                        | ClientAttentionStatus::Done
                ))
        {
            self.apply_event(workspace_id, pane_id, CompatEvent::CommandStart);
        } else if previous.as_deref().is_some_and(|value| !is_shell(value))
            && shell
            && matches!(
                current_status,
                ClientAttentionStatus::Working | ClientAttentionStatus::Unknown
            )
        {
            self.apply_event(workspace_id, pane_id, CompatEvent::CommandDone);
        }
    }

    fn maybe_eval_regex(&mut self, workspace_id: &str, pane_id: u32, last_line: &str) {
        if !self.config.enabled || self.config.blocked_regex.is_empty() {
            return;
        }
        let key = (workspace_id.to_string(), pane_id);
        let now = Instant::now();
        let last_eval = self.panes.get(&key).and_then(|entry| entry.last_regex_eval);
        if last_eval.is_some_and(|at| {
            now.saturating_duration_since(at)
                < Duration::from_millis(self.config.debounce_ms.max(1))
        }) {
            return;
        }
        let patterns = self.config.blocked_regex.clone();
        let hit = patterns.iter().any(|pattern| {
            let regex = self.regex_cache.entry(pattern.clone()).or_insert_with(|| {
                match Regex::new(pattern) {
                    Ok(regex) => Some(regex),
                    Err(error) => {
                        tracing::warn!(target: "muxterm::linux", %error, "invalid compatibility attention regex");
                        None
                    }
                }
            });
            regex.as_ref().is_some_and(|regex| regex.is_match(last_line))
        });
        if let Some(entry) = self.panes.get_mut(&key) {
            entry.last_regex_eval = Some(now);
        }
        if hit {
            self.apply_event(workspace_id, pane_id, CompatEvent::RegexMatch);
        }
    }
}

fn is_muted(entry: &CompatibilityPane) -> bool {
    entry
        .muted_until
        .is_some_and(|until| until > Instant::now())
}

fn status_name(status: ClientAttentionStatus) -> &'static str {
    match status {
        ClientAttentionStatus::Unknown => "unknown",
        ClientAttentionStatus::Working => "working",
        ClientAttentionStatus::Done => "done",
        ClientAttentionStatus::Blocked => "blocked",
        ClientAttentionStatus::Idle => "idle",
    }
}

fn transition(status: ClientAttentionStatus, event: CompatEvent) -> ClientAttentionStatus {
    use ClientAttentionStatus::*;
    match (status, event) {
        (Unknown, CompatEvent::CommandStart) => Working,
        (Unknown, CompatEvent::CommandDone) => Done,
        (Unknown, CompatEvent::AttentionRequest) => Blocked,
        (Unknown, CompatEvent::RegexMatch) => Blocked,
        (Idle, CompatEvent::CommandStart) => Working,
        (Idle, CompatEvent::CommandDone) => Done,
        (Idle, CompatEvent::AttentionRequest) => Blocked,
        (Idle, CompatEvent::RegexMatch) => Blocked,
        (Working, CompatEvent::CommandStart) => Working,
        (Working, CompatEvent::CommandDone) => Done,
        (Working, CompatEvent::AttentionRequest) => Blocked,
        (Working, CompatEvent::RegexMatch) => Blocked,
        (Done, CompatEvent::CommandStart) => Working,
        (Done, CompatEvent::CommandDone) => Done,
        (Done, CompatEvent::AttentionRequest) => Blocked,
        (Done, CompatEvent::BecameVisible) => Idle,
        (Done, CompatEvent::RegexMatch) => Blocked,
        (Blocked, CompatEvent::CommandStart) => Working,
        (Blocked, CompatEvent::CommandDone) => Done,
        (Blocked, CompatEvent::AttentionRequest) => Blocked,
        (Blocked, CompatEvent::UserInput) => Idle,
        (Blocked, CompatEvent::RegexMatch) => Blocked,
        _ => status,
    }
}

fn known_agent_process_name(value: &str) -> Option<&'static str> {
    value
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')))
        .filter(|token| !token.is_empty())
        .find_map(|token| {
            let lower = token.to_ascii_lowercase();
            KNOWN_AGENTS.iter().find_map(|agent| {
                (lower == *agent
                    || lower.starts_with(&format!("{agent}-"))
                    || lower.starts_with(&format!("{agent}_")))
                .then_some(*agent)
            })
        })
}

fn normalize_process_name(name: &str) -> Option<String> {
    let value = name.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(agent) = known_agent_process_name(value) {
        return Some(agent.to_string());
    }
    let split = value
        .char_indices()
        .find(|(_, ch)| ch.is_whitespace())
        .map(|(index, _)| index)
        .unwrap_or(value.len());
    let executable = value[..split]
        .trim_matches(|ch: char| ch == '\'' || ch == '"')
        .trim_start_matches('-');
    let basename = executable.rsplit(['/', '\\']).next().unwrap_or(executable);
    let arguments = value[split..].trim_start();
    if arguments.is_empty() {
        Some(basename.to_string())
    } else {
        Some(format!("{basename} {arguments}"))
    }
}

fn is_shell(command: &str) -> bool {
    let mut words = command.split_whitespace();
    let Some(executable) = words.next() else {
        return false;
    };
    let base = executable
        .trim_matches(|ch| ch == '\'' || ch == '"')
        .trim_start_matches('-')
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(executable)
        .to_ascii_lowercase();
    let shell = matches!(
        base.as_str(),
        "zsh" | "bash" | "sh" | "fish" | "tcsh" | "csh" | "dash" | "ksh"
    );
    shell
        && !words.any(|arg| {
            arg == "-c"
                || arg == "--command"
                || (arg.starts_with('-')
                    && !arg.starts_with("--")
                    && arg.chars().skip(1).any(|ch| ch == 'c'))
        })
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    (!needle.is_empty())
        .then(|| {
            haystack
                .windows(needle.len())
                .position(|window| window == needle)
        })
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn activity() -> CompatibilityActivity {
        CompatibilityActivity::new(ClientAttentionConfig::default())
    }

    #[test]
    fn background_bel_is_blocked_and_notified_once() {
        let mut activity = activity();
        activity.apply_output("ws", 1, b"hello\r\n\x07", false);
        let snapshot = activity.snapshot();
        assert_eq!(snapshot[0].blocked, 1);
        assert_eq!(activity.take_notifications().len(), 1);
        assert!(activity.take_notifications().is_empty());
    }

    #[test]
    fn visible_done_is_read_but_visible_blocked_stays_blocked() {
        let mut activity = activity();
        activity.apply_output("ws", 1, b"\x1b]133;D;0\x07", true);
        assert_eq!(activity.snapshot()[0].done, 0);
        activity.apply_output("ws", 1, b"\x07", false);
        assert_eq!(activity.snapshot()[0].blocked, 1);
    }

    #[test]
    fn authoritative_agent_ignores_user_input_status_clear() {
        let mut activity = activity();
        activity.set_agent_attention("ws", 1, "codex", ClientAttentionStatus::Blocked);
        activity.on_user_input("ws", 1);
        let pane = &activity.snapshot()[0].panes[0];
        assert_eq!(pane.status_kind(), ClientAttentionStatus::Blocked);
        assert!(pane.acknowledged);
    }
}
