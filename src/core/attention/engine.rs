//! AttentionEngine：把信号/输入/可见性应用到 pane，聚合工作区。
//!
//! 红点计数 = **blocked 工作区数**（不是 pane 数、不是未读行数）。
//! 正则只在 `last_line` 变化且距上次求值超过 debounce_ms 时评估；
//! 非法正则跳过该条（启动解析失败记 tracing，不 panic）。

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use regex::Regex;

use super::clock::Clock;
use super::signal::AttentionSignal;
use super::state::{transition, PaneEvent, PaneStatus};
use crate::core::config::AttentionConfig;

const KNOWN_AGENTS: &[&str] = &[
    "codex", "cursor", "claude", "gemini", "aider", "opencode", "copilot", "cline", "goose", "amp",
    "grok", "windsurf", "kiro", "pi", "hermes",
];

/// 从进程名/argv 中识别通用 agent 名；不依赖具体 Runtime。
pub fn known_agent_process_name(value: &str) -> Option<&'static str> {
    for token in value.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))) {
        if token.is_empty() {
            continue;
        }
        let lower = token.to_ascii_lowercase();
        if let Some(agent) = KNOWN_AGENTS.iter().find(|agent| {
            lower == **agent
                || lower.starts_with(&format!("{}-", agent))
                || lower.starts_with(&format!("{}_", agent))
        }) {
            return Some(*agent);
        }
    }
    None
}

/// 单个 pane 的注意力状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneAttention {
    pub workspace_id: String,
    pub pane_id: u32,
    pub status: PaneStatus,
    /// 用户是否已经查看/处理了当前 Blocked 或 Done 状态。
    /// Runtime 权威状态可以继续保持 Blocked/Done；该位只表达 UI 未读语义。
    pub acknowledged: bool,
    pub last_line: String,
    pub seq: u64,
    /// 当前或最近一次命令的展示名。
    pub process_name: Option<String>,
    /// `process_name` 对应的命令是否被识别为 agent。
    pub process_is_agent: bool,
    /// 该 pane 最近识别到的 agent；已读或 Runtime 释放 authority 后仍保留。
    pub agent_name: Option<String>,
    /// 最近看到的交互 shell，用于已读普通命令回退到稳定的 shell 行。
    pub shell_name: Option<String>,
    pub mute_until: Option<Instant>,
    pub last_regex_eval: Instant,
}

/// 工作区聚合视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAttention {
    pub workspace_id: String,
    /// 该工作区 blocked pane 数（列表用）。
    pub blocked: usize,
    pub done: usize,
    pub working: usize,
    pub panes: Vec<PaneAttention>,
}

/// 一次需要展示给前端的 pane 通知。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionNotification {
    pub workspace_id: String,
    pub pane_id: u32,
    pub kind: AttentionNotificationKind,
    pub process_name: Option<String>,
    pub last_line: String,
    pub seq: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttentionNotificationKind {
    Blocked,
    Done,
}

/// 注意力引擎（跨工作区聚合）。
pub struct AttentionEngine<C: Clock> {
    panes: HashMap<(String, u32), PaneAttention>,
    config: AttentionConfig,
    clock: C,
    /// 已对哪些 workspace 发过「进入 blocked」通知（清除后删除，重亮再通知）。
    notified_blocked: HashSet<String>,
    /// 已对哪些 workspace 发过「后台任务完成」通知（状态离开 Done 后删除）。
    notified_done: HashSet<String>,
    /// 结构化通知按 pane 去重；workspace 级旧 API 保留兼容。
    notified_blocked_panes: HashSet<(String, u32)>,
    notified_done_panes: HashSet<(String, u32)>,
    /// Runtime 已提供结构化权威状态的 pane；这些 pane 的 OSC/BEL/正则只
    /// 继续更新文本索引，不覆盖 agent lifecycle。
    authoritative_panes: HashSet<(String, u32)>,
    /// pane-cmd 上一次实际看到的前台进程。展示名称会在回到 shell 后
    /// 保留 agent 名用于 Done 行，所以生命周期边沿不能反查展示字段。
    foreground_processes: HashMap<(String, u32), String>,
    /// 正则缓存：编译失败时记录该条并跳过。
    regex_cache: HashMap<String, Option<Regex>>,
}

impl<C: Clock> AttentionEngine<C> {
    pub fn new(config: AttentionConfig, clock: C) -> Self {
        Self {
            panes: HashMap::new(),
            config,
            clock,
            notified_blocked: HashSet::new(),
            notified_done: HashSet::new(),
            notified_blocked_panes: HashSet::new(),
            notified_done_panes: HashSet::new(),
            authoritative_panes: HashSet::new(),
            foreground_processes: HashMap::new(),
            regex_cache: HashMap::new(),
        }
    }

    pub fn set_config(&mut self, config: AttentionConfig) {
        self.config = config;
        self.regex_cache.clear();
    }

    fn entry_mut(&mut self, ws: &str, pane: u32) -> &mut PaneAttention {
        let now = self.clock.now();
        self.panes
            .entry((ws.to_string(), pane))
            .or_insert_with(|| PaneAttention {
                workspace_id: ws.to_string(),
                pane_id: pane,
                status: PaneStatus::Unknown,
                acknowledged: true,
                last_line: String::new(),
                seq: 0,
                process_name: None,
                process_is_agent: false,
                agent_name: None,
                shell_name: None,
                mute_until: None,
                // 初始化为久远过去，保证第一条输出就参与正则评估。
                last_regex_eval: now.checked_sub(Duration::from_secs(3600)).unwrap_or(now),
            })
    }

    /// 应用一条 pane 输出产生的信号 + 最新行/seq。
    pub fn apply(
        &mut self,
        ws: &str,
        pane: u32,
        signals: &[AttentionSignal],
        last_line: &str,
        seq: u64,
    ) {
        let now = self.clock.now();
        let key = (ws.to_string(), pane);
        let mut authoritative = self.authoritative_panes.contains(&key);
        let (mut status, mut acknowledged) = {
            let entry = self.entry_mut(ws, pane);
            (entry.status, entry.acknowledged)
        };
        let previous_status = status;
        let mut initial_status = None;

        for sig in signals {
            match sig {
                AttentionSignal::AuthoritativeStatus {
                    status: next,
                    initial,
                } => {
                    authoritative = true;
                    if *initial {
                        initial_status = Some(*next);
                    } else if status != *next {
                        // 同一轮里 bootstrap 之后又发生了真实转换时，
                        // bootstrap 只抑制自己的状态，不能吞掉最终转换。
                        initial_status = None;
                    }
                    status = *next;
                }
                AttentionSignal::ClearAuthoritativeStatus => {
                    authoritative = false;
                    status = PaneStatus::Unknown;
                    initial_status = None;
                }
                AttentionSignal::CommandStart if !authoritative => {
                    status = transition(status, PaneEvent::CommandStart);
                }
                AttentionSignal::CommandDone { exit_code } if !authoritative => {
                    status = transition(
                        status,
                        PaneEvent::CommandDone {
                            exit_code: *exit_code,
                        },
                    );
                }
                AttentionSignal::AttentionRequest { .. } if !authoritative => {
                    status = transition(status, PaneEvent::AttentionRequest);
                }
                _ => {}
            }
        }

        if authoritative {
            self.authoritative_panes.insert(key.clone());
        } else {
            self.authoritative_panes.remove(&key);
        }
        {
            let entry = self.entry_mut(ws, pane);
            entry.last_line = last_line.to_string();
            entry.seq = seq;
            entry.status = status;
            if status != previous_status {
                acknowledged = !matches!(status, PaneStatus::Blocked | PaneStatus::Done);
            }
            entry.acknowledged = acknowledged;
        }
        // Runtime bootstrap（attach 或 authority handoff）只播种现状。把
        // 初始 blocked/done 记为已经通知，防止伪造一条「新转换」通知；
        // workspace 级与 pane 级两级去重同时预置。
        match initial_status {
            Some(PaneStatus::Blocked) => {
                self.notified_blocked.insert(ws.to_string());
                self.notified_blocked_panes.insert((ws.to_string(), pane));
            }
            Some(PaneStatus::Done) => {
                self.notified_done.insert(ws.to_string());
                self.notified_done_panes.insert((ws.to_string(), pane));
            }
            _ => {}
        }
        self.maybe_eval_regex(ws, pane, now);
        self.sync_notified(ws, pane, now);
    }

    /// 用户输入：Blocked → Idle（输入才算处理）。
    pub fn on_user_input(&mut self, ws: &str, pane: u32) {
        let now = self.clock.now();
        self.entry_mut(ws, pane).acknowledged = true;
        if self.authoritative_panes.contains(&(ws.to_string(), pane)) {
            return;
        }
        let entry = self.entry_mut(ws, pane);
        entry.status = transition(entry.status, PaneEvent::UserInput);
        self.sync_notified(ws, pane, now);
    }

    /// 该 pane 成为前台可见（仅当前台 pane，后台不触发）。
    pub fn on_became_visible(&mut self, ws: &str, pane: u32) {
        let now = self.clock.now();
        self.entry_mut(ws, pane).acknowledged = true;
        if self.authoritative_panes.contains(&(ws.to_string(), pane)) {
            return;
        }
        let entry = self.entry_mut(ws, pane);
        entry.status = transition(entry.status, PaneEvent::BecameVisible);
        self.sync_notified(ws, pane, now);
    }

    /// 用户从通知跳转/打开 pane 后确认已读。
    ///
    /// Blocked 仍遵循“输入才清除”的状态语义，Done 遵循“变为可见即清除”；
    /// 该显式入口只把两种已列出的状态转换为 Idle，不影响 Working。
    pub fn acknowledge(&mut self, ws: &str, pane: u32) {
        let now = self.clock.now();
        self.entry_mut(ws, pane).acknowledged = true;
        if self.authoritative_panes.contains(&(ws.to_string(), pane)) {
            return;
        }
        let entry = self.entry_mut(ws, pane);
        entry.status = match entry.status {
            PaneStatus::Blocked => transition(entry.status, PaneEvent::UserInput),
            PaneStatus::Done => transition(entry.status, PaneEvent::BecameVisible),
            status => status,
        };
        self.sync_notified(ws, pane, now);
    }

    /// 是否只是空闲交互 shell（pane-cmd 回到它 = 命令结束）。
    /// `bash -c ...` / `zsh -lc ...` 是实际命令，不能当作空闲容器。
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

    /// 更新 pane 进程名；非 shell 进程名可作 Working 粗判来源（注释见 LINUX-PLAN §9）。
    ///
    /// W19：pane-cmd 从非 shell（sleep/cat 等）回到交互 shell（zsh/bash）时，
    /// 视为后台命令结束 → CommandDone（OSC 133 D 之外的兜底）。只对
    /// Working/Idle/Unknown 生效，不覆盖 Blocked（输入才熄）。
    pub fn set_process_name(&mut self, ws: &str, pane: u32, name: Option<String>) {
        self.set_process_name_with_kind(ws, pane, name, false);
    }

    /// Runtime 结构化识别出的 agent。GUI 只调用这份通用产品接口，不按
    /// Runtime 名字分支；agent identity 会一直保留到 pane 被关闭。
    pub fn set_agent_process_name(&mut self, ws: &str, pane: u32, name: Option<String>) {
        self.set_process_name_with_kind(ws, pane, name, true);
    }

    fn set_process_name_with_kind(
        &mut self,
        ws: &str,
        pane: u32,
        name: Option<String>,
        authoritative_agent: bool,
    ) {
        let now = self.clock.now();
        let key = (ws.to_string(), pane);
        let previous = self.foreground_processes.get(&key).cloned();
        let normalized = name.and_then(|value| Self::normalize_process_name(&value));
        let Some(next_process) = normalized.as_ref() else {
            self.foreground_processes.remove(&key);
            let preserve_current_agent = {
                let entry = self.entry_mut(ws, pane);
                authoritative_agent || entry.process_is_agent
            };
            if !preserve_current_agent {
                let entry = self.entry_mut(ws, pane);
                entry.process_name = None;
                entry.process_is_agent = false;
            }
            self.sync_notified(ws, pane, now);
            return;
        };
        self.foreground_processes
            .insert(key.clone(), next_process.clone());
        let is_shell = Self::is_shell(next_process);
        let detected_agent = known_agent_process_name(next_process);
        let process_is_agent = authoritative_agent || detected_agent.is_some();
        {
            let entry = self.entry_mut(ws, pane);
            // shell 只是容器，不应覆盖刚完成的 codex/cursor/agent 名称。
            // 但首次订阅通常先到 shell（zsh/bash）；记录它作为竞态期间的
            // 可靠兜底，避免 Attention 行在后台 Done 先到时显示成 `?`。
            let is_initial_process = entry.process_name.is_none();
            if is_shell {
                entry.shell_name = normalized.clone();
            }
            if !is_shell || is_initial_process {
                entry.process_name = normalized.clone();
                entry.process_is_agent = process_is_agent;
            }
            if process_is_agent {
                entry.agent_name = Some(
                    detected_agent
                        .map(str::to_string)
                        .unwrap_or_else(|| next_process.clone()),
                );
            }
        }
        let starts_command = !is_shell
            && (previous.as_deref().is_none_or(Self::is_shell)
                || matches!(
                    self.panes.get(&key).map(|pane| pane.status),
                    Some(PaneStatus::Unknown | PaneStatus::Idle | PaneStatus::Done)
                ));
        if starts_command {
            let (last_line, seq) = self
                .panes
                .get(&key)
                .map(|entry| (entry.last_line.clone(), entry.seq))
                .unwrap_or_default();
            self.apply(ws, pane, &[AttentionSignal::CommandStart], &last_line, seq);
            return;
        }
        if let (Some(prev), Some(next)) = (previous.as_deref(), normalized.as_deref()) {
            if !Self::is_shell(prev) && Self::is_shell(next) {
                let shell_ok = {
                    let entry = self.panes.get(&key);
                    // 只对 Working/Unknown 生效：Idle 是被前台可见清掉的
                    // （on_became_visible），不能又被 pane-cmd 点亮成 Done。
                    matches!(
                        entry.map(|p| p.status),
                        Some(PaneStatus::Working | PaneStatus::Unknown)
                    )
                };
                if shell_ok {
                    let (last_line, seq) = self
                        .panes
                        .get(&key)
                        .map(|entry| (entry.last_line.clone(), entry.seq))
                        .unwrap_or_default();
                    self.apply(
                        ws,
                        pane,
                        &[AttentionSignal::CommandDone { exit_code: None }],
                        &last_line,
                        seq,
                    );
                    return;
                }
            }
        }
        let _ = now;
    }

    fn normalize_process_name(name: &str) -> Option<String> {
        let value = name.trim();
        if value.is_empty() {
            return None;
        }
        // tmux 通常只给 pane_current_command（例如 `node`），但某些
        // transports/fixtures 会传完整 argv。优先从整条命令中识别 agent，
        // 避免把 npx/node/wrapper 当成用户真正执行的 Codex/Cursor。
        if let Some(agent) = known_agent_process_name(value) {
            return Some(agent.to_string());
        }
        let split = value
            .char_indices()
            .find(|(_, ch)| ch.is_whitespace())
            .map(|(index, _)| index)
            .unwrap_or(value.len());
        let executable = value[..split]
            .trim_matches(|c: char| c == '\'' || c == '"')
            .trim_start_matches('-');
        let basename = executable.rsplit(['/', '\\']).next().unwrap_or(executable);
        let arguments = value[split..].trim_start();
        if arguments.is_empty() {
            Some(basename.to_string())
        } else {
            Some(format!("{basename} {arguments}"))
        }
    }

    /// pane 已从 Workspace 拓扑删除：同时清理状态、权威来源和 workspace
    /// 通知去重，避免关闭的 blocked/done agent 永久留在红点里。
    pub fn remove_pane(&mut self, ws: &str, pane: u32) {
        let now = self.clock.now();
        let key = (ws.to_string(), pane);
        self.panes.remove(&key);
        self.authoritative_panes.remove(&key);
        self.foreground_processes.remove(&key);
        self.sync_notified(ws, pane, now);
    }

    /// 静音一段时间：不进红点、不通知；peek/答复仍可用。
    pub fn mute_for(&mut self, ws: &str, pane: u32, d: Duration) {
        let now = self.clock.now();
        let entry = self.entry_mut(ws, pane);
        entry.mute_until = Some(now + d);
        self.sync_notified(ws, pane, now);
    }

    /// 工作区聚合快照。
    pub fn snapshot(&self) -> Vec<WorkspaceAttention> {
        let mut map: HashMap<String, Vec<&PaneAttention>> = HashMap::new();
        for p in self.panes.values() {
            map.entry(p.workspace_id.clone()).or_default().push(p);
        }
        let mut out: Vec<WorkspaceAttention> = map
            .into_iter()
            .map(|(workspace_id, panes)| {
                let blocked = panes
                    .iter()
                    .filter(|p| p.status == PaneStatus::Blocked)
                    .count();
                let done = panes
                    .iter()
                    .filter(|p| p.status == PaneStatus::Done)
                    .count();
                let working = panes
                    .iter()
                    .filter(|p| p.status == PaneStatus::Working)
                    .count();
                let mut panes = panes.into_iter().cloned().collect::<Vec<_>>();
                panes.sort_by_key(|p| (p.pane_id, p.seq));
                WorkspaceAttention {
                    workspace_id,
                    blocked,
                    done,
                    working,
                    panes,
                }
            })
            .collect();
        out.sort_by(|a, b| a.workspace_id.cmp(&b.workspace_id));
        out
    }

    /// 红点 N：mute 未到期的 blocked **工作区**数。
    pub fn blocked_workspace_count(&self) -> usize {
        let now = self.clock.now();
        let mut ws: HashSet<&str> = HashSet::new();
        for p in self.panes.values() {
            if p.status == PaneStatus::Blocked && !p.mute_until.map(|m| m > now).unwrap_or(false) {
                ws.insert(p.workspace_id.as_str());
            }
        }
        ws.len()
    }

    /// 取走本轮新进入 blocked 的 workspace（保持期间不重复；清除后重亮再通知）。
    pub fn take_new_blocked_notifications(&mut self) -> Vec<String> {
        let now = self.clock.now();
        let mut out = Vec::new();
        for p in self.panes.values() {
            if p.status == PaneStatus::Blocked
                && !p.mute_until.map(|m| m > now).unwrap_or(false)
                && !self.notified_blocked.contains(&p.workspace_id)
            {
                self.notified_blocked.insert(p.workspace_id.clone());
                out.push(p.workspace_id.clone());
            }
        }
        out.sort();
        out
    }

    /// 取走本轮新进入 Done 的 workspace（后台 pane 任务完成；保持期间不重复）。
    ///
    /// 前台 pane 的 Done 会被 `on_became_visible` 清成 Idle，所以这里只
    /// 剩后台 pane 的完成事件。
    pub fn take_new_done_notifications(&mut self) -> Vec<String> {
        let now = self.clock.now();
        let mut out = Vec::new();
        for p in self.panes.values() {
            if p.status == PaneStatus::Done
                && !p.mute_until.map(|m| m > now).unwrap_or(false)
                && !self.notified_done.contains(&p.workspace_id)
            {
                self.notified_done.insert(p.workspace_id.clone());
                out.push(p.workspace_id.clone());
            }
        }
        out.sort();
        out
    }

    /// 取走结构化 pane 通知；Blocked/Done 分别按 pane 去重。
    pub fn take_notifications(&mut self) -> Vec<AttentionNotification> {
        let now = self.clock.now();
        let mut out = Vec::new();
        for pane in self.panes.values() {
            let key = (pane.workspace_id.clone(), pane.pane_id);
            let muted = pane.mute_until.map(|m| m > now).unwrap_or(false);
            match pane.status {
                PaneStatus::Blocked if !muted && !self.notified_blocked_panes.contains(&key) => {
                    self.notified_blocked_panes.insert(key);
                    out.push(AttentionNotification {
                        workspace_id: pane.workspace_id.clone(),
                        pane_id: pane.pane_id,
                        kind: AttentionNotificationKind::Blocked,
                        process_name: pane.process_name.clone(),
                        last_line: pane.last_line.clone(),
                        seq: pane.seq,
                    });
                }
                PaneStatus::Done if !muted && !self.notified_done_panes.contains(&key) => {
                    self.notified_done_panes.insert(key);
                    out.push(AttentionNotification {
                        workspace_id: pane.workspace_id.clone(),
                        pane_id: pane.pane_id,
                        kind: AttentionNotificationKind::Done,
                        process_name: pane.process_name.clone(),
                        last_line: pane.last_line.clone(),
                        seq: pane.seq,
                    });
                }
                _ => {}
            }
        }
        out.sort_by(|a, b| {
            a.workspace_id
                .cmp(&b.workspace_id)
                .then(a.pane_id.cmp(&b.pane_id))
                .then(a.seq.cmp(&b.seq))
        });
        out
    }

    fn maybe_eval_regex(&mut self, ws: &str, pane: u32, now: Instant) {
        if self.authoritative_panes.contains(&(ws.to_string(), pane)) {
            return;
        }
        let (enabled, patterns, debounce) = {
            (
                self.config.enabled,
                self.config.blocked_regex.clone(),
                self.config.debounce_ms.max(1),
            )
        };
        if !enabled || patterns.is_empty() {
            return;
        }
        let last_eval = self
            .panes
            .get(&(ws.to_string(), pane))
            .map(|p| p.last_regex_eval)
            .unwrap_or(now);
        if now.saturating_duration_since(last_eval) < Duration::from_millis(debounce) {
            return;
        }
        let line = self
            .panes
            .get(&(ws.to_string(), pane))
            .map(|p| p.last_line.clone())
            .unwrap_or_default();
        let mut hit = false;
        for pattern in &patterns {
            let rx = self.regex_cache.entry(pattern.clone()).or_insert_with(|| {
                match Regex::new(pattern) {
                    Ok(r) => Some(r),
                    Err(e) => {
                        tracing::warn!(target = "muxterm::attention", "非法 blocked_regex: {e}");
                        None
                    }
                }
            });
            if let Some(rx) = rx {
                if rx.is_match(&line) {
                    hit = true;
                    break;
                }
            }
        }
        let entry = self.entry_mut(ws, pane);
        entry.last_regex_eval = now;
        if hit {
            entry.status = transition(entry.status, PaneEvent::RegexMatch);
        }
    }

    /// 同步通知去重标记（两级）：
    /// - pane 级：pane 离开对应状态（或静音）就清自己的去重标记；
    /// - workspace 级：只有整个 workspace 都不再有未静音的对应状态 pane
    ///   时，才允许该 workspace 下一次重新通知（另一个 idle pane 的
    ///   转换不能重置仍 blocked pane 的 workspace 标记）。
    ///
    /// pane 可能已被 remove_pane 移除：此时只做 workspace 级聚合清理。
    fn sync_notified(&mut self, ws: &str, pane: u32, now: Instant) {
        if let Some(entry) = self.panes.get(&(ws.to_string(), pane)) {
            let muted = entry.mute_until.map(|m| m > now).unwrap_or(false);
            if entry.status != PaneStatus::Blocked || muted {
                self.notified_blocked_panes.remove(&(ws.to_string(), pane));
            }
            if entry.status != PaneStatus::Done || muted {
                self.notified_done_panes.remove(&(ws.to_string(), pane));
            }
        }
        let has_unmuted = |status| {
            self.panes.values().any(|entry| {
                entry.workspace_id == ws
                    && entry.status == status
                    && !entry.mute_until.map(|m| m > now).unwrap_or(false)
            })
        };
        if !has_unmuted(PaneStatus::Blocked) {
            self.notified_blocked.remove(ws);
        }
        if !has_unmuted(PaneStatus::Done) {
            self.notified_done.remove(ws);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::attention::clock::FakeClock;
    use crate::core::attention::signal::AttentionSource;

    fn clock() -> FakeClock {
        FakeClock::new(Instant::now())
    }

    #[test]
    fn bel_storm_one_transition() {
        let mut c = clock();
        let mut e = AttentionEngine::new(AttentionConfig::default(), c.clone());
        for _ in 0..50 {
            e.apply(
                "ws",
                1,
                &[AttentionSignal::AttentionRequest {
                    source: AttentionSource::Bel,
                }],
                "line",
                1,
            );
        }
        assert_eq!(e.blocked_workspace_count(), 1);
        let _ = &mut c;
    }

    #[test]
    fn regex_debounced() {
        let mut c = clock();
        let cfg = AttentionConfig {
            blocked_regex: vec!["ask".into()],
            debounce_ms: 50,
            ..AttentionConfig::default()
        };
        let mut e = AttentionEngine::new(cfg, c.clone());
        e.apply("ws", 1, &[], "ask1", 1);
        assert_eq!(e.blocked_workspace_count(), 1);
        // 未过 debounce：换行不再评估（不重亮/不变动）
        e.apply("ws", 1, &[], "ask2", 2);
        assert_eq!(e.blocked_workspace_count(), 1);
        let _ = &mut c;
    }

    #[test]
    fn mute_excludes_from_badge_count() {
        let mut e = AttentionEngine::new(AttentionConfig::default(), clock());
        e.apply(
            "ws",
            1,
            &[AttentionSignal::AttentionRequest {
                source: AttentionSource::Bel,
            }],
            "line",
            1,
        );
        assert_eq!(e.blocked_workspace_count(), 1);
        e.mute_for("ws", 1, Duration::from_secs(3600));
        assert_eq!(e.blocked_workspace_count(), 0);
    }

    #[test]
    fn notify_once_per_blocked_entry() {
        let mut e = AttentionEngine::new(AttentionConfig::default(), clock());
        e.apply(
            "ws",
            1,
            &[AttentionSignal::AttentionRequest {
                source: AttentionSource::Bel,
            }],
            "line",
            1,
        );
        assert_eq!(e.take_new_blocked_notifications(), vec!["ws"]);
        assert!(e.take_new_blocked_notifications().is_empty());
        // 输入清除后再 BEL → 再通知一次
        e.on_user_input("ws", 1);
        e.apply(
            "ws",
            1,
            &[AttentionSignal::AttentionRequest {
                source: AttentionSource::Bel,
            }],
            "line",
            2,
        );
        assert_eq!(e.take_new_blocked_notifications(), vec!["ws"]);
    }

    #[test]
    fn aggregate_two_panes_one_workspace() {
        let mut e = AttentionEngine::new(AttentionConfig::default(), clock());
        e.apply(
            "ws",
            1,
            &[AttentionSignal::AttentionRequest {
                source: AttentionSource::Bel,
            }],
            "a",
            1,
        );
        e.apply(
            "ws",
            2,
            &[AttentionSignal::AttentionRequest {
                source: AttentionSource::Bel,
            }],
            "b",
            2,
        );
        // 红点 N=1 不是 2
        assert_eq!(e.blocked_workspace_count(), 1);
        let snap = e.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].blocked, 2);
    }

    #[test]
    fn idle_pane_does_not_rearm_blocked_workspace_notification() {
        let mut e = AttentionEngine::new(AttentionConfig::default(), clock());
        e.apply(
            "ws",
            1,
            &[AttentionSignal::AttentionRequest {
                source: AttentionSource::Bel,
            }],
            "blocked",
            1,
        );
        assert_eq!(e.take_new_blocked_notifications(), vec!["ws"]);

        // 同一 workspace 的另一个 pane 没有 blocked，不能把 workspace 级
        // 去重标记清掉，否则 pane 1 未离开 blocked 也会重复通知。
        e.apply("ws", 2, &[], "idle", 1);
        assert!(e.take_new_blocked_notifications().is_empty());
    }

    #[test]
    fn initial_authoritative_blocked_and_done_are_not_notifications() {
        let mut e = AttentionEngine::new(AttentionConfig::default(), clock());
        e.apply(
            "blocked-ws",
            1,
            &[AttentionSignal::AuthoritativeStatus {
                status: PaneStatus::Blocked,
                initial: true,
            }],
            "waiting",
            1,
        );
        e.apply("blocked-ws", 2, &[], "shell", 1);
        e.apply(
            "done-ws",
            1,
            &[AttentionSignal::AuthoritativeStatus {
                status: PaneStatus::Done,
                initial: true,
            }],
            "finished",
            1,
        );
        e.apply("done-ws", 2, &[], "shell", 1);

        assert!(e.take_new_blocked_notifications().is_empty());
        assert!(e.take_new_done_notifications().is_empty());
    }

    #[test]
    fn authoritative_transition_notifies_once_and_ignores_heuristics() {
        let cfg = AttentionConfig {
            blocked_regex: vec!["approve".into()],
            ..AttentionConfig::default()
        };
        let mut e = AttentionEngine::new(cfg, clock());
        e.apply(
            "ws",
            1,
            &[AttentionSignal::AuthoritativeStatus {
                status: PaneStatus::Working,
                initial: true,
            }],
            "running",
            1,
        );
        e.apply(
            "ws",
            1,
            &[
                AttentionSignal::AttentionRequest {
                    source: AttentionSource::Bel,
                },
                AttentionSignal::CommandDone { exit_code: Some(0) },
            ],
            "approve now",
            2,
        );
        assert_eq!(e.snapshot()[0].panes[0].status, PaneStatus::Working);
        assert!(e.take_new_blocked_notifications().is_empty());
        assert!(e.take_new_done_notifications().is_empty());

        e.apply(
            "ws",
            1,
            &[AttentionSignal::AuthoritativeStatus {
                status: PaneStatus::Blocked,
                initial: false,
            }],
            "approve now",
            3,
        );
        assert_eq!(e.take_new_blocked_notifications(), vec!["ws"]);
        assert!(e.take_new_blocked_notifications().is_empty());

        e.apply(
            "ws",
            1,
            &[AttentionSignal::AuthoritativeStatus {
                status: PaneStatus::Done,
                initial: false,
            }],
            "finished",
            4,
        );
        assert_eq!(e.take_new_done_notifications(), vec!["ws"]);
        assert!(e.take_new_done_notifications().is_empty());
    }

    #[test]
    fn coalesced_source_handoff_does_not_hide_final_done_transition() {
        let mut e = AttentionEngine::new(AttentionConfig::default(), clock());
        e.apply(
            "ws",
            1,
            &[AttentionSignal::AuthoritativeStatus {
                status: PaneStatus::Blocked,
                initial: false,
            }],
            "waiting",
            1,
        );
        assert_eq!(e.take_new_blocked_notifications(), vec!["ws"]);

        // GUI 的一次 poll 可以合并 hook release 后的 detector bootstrap、
        // settling 和最终完成帧。只有第一帧是 bootstrap；最后的 Done
        // 仍是一个必须通知的真实 Working -> Done 转换。
        e.apply(
            "ws",
            1,
            &[
                AttentionSignal::AuthoritativeStatus {
                    status: PaneStatus::Done,
                    initial: true,
                },
                AttentionSignal::AuthoritativeStatus {
                    status: PaneStatus::Working,
                    initial: false,
                },
                AttentionSignal::AuthoritativeStatus {
                    status: PaneStatus::Done,
                    initial: false,
                },
            ],
            "finished",
            4,
        );

        assert_eq!(e.snapshot()[0].panes[0].status, PaneStatus::Done);
        assert_eq!(e.take_new_done_notifications(), vec!["ws"]);
        assert!(e.take_new_done_notifications().is_empty());
    }

    #[test]
    fn clearing_authoritative_status_restores_byte_heuristics() {
        let mut e = AttentionEngine::new(AttentionConfig::default(), clock());
        e.apply(
            "ws",
            1,
            &[AttentionSignal::AuthoritativeStatus {
                status: PaneStatus::Working,
                initial: true,
            }],
            "running",
            1,
        );
        e.apply(
            "ws",
            1,
            &[AttentionSignal::ClearAuthoritativeStatus],
            "released",
            2,
        );
        e.apply(
            "ws",
            1,
            &[AttentionSignal::AttentionRequest {
                source: AttentionSource::Bel,
            }],
            "waiting",
            3,
        );

        assert_eq!(e.snapshot()[0].panes[0].status, PaneStatus::Blocked);
        assert_eq!(e.take_new_blocked_notifications(), vec!["ws"]);
    }

    #[test]
    fn removing_authoritative_pane_clears_workspace_attention() {
        let mut e = AttentionEngine::new(AttentionConfig::default(), clock());
        e.apply(
            "ws",
            7,
            &[AttentionSignal::AuthoritativeStatus {
                status: PaneStatus::Blocked,
                initial: false,
            }],
            "waiting",
            1,
        );
        assert_eq!(e.blocked_workspace_count(), 1);
        assert_eq!(e.take_new_blocked_notifications(), vec!["ws"]);

        e.remove_pane("ws", 7);
        assert_eq!(e.blocked_workspace_count(), 0);
        assert!(e.snapshot().is_empty());

        // 同名 workspace 后续的新 pane 仍可产生一条新的转换通知。
        e.apply(
            "ws",
            8,
            &[AttentionSignal::AttentionRequest {
                source: AttentionSource::Bel,
            }],
            "new pane",
            1,
        );
        assert_eq!(e.take_new_blocked_notifications(), vec!["ws"]);
    }

    #[test]
    fn two_workspaces_blocked_count_is_2() {
        let mut e = AttentionEngine::new(AttentionConfig::default(), clock());
        for ws in ["ws-a", "ws-b"] {
            e.apply(
                ws,
                1,
                &[AttentionSignal::AttentionRequest {
                    source: AttentionSource::Bel,
                }],
                "line",
                1,
            );
        }
        assert_eq!(e.blocked_workspace_count(), 2);
    }

    #[test]
    fn invalid_regex_skipped_without_panic() {
        let cfg = AttentionConfig {
            blocked_regex: vec!["[".into()],
            ..AttentionConfig::default()
        };
        let mut e = AttentionEngine::new(cfg, clock());
        e.apply("ws", 1, &[], "line", 1);
        assert_eq!(e.blocked_workspace_count(), 0);
    }

    /// E6：前台 pane 的 CommandDone 视为已看见（BecameVisible → Idle），
    /// 不进 attention 列表（前台 `ls` 不弹提醒）。
    #[test]
    fn foreground_command_done_is_not_listed() {
        let mut e = AttentionEngine::new(AttentionConfig::default(), clock());
        e.apply(
            "ws",
            1,
            &[AttentionSignal::CommandDone { exit_code: Some(0) }],
            "ls",
            1,
        );
        assert_eq!(
            e.snapshot()[0].panes[0].status,
            PaneStatus::Done,
            "未看见前 CommandDone 是 Done"
        );
        // 前台输出/可见 → 已看见，Done 清成 Idle。
        e.on_became_visible("ws", 1);
        assert_eq!(
            e.snapshot()[0].panes[0].status,
            PaneStatus::Idle,
            "前台 CommandDone 应清成 Idle"
        );
        // 前台 Done 清成 Idle 后，注意力列表（只列 Blocked/Done）不应包含它。
        assert_eq!(
            e.snapshot()[0].panes[0].status,
            PaneStatus::Idle,
            "前台 CommandDone 应清成 Idle"
        );
    }

    #[test]
    fn process_name_normalizes_agent_wrappers() {
        assert_eq!(
            AttentionEngine::<FakeClock>::normalize_process_name("node /opt/bin/codex"),
            Some("codex".into())
        );
        assert_eq!(
            AttentionEngine::<FakeClock>::normalize_process_name("npx @openai/codex-cli"),
            Some("codex".into())
        );
        assert_eq!(
            AttentionEngine::<FakeClock>::normalize_process_name(
                "/Applications/Cursor.app/cursor-agent"
            ),
            Some("cursor".into())
        );
        assert_eq!(
            AttentionEngine::<FakeClock>::normalize_process_name("/bin/zsh"),
            Some("zsh".into())
        );
        assert_eq!(
            AttentionEngine::<FakeClock>::normalize_process_name("pi"),
            Some("pi".into()),
            "tmux pane_current_command reports the Pi coding agent as `pi`"
        );
    }

    #[test]
    fn tmux_agent_process_tracks_working_blocked_done_and_restart() {
        let mut e = AttentionEngine::new(AttentionConfig::default(), clock());
        e.set_process_name("ws", 1, Some("zsh".into()));

        e.set_process_name("ws", 1, Some("pi".into()));
        let pane = &e.snapshot()[0].panes[0];
        assert_eq!(pane.process_name.as_deref(), Some("pi"));
        assert!(pane.process_is_agent);
        assert_eq!(pane.agent_name.as_deref(), Some("pi"));
        assert_eq!(pane.status, PaneStatus::Working);

        e.apply(
            "ws",
            1,
            &[AttentionSignal::AttentionRequest {
                source: AttentionSource::Bel,
            }],
            "approval required",
            2,
        );
        e.set_process_name("ws", 1, Some("pi".into()));
        assert_eq!(
            e.snapshot()[0].panes[0].status,
            PaneStatus::Blocked,
            "a repeated tmux subscription must not erase a blocked agent state"
        );

        e.set_process_name("ws", 1, Some("zsh".into()));
        let pane = &e.snapshot()[0].panes[0];
        assert_eq!(pane.process_name.as_deref(), Some("pi"));
        assert_eq!(
            pane.status,
            PaneStatus::Blocked,
            "waiting-for-user remains blocked until the user acknowledges it"
        );

        e.on_user_input("ws", 1);
        assert_eq!(e.snapshot()[0].panes[0].status, PaneStatus::Idle);

        e.set_process_name("ws", 1, Some("pi".into()));
        assert_eq!(
            e.snapshot()[0].panes[0].status,
            PaneStatus::Working,
            "starting the same agent again must enter Working"
        );
        e.set_process_name("ws", 1, Some("zsh".into()));
        assert_eq!(e.snapshot()[0].panes[0].status, PaneStatus::Done);

        e.set_process_name("ws", 1, Some("pi".into()));
        assert_eq!(
            e.snapshot()[0].panes[0].status,
            PaneStatus::Working,
            "a new run must replace the previous Done state"
        );
    }

    #[test]
    fn tmux_non_agent_process_tracks_running_and_done() {
        let mut e = AttentionEngine::new(AttentionConfig::default(), clock());
        e.set_process_name("ws", 1, Some("zsh".into()));

        e.set_process_name("ws", 1, Some("cargo".into()));
        let pane = &e.snapshot()[0].panes[0];
        assert_eq!(pane.process_name.as_deref(), Some("cargo"));
        assert_eq!(
            pane.status,
            PaneStatus::Working,
            "every non-shell command must enter the running lifecycle"
        );

        e.set_process_name("ws", 1, Some("zsh".into()));
        let pane = &e.snapshot()[0].panes[0];
        assert_eq!(pane.process_name.as_deref(), Some("cargo"));
        assert_eq!(
            pane.status,
            PaneStatus::Done,
            "returning to the shell must leave an unread completed command"
        );
    }

    #[test]
    fn login_shell_is_idle_but_shell_command_tracks_running_and_done() {
        let mut e = AttentionEngine::new(AttentionConfig::default(), clock());
        e.set_process_name("ws", 1, Some("/bin/zsh -l".into()));
        assert_ne!(
            e.snapshot()[0].panes[0].status,
            PaneStatus::Working,
            "an interactive login shell is only a container"
        );

        e.set_process_name("ws", 1, Some("/bin/bash -c echo-ready".into()));
        let pane = &e.snapshot()[0].panes[0];
        assert_eq!(pane.process_name.as_deref(), Some("bash -c echo-ready"));
        assert_eq!(pane.status, PaneStatus::Working);

        e.set_process_name("ws", 1, Some("/bin/zsh -l".into()));
        assert_eq!(e.snapshot()[0].panes[0].status, PaneStatus::Done);
    }

    #[test]
    fn real_tmux_wrapped_codex_argv_is_classified_as_agent() {
        let fixture =
            include_str!("../../../tests/samples/tmux-agent-process-observation-2026-0901.txt");
        let argv = fixture
            .lines()
            .find(|line| line.contains("|node|node /usr/bin/codex "))
            .and_then(|line| line.split('|').next_back())
            .expect("captured Codex foreground argv");

        let mut e = AttentionEngine::new(AttentionConfig::default(), clock());
        e.set_process_name("ws", 1, Some("zsh".into()));
        e.set_process_name("ws", 1, Some(argv.into()));

        let pane = &e.snapshot()[0].panes[0];
        assert_eq!(pane.process_name.as_deref(), Some("codex"));
        assert_eq!(pane.agent_name.as_deref(), Some("codex"));
        assert!(pane.process_is_agent);
        assert_eq!(pane.status, PaneStatus::Working);
    }

    #[test]
    fn released_authoritative_agent_keeps_its_identity() {
        let mut e = AttentionEngine::new(AttentionConfig::default(), clock());
        e.set_agent_process_name("ws", 1, Some("review-bot".into()));
        e.apply(
            "ws",
            1,
            &[AttentionSignal::AuthoritativeStatus {
                status: PaneStatus::Working,
                initial: false,
            }],
            "",
            1,
        );

        e.set_agent_process_name("ws", 1, None);
        e.apply("ws", 1, &[AttentionSignal::ClearAuthoritativeStatus], "", 2);

        let pane = &e.snapshot()[0].panes[0];
        assert_eq!(
            pane.process_name.as_deref(),
            Some("review-bot"),
            "an acknowledged/released agent must remain available to the Agents projection"
        );
        assert!(pane.process_is_agent);
        assert_eq!(pane.agent_name.as_deref(), Some("review-bot"));
    }

    #[test]
    fn process_name_keeps_shell_fallback_without_overwriting_command() {
        let mut e = AttentionEngine::new(AttentionConfig::default(), clock());
        e.set_process_name("ws", 1, Some("/bin/zsh".into()));
        assert_eq!(
            e.snapshot()[0].panes[0].process_name.as_deref(),
            Some("zsh")
        );

        e.set_process_name("ws", 1, Some("sleep".into()));
        e.set_process_name("ws", 1, Some("zsh".into()));
        assert_eq!(
            e.snapshot()[0].panes[0].process_name.as_deref(),
            Some("sleep")
        );
    }

    #[test]
    fn structured_notifications_include_process_and_pane() {
        let mut e = AttentionEngine::new(AttentionConfig::default(), clock());
        e.set_process_name("ws", 7, Some("node /opt/codex".into()));
        e.apply(
            "ws",
            7,
            &[AttentionSignal::AttentionRequest {
                source: AttentionSource::Bel,
            }],
            "approve?",
            42,
        );
        let notifications = e.take_notifications();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].workspace_id, "ws");
        assert_eq!(notifications[0].pane_id, 7);
        assert_eq!(notifications[0].process_name.as_deref(), Some("codex"));
        assert_eq!(notifications[0].last_line, "approve?");
        assert_eq!(notifications[0].seq, 42);
        assert!(e.take_notifications().is_empty());
    }

    #[test]
    fn acknowledge_clears_listed_status_and_notification_deduplication() {
        let mut e = AttentionEngine::new(AttentionConfig::default(), clock());
        e.apply(
            "ws",
            3,
            &[AttentionSignal::AttentionRequest {
                source: AttentionSource::Bel,
            }],
            "approve",
            1,
        );
        assert_eq!(e.take_notifications().len(), 1);
        e.acknowledge("ws", 3);
        assert_eq!(e.blocked_workspace_count(), 0);
        assert!(e.snapshot()[0].panes[0].status == PaneStatus::Idle);
        assert!(e.take_notifications().is_empty());
    }

    #[test]
    fn authoritative_status_tracks_viewed_state_without_overwriting_runtime_state() {
        let mut e = AttentionEngine::new(AttentionConfig::default(), clock());
        e.apply(
            "ws",
            9,
            &[AttentionSignal::AuthoritativeStatus {
                status: PaneStatus::Blocked,
                initial: false,
            }],
            "approve",
            1,
        );
        let pane = &e.snapshot()[0].panes[0];
        assert_eq!(pane.status, PaneStatus::Blocked);
        assert!(!pane.acknowledged);

        e.on_became_visible("ws", 9);
        let pane = &e.snapshot()[0].panes[0];
        assert_eq!(pane.status, PaneStatus::Blocked);
        assert!(pane.acknowledged);

        // 同一权威状态的元数据刷新不能重新点黄；真正的新状态会重置未读。
        e.apply(
            "ws",
            9,
            &[AttentionSignal::AuthoritativeStatus {
                status: PaneStatus::Blocked,
                initial: false,
            }],
            "approve",
            2,
        );
        assert!(e.snapshot()[0].panes[0].acknowledged);
        e.apply(
            "ws",
            9,
            &[AttentionSignal::AuthoritativeStatus {
                status: PaneStatus::Done,
                initial: false,
            }],
            "done",
            3,
        );
        let pane = &e.snapshot()[0].panes[0];
        assert_eq!(pane.status, PaneStatus::Done);
        assert!(!pane.acknowledged);
    }
}
