//! Activity-lane and attention effects for the GTK window.
//!
//! This module projects command marks, attention notifications, and
//! session-close effects from owned frontend state into the UI.

use super::*;

pub(super) fn update_last_seen(s: &mut UiState) {
    if s.scenes.widget().visible_child_name().as_deref() == Some("workspace-loading") {
        s.overlay.last_seen.set_visible(false);
        return;
    }
    let key = (active_workspace_key(s), s.active_pane);
    let workspace = active_workspace_key(s);
    let latest = s
        .event_pump
        .client()
        .workspace_pane_latest_line_seq(&workspace, key.1);
    s.last_seen.observe(key.clone(), latest);
    let offset = s.last_seen.baseline(&key).and_then(|seq| {
        s.event_pump
            .client()
            .workspace_pane_viewport_for_seq(&workspace, key.1, seq)
    });
    let visible = s.last_seen.target(&key, offset, Instant::now()).is_some();
    s.overlay.last_seen.set_visible(visible);
}

/// 回底按钮可见性：VTE 滚离底部时显示，回到尾部隐藏（W16a）。
/// 把当前 pane 滚到包含指定文本的行（命令刻度 / 上次看到这里共用）。
pub(super) fn scroll_to_command_text(
    state: &Rc<RefCell<UiState>>,
    text: &Rc<RefCell<Option<String>>>,
) {
    let s = state.borrow();
    let Some(text) = text.borrow().clone() else {
        return;
    };
    let pane = s.active_pane;
    let workspace_id = active_workspace_key(&s);
    let marks = s
        .event_pump
        .client()
        .workspace_pane_command_marks(&workspace_id, pane)
        .unwrap_or_default();
    if let Some(offset) = marks
        .iter()
        .rev()
        .find(|mark| mark.command == text)
        .and_then(|mark| {
            s.event_pump
                .client()
                .workspace_pane_viewport_for_seq(&workspace_id, pane, mark.seq)
        })
    {
        if let Some(view) = s.active_layout().pane(pane).cloned() {
            scroll_to_history_offset(&view, offset);
        }
    }
}

/// Core 偏移以 live tail 为零点；GTK adjustment 以历史起点为零点。
pub(super) fn scroll_to_history_offset(view: &PaneSurface, offset: u32) {
    if let Some(adj) = view.terminal().vadjustment() {
        adj.set_value((adj.upper() - adj.page_size() - f64::from(offset)).max(adj.lower()));
    }
}

/// 从当前 pane 的 OSC 133 刻度刷新红/绿标记（W18h）。
pub(super) fn update_command_marks(s: &UiState) {
    let workspace_id = active_workspace_key(s);
    let marks = s
        .event_pump
        .client()
        .workspace_pane_command_marks(&workspace_id, s.active_pane)
        .unwrap_or_default();
    let ok = marks.iter().rev().find(|m| m.exit_code == Some(0));
    let fail = marks
        .iter()
        .rev()
        .find(|m| m.exit_code.is_some_and(|c| c != 0));
    if let Some(m) = ok {
        s.overlay.command_ok.set_visible(true);
        s.overlay.command_ok.set_tooltip_text(Some(&m.command));
        *s.overlay.command_ok_text.borrow_mut() = Some(m.command.clone());
    } else {
        s.overlay.command_ok.set_visible(false);
        *s.overlay.command_ok_text.borrow_mut() = None;
    }
    if let Some(m) = fail {
        s.overlay.command_fail.set_visible(true);
        s.overlay.command_fail.set_tooltip_text(Some(&m.command));
        *s.overlay.command_fail_text.borrow_mut() = Some(m.command.clone());
    } else {
        s.overlay.command_fail.set_visible(false);
        *s.overlay.command_fail_text.borrow_mut() = None;
    }
}

/// 当前激活 pane 的 VTE 是否在底部（scroll lock / 回底按钮共用）。
pub(super) fn view_at_bottom(view: &std::rc::Rc<PaneSurface>) -> bool {
    view.terminal()
        .vadjustment()
        .map(|adj| {
            let page = adj.page_size();
            let upper = adj.upper();
            // VTE 内容不足一屏时 upper 可能等于 page_size，视为已在底部。
            upper - page <= adj.value() + 1.0
        })
        .unwrap_or(true)
}

pub(super) fn update_jump_latest(s: &UiState) {
    let at_bottom = s
        .active_layout()
        .pane(s.active_pane)
        .map(view_at_bottom)
        .unwrap_or(true);
    s.overlay.jump_latest.set_visible(!at_bottom);
    if at_bottom {
        // 回到尾部：搜索高亮不再有意义（W17c）。
        s.overlay.search_highlight.set_visible(false);
    } else if s.overlay.jump_unseen > 0 {
        s.overlay
            .jump_latest
            .set_label(&format!("↓ +{}", s.overlay.jump_unseen));
    } else {
        s.overlay.jump_latest.set_label("↓");
    }
}

#[cfg(test)]
#[derive(Debug, Default)]
pub(super) struct UiBatchEffects {
    pub(super) topology_changed: bool,
}

#[cfg(test)]
impl UiBatchEffects {
    pub(super) fn note_topology(&mut self) {
        self.topology_changed = true;
    }
}

#[cfg(test)]
pub(super) fn attention_event_pane(event: &StateChange) -> Option<u32> {
    match event {
        StateChange::PaneOutput { pane, .. }
        | StateChange::PaneFrame { pane, .. }
        | StateChange::PaneAgentChanged { pane, .. } => Some(pane.0),
        _ => None,
    }
}

pub(super) fn mark_pending_close_if_session_ended(s: &mut UiState) {
    let workspace_id = active_workspace_key(s);
    let n_tabs = s
        .view_store
        .workspace(&workspace_id)
        .map_or(0, |view| view.tabs.len());
    if should_close_window(false, n_tabs, s.on_last_pane_exit) {
        s.pending_close = true;
    }
}

/// 取走本轮 blocked / done 通知并交给 sink（测试日志也记录）。
pub(super) fn drain_attention_notifications(s: &mut UiState) {
    let notifications = s
        .event_pump
        .client()
        .take_activity_notifications()
        .unwrap_or_default();
    if notifications.notifications.is_empty() {
        for ws in notifications.blocked {
            record_attention_notification(s, &ws, "blocked");
        }
        for ws in notifications.done {
            record_attention_notification(s, &ws, "done");
        }
    } else {
        for notification in notifications.notifications {
            record_attention_notification(s, &notification.workspace_id, &notification.kind);
        }
    }

    // Compatibility-only direct injection used by the GTK tests. Runtime
    // events are drained from Core above and never pass through this adapter.
    for notification in s.compatibility_activity.take_notifications() {
        record_attention_notification(s, &notification.workspace_id, &notification.kind);
    }
}

pub(super) fn record_attention_notification(s: &mut UiState, workspace: &str, kind: &str) {
    match kind {
        "blocked" => {
            tracing::info!(
                target: "muxterm::notify",
                workspace = %workspace,
                "blocked workspace notification"
            );
            s.notification_sink
                .notify_blocked(workspace, "needs attention");
            s.notification_log
                .push(format!("{workspace}: needs attention"));
        }
        "done" => {
            tracing::info!(
                target: "muxterm::notify",
                workspace = %workspace,
                "background task done"
            );
            s.notification_sink.notify_done(workspace, "task complete");
            s.notification_log
                .push(format!("{workspace}: task complete"));
        }
        other => {
            tracing::debug!(
                target: "muxterm::notify",
                workspace = %workspace,
                kind = other,
                "ignored unknown activity notification"
            );
        }
    }
}
