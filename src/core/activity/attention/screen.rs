//! tmux agent 屏幕规则：对照 Herdr screen manifest，不用 hook。
//!
//! 每个已知 agent 一条规则表。按 priority 从高到低匹配；
//! 全部未命中 → Idle（Herdr `default_known_agent_idle_fallback`）。

use super::state::PaneStatus;
use regex::Regex;

/// 可见屏 + OSC 标题/进度，给规则当输入。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScreenSnapshot {
    pub title: String,
    pub osc_progress: String,
    /// 可见屏非空行，从上到下。
    pub lines: Vec<String>,
}

impl ScreenSnapshot {
    pub fn from_visible(
        title: impl Into<String>,
        osc_progress: impl Into<String>,
        lines: Vec<String>,
    ) -> Self {
        Self {
            title: title.into(),
            osc_progress: osc_progress.into(),
            lines: lines
                .into_iter()
                .map(|line| line.trim_end().to_string())
                .filter(|line| !line.trim().is_empty())
                .collect(),
        }
    }

    pub fn from_text(body: &str) -> Self {
        Self::from_visible("", "", body.lines().map(str::to_string).collect())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Region {
    Title,
    Progress,
    Whole,
    Bottom(usize),
    Top(usize),
}

struct Rule {
    agent: &'static str,
    priority: i32,
    state: PaneStatus,
    keep: bool,
    region: Region,
    contains: &'static [&'static str],
    any_contains: &'static [&'static str],
    line_regex: &'static [&'static str],
    regex: &'static [&'static str],
    not_contains: &'static [&'static str],
    not_regex: &'static [&'static str],
}

/// 对已知 agent 的当前屏幕分类。`None` = 保持原状态（transcript viewer）。
pub fn classify_agent_screen(agent: &str, screen: &ScreenSnapshot) -> Option<PaneStatus> {
    let agent = agent.to_ascii_lowercase();
    let mut rules: Vec<&Rule> = RULES
        .iter()
        .filter(|rule| rule.agent.is_empty() || rule.agent == agent)
        .collect();
    rules.sort_by_key(|rule| -rule.priority);
    for rule in rules {
        if rule_matches(rule, screen) {
            if rule.keep {
                return None;
            }
            return Some(rule.state);
        }
    }
    Some(PaneStatus::Idle)
}

fn rule_matches(rule: &Rule, screen: &ScreenSnapshot) -> bool {
    let (text, lines) = region_text(rule.region, screen);
    if text.trim().is_empty() && !matches!(rule.region, Region::Whole) {
        if !rule.contains.is_empty()
            || !rule.any_contains.is_empty()
            || !rule.line_regex.is_empty()
            || !rule.regex.is_empty()
        {
            return false;
        }
    }
    if !rule
        .contains
        .iter()
        .all(|needle| contains_ci(&text, needle))
    {
        return false;
    }
    if !rule.any_contains.is_empty()
        && !rule
            .any_contains
            .iter()
            .any(|needle| contains_ci(&text, needle))
    {
        return false;
    }
    if !rule.line_regex.is_empty()
        && !rule
            .line_regex
            .iter()
            .any(|pattern| lines.iter().any(|line| regex_is_match(pattern, line)))
    {
        return false;
    }
    if !rule
        .regex
        .iter()
        .all(|pattern| regex_is_match(pattern, &text))
    {
        return false;
    }
    if rule
        .not_contains
        .iter()
        .any(|needle| contains_ci(&text, needle))
    {
        return false;
    }
    if rule
        .not_regex
        .iter()
        .any(|pattern| regex_is_match(pattern, &text))
    {
        return false;
    }
    true
}

fn region_text(region: Region, screen: &ScreenSnapshot) -> (String, Vec<String>) {
    match region {
        Region::Title => (screen.title.clone(), vec![screen.title.clone()]),
        Region::Progress => (
            screen.osc_progress.clone(),
            vec![screen.osc_progress.clone()],
        ),
        Region::Whole => {
            let text = screen.lines.join("\n");
            (text, screen.lines.clone())
        }
        Region::Bottom(n) => {
            let start = screen.lines.len().saturating_sub(n);
            let lines = screen.lines[start..].to_vec();
            (lines.join("\n"), lines)
        }
        Region::Top(n) => {
            let lines: Vec<String> = screen.lines.iter().take(n).cloned().collect();
            (lines.join("\n"), lines)
        }
    }
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

fn regex_is_match(pattern: &str, text: &str) -> bool {
    Regex::new(pattern)
        .map(|regex| regex.is_match(text))
        .unwrap_or(false)
}

const EMPTY: &[&str] = &[];

const RULES: &[Rule] = &[
    // —— grok（Herdr grok.toml 2026.07.16.2）——
    Rule {
        agent: "grok",
        priority: 1300,
        state: PaneStatus::Blocked,
        keep: false,
        region: Region::Title,
        contains: &["Action Required"],
        any_contains: EMPTY,
        line_regex: EMPTY,
        regex: EMPTY,
        not_contains: EMPTY,
        not_regex: EMPTY,
    },
    Rule {
        agent: "grok",
        priority: 1200,
        state: PaneStatus::Blocked,
        keep: false,
        region: Region::Whole,
        contains: EMPTY,
        any_contains: EMPTY,
        line_regex: &[r"^\s*┃\s+[0-9a-z]+\s+\([●○]\)\s"],
        regex: EMPTY,
        not_contains: EMPTY,
        not_regex: EMPTY,
    },
    Rule {
        agent: "grok",
        priority: 1190,
        state: PaneStatus::Blocked,
        keep: false,
        region: Region::Bottom(2),
        contains: &[":select", "ctrl+o:yolo", "ctrl+c:cancel"],
        any_contains: EMPTY,
        line_regex: EMPTY,
        regex: EMPTY,
        not_contains: EMPTY,
        not_regex: EMPTY,
    },
    Rule {
        agent: "grok",
        priority: 1185,
        state: PaneStatus::Blocked,
        keep: false,
        region: Region::Bottom(2),
        contains: &["tab:scrollback", "shift+x:dismiss"],
        any_contains: EMPTY,
        line_regex: EMPTY,
        regex: EMPTY,
        not_contains: EMPTY,
        not_regex: EMPTY,
    },
    Rule {
        agent: "grok",
        priority: 1180,
        state: PaneStatus::Blocked,
        keep: false,
        region: Region::Whole,
        contains: &["yes, proceed", "no, reject"],
        any_contains: EMPTY,
        line_regex: EMPTY,
        regex: EMPTY,
        not_contains: EMPTY,
        not_regex: EMPTY,
    },
    Rule {
        agent: "grok",
        priority: 1150,
        state: PaneStatus::Working,
        keep: false,
        region: Region::Progress,
        contains: EMPTY,
        any_contains: EMPTY,
        line_regex: EMPTY,
        regex: &[r"^4;1;-1$"],
        not_contains: EMPTY,
        not_regex: EMPTY,
    },
    Rule {
        agent: "grok",
        priority: 1100,
        state: PaneStatus::Idle,
        keep: false,
        region: Region::Title,
        contains: EMPTY,
        any_contains: EMPTY,
        line_regex: EMPTY,
        regex: &[r"(?:^| - )grok$"],
        not_contains: EMPTY,
        not_regex: &[r"[\x{2800}-\x{28FF}]"],
    },
    Rule {
        agent: "grok",
        priority: 1000,
        state: PaneStatus::Working,
        keep: false,
        region: Region::Title,
        contains: EMPTY,
        any_contains: EMPTY,
        line_regex: EMPTY,
        regex: &[r"[\x{2800}-\x{28FF}]"],
        not_contains: EMPTY,
        not_regex: EMPTY,
    },
    Rule {
        agent: "grok",
        priority: 950,
        state: PaneStatus::Idle,
        keep: false,
        region: Region::Progress,
        contains: EMPTY,
        any_contains: EMPTY,
        line_regex: EMPTY,
        regex: &[r"^4;0;0$"],
        not_contains: EMPTY,
        not_regex: EMPTY,
    },
    Rule {
        agent: "grok",
        priority: 200,
        state: PaneStatus::Working,
        keep: false,
        region: Region::Whole,
        contains: EMPTY,
        any_contains: EMPTY,
        line_regex: &[r"^\s*[\x{2801}-\x{28FF}]\s.*\[stop\]\s*$"],
        regex: EMPTY,
        not_contains: EMPTY,
        not_regex: EMPTY,
    },
    Rule {
        agent: "grok",
        priority: 190,
        state: PaneStatus::Working,
        keep: false,
        region: Region::Bottom(2),
        contains: &["esc:cancel", "ctrl+.:shortcuts"],
        any_contains: EMPTY,
        line_regex: EMPTY,
        regex: EMPTY,
        not_contains: EMPTY,
        not_regex: EMPTY,
    },
    Rule {
        agent: "grok",
        priority: 100,
        state: PaneStatus::Idle,
        keep: false,
        region: Region::Bottom(2),
        contains: &["ctrl+.:shortcuts"],
        any_contains: EMPTY,
        line_regex: EMPTY,
        regex: EMPTY,
        not_contains: &["esc:cancel", "ctrl+c:cancel"],
        not_regex: EMPTY,
    },
    // —— claude ——
    Rule {
        agent: "claude",
        priority: 1100,
        state: PaneStatus::Working,
        keep: false,
        region: Region::Title,
        contains: EMPTY,
        any_contains: EMPTY,
        line_regex: EMPTY,
        regex: &[r"^[\x{2800}-\x{28FF}\x{25D0}-\x{25D3}] "],
        not_contains: EMPTY,
        not_regex: EMPTY,
    },
    Rule {
        agent: "claude",
        priority: 980,
        state: PaneStatus::Blocked,
        keep: false,
        region: Region::Whole,
        contains: &["esc to cancel"],
        any_contains: &[
            "enter to confirm",
            "enter to select",
            "do you want to proceed?",
        ],
        line_regex: EMPTY,
        regex: EMPTY,
        not_contains: EMPTY,
        not_regex: EMPTY,
    },
    Rule {
        agent: "claude",
        priority: 970,
        state: PaneStatus::Working,
        keep: false,
        region: Region::Bottom(12),
        contains: EMPTY,
        any_contains: &["esc to interrupt"],
        line_regex: EMPTY,
        regex: EMPTY,
        not_contains: EMPTY,
        not_regex: EMPTY,
    },
    Rule {
        agent: "claude",
        priority: 950,
        state: PaneStatus::Idle,
        keep: false,
        region: Region::Bottom(8),
        contains: EMPTY,
        any_contains: EMPTY,
        line_regex: &[r"^\s*❯"],
        regex: EMPTY,
        not_contains: &["enter to select", "esc to cancel"],
        not_regex: EMPTY,
    },
    // —— codex ——
    Rule {
        agent: "codex",
        priority: 1100,
        state: PaneStatus::Blocked,
        keep: false,
        region: Region::Title,
        contains: &["Action Required"],
        any_contains: EMPTY,
        line_regex: EMPTY,
        regex: EMPTY,
        not_contains: EMPTY,
        not_regex: EMPTY,
    },
    Rule {
        agent: "codex",
        priority: 1050,
        state: PaneStatus::Working,
        keep: false,
        region: Region::Title,
        contains: EMPTY,
        any_contains: EMPTY,
        line_regex: EMPTY,
        regex: &[r"(?:^| )[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏](?: |$)"],
        not_contains: EMPTY,
        not_regex: EMPTY,
    },
    Rule {
        agent: "codex",
        priority: 950,
        state: PaneStatus::Blocked,
        keep: false,
        region: Region::Whole,
        contains: EMPTY,
        any_contains: &[
            "press enter to confirm or esc to cancel",
            "allow command?",
            "enter to submit answer",
            "[y/n]",
            "Update available!",
        ],
        line_regex: EMPTY,
        regex: EMPTY,
        not_contains: EMPTY,
        not_regex: EMPTY,
    },
    Rule {
        agent: "codex",
        priority: 500,
        state: PaneStatus::Working,
        keep: false,
        region: Region::Whole,
        contains: &["esc to interrupt"],
        any_contains: EMPTY,
        line_regex: EMPTY,
        regex: EMPTY,
        not_contains: EMPTY,
        not_regex: EMPTY,
    },
    Rule {
        agent: "codex",
        priority: 100,
        state: PaneStatus::Idle,
        keep: false,
        region: Region::Title,
        contains: EMPTY,
        any_contains: EMPTY,
        line_regex: EMPTY,
        regex: &[r"\S"],
        not_contains: &["Action Required"],
        not_regex: &[r"(?:^| )[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏](?: |$)"],
    },
    // —— 其它已知 agent 的弱规则 ——
    Rule {
        agent: "",
        priority: 80,
        state: PaneStatus::Blocked,
        keep: false,
        region: Region::Whole,
        contains: EMPTY,
        any_contains: &[
            "action required",
            "allow command?",
            "do you want to proceed?",
            "waiting for permission",
            "[y/n]",
        ],
        line_regex: EMPTY,
        regex: EMPTY,
        not_contains: EMPTY,
        not_regex: EMPTY,
    },
    Rule {
        agent: "",
        priority: 70,
        state: PaneStatus::Working,
        keep: false,
        region: Region::Whole,
        contains: EMPTY,
        any_contains: &["esc to interrupt", "[stop]"],
        line_regex: EMPTY,
        regex: EMPTY,
        not_contains: EMPTY,
        not_regex: EMPTY,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grok_idle_prompt_is_idle() {
        let screen = ScreenSnapshot::from_visible(
            "muxterm - grok",
            "4;0;0",
            vec!["ready".into(), "Ctrl+.:shortcuts".into()],
        );
        assert_eq!(
            classify_agent_screen("grok", &screen),
            Some(PaneStatus::Idle)
        );
    }

    #[test]
    fn grok_spinner_stop_chip_is_working() {
        let screen = ScreenSnapshot::from_text(
            "⠧ Waiting on subagent… 2.8s   13s ⇣29.7k [stop]\nCtrl+.:shortcuts  Esc:cancel",
        );
        assert_eq!(
            classify_agent_screen("grok", &screen),
            Some(PaneStatus::Working)
        );
    }

    #[test]
    fn grok_title_spinner_is_working() {
        let screen = ScreenSnapshot::from_visible("⠇ implementing", "", Vec::new());
        assert_eq!(
            classify_agent_screen("grok", &screen),
            Some(PaneStatus::Working)
        );
    }

    #[test]
    fn grok_permission_dialog_is_blocked() {
        let screen = ScreenSnapshot::from_text(
            "┃  2 (○) Yes, proceed\n┃  z (○) Type your answer here\n1/3:select │ Ctrl+o:yolo │ Ctrl+c:cancel",
        );
        assert_eq!(
            classify_agent_screen("grok", &screen),
            Some(PaneStatus::Blocked)
        );
    }

    #[test]
    fn grok_action_required_title_is_blocked() {
        let screen = ScreenSnapshot::from_visible("⚠ Action Required", "", Vec::new());
        assert_eq!(
            classify_agent_screen("grok", &screen),
            Some(PaneStatus::Blocked)
        );
    }

    #[test]
    fn claude_prompt_box_is_idle() {
        let screen = ScreenSnapshot::from_text("review the diff\n❯ ");
        assert_eq!(
            classify_agent_screen("claude", &screen),
            Some(PaneStatus::Idle)
        );
    }

    #[test]
    fn claude_interrupt_hint_is_working() {
        let screen = ScreenSnapshot::from_text("✽ Transmuting… (2m 21s · esc to interrupt)");
        assert_eq!(
            classify_agent_screen("claude", &screen),
            Some(PaneStatus::Working)
        );
    }

    #[test]
    fn claude_permission_is_blocked() {
        let screen =
            ScreenSnapshot::from_text("Do you want to proceed?\n❯ 1. Yes\n2. No\nesc to cancel");
        assert_eq!(
            classify_agent_screen("claude", &screen),
            Some(PaneStatus::Blocked)
        );
    }

    #[test]
    fn codex_esc_interrupt_is_working() {
        let screen = ScreenSnapshot::from_text("• Running cargo test (12s • esc to interrupt)");
        assert_eq!(
            classify_agent_screen("codex", &screen),
            Some(PaneStatus::Working)
        );
    }

    #[test]
    fn grok_prompt_edit_is_idle() {
        let screen = ScreenSnapshot::from_visible(
            "review muxterm",
            "",
            vec!["type a question".into(), "Ctrl+.:shortcuts".into()],
        );
        assert_eq!(
            classify_agent_screen("grok", &screen),
            Some(PaneStatus::Idle),
            "在 prompt 里打字不是 working"
        );
    }

    #[test]
    fn known_agent_without_match_falls_back_to_idle() {
        let screen = ScreenSnapshot::from_text("some transcript line");
        assert_eq!(classify_agent_screen("pi", &screen), Some(PaneStatus::Idle));
    }
}
