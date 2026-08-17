//! 红点 / 标题 / 通知的纯逻辑与 sink（LINUX-PLAN §10 C3.4）。

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::gio::prelude::ApplicationExt;

use crate::core::attention::state::PaneStatus;

/// 状态栏红点文案：0 → None（隐藏），否则 `● N`。
pub fn badge_label(n: usize) -> Option<String> {
    if n == 0 {
        None
    } else {
        Some(format!("● {n}"))
    }
}

/// 窗口标题：blocked 工作区数 > 0 时加 `(●N) ` 前缀。
pub fn window_title(n: usize, workspace: &str) -> String {
    if n == 0 {
        format!("{workspace} — Muxterm")
    } else {
        format!("(●{n}) {workspace} — Muxterm")
    }
}

/// tab 前缀：blocked `● `，done `✓ `，其余空。
pub fn tab_prefix(status: Option<PaneStatus>) -> &'static str {
    match status {
        Some(PaneStatus::Blocked) => "● ",
        Some(PaneStatus::Done) => "✓ ",
        _ => "",
    }
}

/// 通知出口（测试永远注入 RecordingSink，生产 GioSink fail-soft）。
pub trait NotificationSink {
    fn notify_blocked(&self, workspace_id: &str, body: &str);
    /// 后台 pane 任务完成（OSC 133 D）通知。
    fn notify_done(&self, workspace_id: &str, body: &str);
}

/// 无操作 sink。
pub struct NullSink;

impl NotificationSink for NullSink {
    fn notify_blocked(&self, _workspace_id: &str, _body: &str) {}
    fn notify_done(&self, _workspace_id: &str, _body: &str) {}
}

/// 记录型 sink（测试断言用）。
#[derive(Clone, Default)]
pub struct RecordingSink {
    pub log: Rc<RefCell<Vec<String>>>,
}

impl RecordingSink {
    pub fn new() -> Self {
        Self::default()
    }
}

impl NotificationSink for RecordingSink {
    fn notify_blocked(&self, workspace_id: &str, body: &str) {
        self.log
            .borrow_mut()
            .push(format!("{workspace_id}: {body}"));
    }

    fn notify_done(&self, workspace_id: &str, body: &str) {
        self.log
            .borrow_mut()
            .push(format!("{workspace_id}: {body}"));
    }
}

/// Gio 通知 sink：无通知 daemon 时 no-op，绝不 panic。
pub struct GioSink {
    app: Option<gtk4::gio::Application>,
}

impl GioSink {
    pub fn new(app: Option<gtk4::gio::Application>) -> Self {
        Self { app }
    }
}

impl NotificationSink for GioSink {
    fn notify_blocked(&self, workspace_id: &str, body: &str) {
        let Some(app) = &self.app else {
            return;
        };
        let notification = gtk4::gio::Notification::new(workspace_id);
        notification.set_body(Some(body));
        app.send_notification(Some(workspace_id), &notification);
    }

    fn notify_done(&self, workspace_id: &str, body: &str) {
        let Some(app) = &self.app else {
            return;
        };
        let notification = gtk4::gio::Notification::new(workspace_id);
        notification.set_body(Some(body));
        app.send_notification(Some(workspace_id), &notification);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_label_hides_zero() {
        assert_eq!(badge_label(0), None);
        assert_eq!(badge_label(1), Some("● 1".into()));
        assert_eq!(badge_label(3), Some("● 3".into()));
    }

    #[test]
    fn window_title_prefixes_blocked_count() {
        assert_eq!(window_title(0, "legion"), "legion — Muxterm");
        assert_eq!(window_title(2, "legion"), "(●2) legion — Muxterm");
    }

    #[test]
    fn tab_prefix_maps_status() {
        assert_eq!(tab_prefix(Some(PaneStatus::Blocked)), "● ");
        assert_eq!(tab_prefix(Some(PaneStatus::Done)), "✓ ");
        assert_eq!(tab_prefix(Some(PaneStatus::Working)), "");
        assert_eq!(tab_prefix(None), "");
    }

    #[test]
    fn recording_sink_captures_notifications() {
        let sink = RecordingSink::new();
        sink.notify_blocked("ws-a", "needs you");
        sink.notify_blocked("ws-b", "ask");
        assert_eq!(
            *sink.log.borrow(),
            vec!["ws-a: needs you".to_string(), "ws-b: ask".to_string()]
        );
    }

    #[test]
    fn gio_sink_without_app_does_not_panic() {
        let sink = GioSink::new(None);
        sink.notify_blocked("ws", "needs you");
        sink.notify_done("ws", "task complete");
    }
}
