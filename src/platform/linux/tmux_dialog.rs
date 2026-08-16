//! 工作区 attach 流程（VSCode Quick Pick 风格）。
//!
//! 由命令面板触发：
//! 1. 列出 core discovery 的工作区候选（名 + 创建时间 + tab 数）
//! 2. 顶部 `+ Create new workspace`
//! 3. 选已有 → attach；选 Create → 输入名字 → 创建 + attach

use std::time::{SystemTime, UNIX_EPOCH};

use gtk4::prelude::*;
use gtk4::Window;

use crate::platform::linux::ffi_bridge::{SshHostEntry, TmuxSessionEntry};
use crate::platform::linux::pane_switcher;
use crate::platform::linux::quick_pick::{self, QuickPickItem};

/// 工作区集成动作结果。
#[derive(Debug, Clone)]
pub enum TmuxAction {
    /// attach 到已有工作区（按名字）。
    Attach { session: String },
    /// 新建工作区（空名=自动命名）。
    NewSession { name: Option<String> },
}

const CREATE_ID: &str = "__create__";

/// 是否为「新建 session」行。
pub fn is_create_session_id(id: &str) -> bool {
    id == CREATE_ID
}

/// SSH host 列表 → Quick Pick 条目。
pub fn ssh_host_pick_items(hosts: &[SshHostEntry]) -> Vec<QuickPickItem> {
    hosts
        .iter()
        .map(|h| {
            let detail = if h.user.is_empty() {
                format!("{}:{}", h.hostname, h.port)
            } else {
                format!("{}@{}:{}", h.user, h.hostname, h.port)
            };
            QuickPickItem {
                id: h.alias.clone(),
                label: h.alias.clone(),
                detail: Some(detail),
            }
        })
        .collect()
}

/// 工作区候选列表（首行永远是新建）。
pub fn tmux_session_pick_items(sessions: &[TmuxSessionEntry]) -> Vec<QuickPickItem> {
    let mut items = Vec::with_capacity(sessions.len() + 1);
    items.push(QuickPickItem {
        id: CREATE_ID.into(),
        label: crate::platform::i18n::tr(crate::platform::i18n::Key::TmuxCreateNew),
        detail: Some(crate::platform::i18n::tr(
            crate::platform::i18n::Key::TmuxCreateDetail,
        )),
    });
    for s in sessions {
        let attached = if s.attached {
            format!(
                " · {}",
                crate::platform::i18n::tr(crate::platform::i18n::Key::TmuxAttached)
            )
        } else {
            String::new()
        };
        items.push(QuickPickItem {
            id: s.name.clone(),
            label: s.name.clone(),
            detail: Some(format!(
                "{}{attached}",
                crate::platform::i18n::tr_args(
                    crate::platform::i18n::Key::TmuxWindows,
                    &[("count", &s.windows.to_string())],
                )
            )),
        });
    }
    items
}

/// 弹出工作区选择器。
///
/// `socket` 为 tmux `-L` socket 名（可选）；列出候选时走 core discovery。
pub fn show<F>(parent: &impl IsA<Window>, socket: Option<&str>, on_done: F)
where
    F: Fn(TmuxAction) + 'static,
{
    let sessions = list_workspace_candidates(socket);
    let mut items = Vec::with_capacity(sessions.len() + 1);
    items.push(QuickPickItem {
        id: CREATE_ID.into(),
        label: crate::platform::i18n::tr(crate::platform::i18n::Key::TmuxCreateNew),
        detail: Some(crate::platform::i18n::tr(
            crate::platform::i18n::Key::TmuxCreateDetail,
        )),
    });
    for s in &sessions {
        items.push(QuickPickItem {
            id: s.name.clone(),
            label: s.name.clone(),
            detail: Some(format_session_detail(s)),
        });
    }

    let parent_win = parent.clone().upcast::<Window>();
    let on_done = std::rc::Rc::new(std::cell::RefCell::new(Some(on_done)));
    let parent_for_cb = parent_win.clone();

    quick_pick::show(
        &parent_win,
        &crate::platform::i18n::tr(crate::platform::i18n::Key::TmuxAttachPlaceholder),
        items,
        move |picked| {
            let Some(item) = picked else {
                return;
            };
            if item.id == CREATE_ID {
                let default_name = default_new_session_name();
                let on_done = on_done.clone();
                pane_switcher::show_rename(&parent_for_cb, &default_name, move |name| {
                    if let Some(cb) = on_done.borrow_mut().take() {
                        cb(TmuxAction::NewSession { name: Some(name) });
                    }
                });
            } else if let Some(cb) = on_done.borrow_mut().take() {
                cb(TmuxAction::Attach { session: item.id });
            }
        },
    );
}

/// 工作区候选（来自 core discovery，产品名不是 tmux session）。
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub name: String,
    pub created: Option<u64>,
    pub windows: Option<u32>,
}

/// 列出工作区候选（core discovery，带创建时间与 tab 数）。
pub fn list_workspace_candidates(socket: Option<&str>) -> Vec<SessionInfo> {
    crate::core::discovery::list_local_tmux_sessions(socket)
        .into_iter()
        .map(|s| SessionInfo {
            name: s.name,
            created: Some(s.created),
            windows: Some(s.windows),
        })
        .collect()
}

fn format_session_detail(s: &SessionInfo) -> String {
    let age = s
        .created
        .map(|ts| relative_age_label(ts, now_secs()))
        .unwrap_or_else(|| crate::platform::i18n::tr(crate::platform::i18n::Key::TmuxUnknown));
    let wins = s
        .windows
        .map(|n| n.to_string())
        .unwrap_or_else(|| crate::platform::i18n::tr(crate::platform::i18n::Key::TmuxUnknown));
    crate::platform::i18n::tr_args(
        crate::platform::i18n::Key::TmuxSessionDetail,
        &[("age", &age), ("count", &wins)],
    )
}

fn relative_age_label(created_secs: u64, now: u64) -> String {
    let ago = now.saturating_sub(created_secs);
    let (key, count) = if ago < 60 {
        (crate::platform::i18n::Key::TmuxSecondsAgo, ago)
    } else {
        let mins = ago / 60;
        if mins < 60 {
            (crate::platform::i18n::Key::TmuxMinutesAgo, mins)
        } else {
            let hours = mins / 60;
            if hours < 48 {
                (crate::platform::i18n::Key::TmuxHoursAgo, hours)
            } else {
                (crate::platform::i18n::Key::TmuxDaysAgo, hours / 24)
            }
        }
    };
    let count = count.to_string();
    crate::platform::i18n::tr_args(key, &[("count", &count)])
}

fn default_new_session_name() -> String {
    let ts = now_secs();
    format!("muxterm-{ts}")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 相对时间文案（如 `2h ago` / `3d ago`）。
pub fn relative_age(created_secs: u64, now: u64) -> String {
    let ago = now.saturating_sub(created_secs);
    if ago < 60 {
        return format!("{ago}s ago");
    }
    let mins = ago / 60;
    if mins < 60 {
        return format!("{mins}m ago");
    }
    let hours = mins / 60;
    if hours < 48 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    format!("{days}d ago")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_age_buckets() {
        let now = 1_000_000u64;
        assert_eq!(relative_age(now - 10, now), "10s ago");
        assert_eq!(relative_age(now - 120, now), "2m ago");
        assert_eq!(relative_age(now - 7200, now), "2h ago");
        assert_eq!(relative_age(now - 3 * 86400, now), "3d ago");
    }

    #[test]
    fn format_session_detail_contains_windows() {
        let s = SessionInfo {
            name: "main".into(),
            created: Some(now_secs().saturating_sub(7200)),
            windows: Some(3),
        };
        let d = format_session_detail(&s);
        assert!(d.contains("3"), "{d}");
        let age = crate::platform::i18n::tr_args(
            crate::platform::i18n::Key::TmuxHoursAgo,
            &[("count", "2")],
        );
        assert!(d.contains(&age) || d.contains("h"), "{d}");
    }

    #[test]
    fn ssh_host_pick_items_use_alias_as_id() {
        let hosts = vec![SshHostEntry {
            alias: "ryzen".into(),
            hostname: "192.168.5.6".into(),
            port: 22,
            user: "wlz".into(),
        }];
        let items = ssh_host_pick_items(&hosts);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "ryzen");
        assert!(items[0].detail.as_ref().unwrap().contains("192.168.5.6"));
    }

    #[test]
    fn workspace_candidate_pick_items_start_with_create() {
        let sessions = vec![TmuxSessionEntry {
            name: "legion".into(),
            windows: 4,
            attached: false,
            created: 0,
        }];
        let items = tmux_session_pick_items(&sessions);
        assert!(is_create_session_id(&items[0].id));
        assert_eq!(items[1].id, "legion");
        assert!(items[1].detail.as_ref().unwrap().contains("4"));
    }
}
