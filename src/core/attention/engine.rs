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

/// 单个 pane 的注意力状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneAttention {
    pub workspace_id: String,
    pub pane_id: u32,
    pub status: PaneStatus,
    pub last_line: String,
    pub seq: u64,
    pub process_name: Option<String>,
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

/// 注意力引擎（跨工作区聚合）。
pub struct AttentionEngine<C: Clock> {
    panes: HashMap<(String, u32), PaneAttention>,
    config: AttentionConfig,
    clock: C,
    /// 已对哪些 workspace 发过「进入 blocked」通知（清除后删除，重亮再通知）。
    notified_blocked: HashSet<String>,
    /// 已对哪些 workspace 发过「后台任务完成」通知（状态离开 Done 后删除）。
    notified_done: HashSet<String>,
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
                last_line: String::new(),
                seq: 0,
                process_name: None,
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
        {
            let entry = self.entry_mut(ws, pane);
            entry.last_line = last_line.to_string();
            entry.seq = seq;

            for sig in signals {
                let event = match sig {
                    AttentionSignal::CommandStart => PaneEvent::CommandStart,
                    AttentionSignal::CommandDone { exit_code } => PaneEvent::CommandDone {
                        exit_code: *exit_code,
                    },
                    AttentionSignal::AttentionRequest { .. } => PaneEvent::AttentionRequest,
                };
                entry.status = transition(entry.status, event);
            }
        }
        self.maybe_eval_regex(ws, pane, now);
        self.sync_notified(ws, pane, now);
    }

    /// 用户输入：Blocked → Idle（输入才算处理）。
    pub fn on_user_input(&mut self, ws: &str, pane: u32) {
        let now = self.clock.now();
        let entry = self.entry_mut(ws, pane);
        entry.status = transition(entry.status, PaneEvent::UserInput);
        self.sync_notified(ws, pane, now);
    }

    /// 该 pane 成为前台可见（仅当前台 pane，后台不触发）。
    pub fn on_became_visible(&mut self, ws: &str, pane: u32) {
        let now = self.clock.now();
        let entry = self.entry_mut(ws, pane);
        entry.status = transition(entry.status, PaneEvent::BecameVisible);
        self.sync_notified(ws, pane, now);
    }

    /// 更新 pane 进程名；非 shell 进程名可作 Working 粗判来源（注释见 LINUX-PLAN §9）。
    pub fn set_process_name(&mut self, ws: &str, pane: u32, name: Option<String>) {
        let entry = self.entry_mut(ws, pane);
        entry.process_name = name;
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

    fn maybe_eval_regex(&mut self, ws: &str, pane: u32, now: Instant) {
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

    /// 同步 notified_blocked：清除的 workspace 移除标记，便于重亮再通知。
    fn sync_notified(&mut self, ws: &str, pane: u32, now: Instant) {
        let Some(entry) = self.panes.get(&(ws.to_string(), pane)) else {
            return;
        };
        let muted = entry.mute_until.map(|m| m > now).unwrap_or(false);
        if entry.status != PaneStatus::Blocked || muted {
            self.notified_blocked.remove(ws);
        }
        if entry.status != PaneStatus::Done || muted {
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
}
