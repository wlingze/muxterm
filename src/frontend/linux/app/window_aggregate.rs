//! 聚合导航始终定位真实源 Workspace/Tab；不创建第二份终端。

use super::window_event_pump::activity_snapshot;
use super::window_scene::{request_switch_tab, show_workspace_scene};
use super::window_status::maybe_refresh_status;
use super::{parse_workspace_id, UiState};
use crate::frontend::linux::chrome::aggregate::{self, AggregateKind, AggregateTab};
use gtk4::prelude::*;

pub(super) fn tabs(s: &UiState) -> Vec<AggregateTab> {
    s.aggregate
        .kind
        .map(|kind| {
            aggregate::project(
                kind,
                &s.view_store,
                &activity_snapshot(s),
                &s.aggregate.hidden_agents,
            )
        })
        .unwrap_or_default()
}

pub(super) fn show(s: &mut UiState, kind: AggregateKind) {
    s.aggregate.kind = Some(kind);
    let tabs = tabs(s);
    if let Some(tab) = s.aggregate.selected(&tabs) {
        select_source(s, tab);
    } else {
        let stack = s.scenes.widget();
        if stack.child_by_name("aggregate-empty").is_none() {
            let label = gtk4::Label::new(Some(&kind.label()));
            label.set_widget_name("muxterm-aggregate-empty");
            stack.add_named(&label, Some("aggregate-empty"));
        }
        if let Some(label) = stack
            .child_by_name("aggregate-empty")
            .and_then(|widget| widget.downcast::<gtk4::Label>().ok())
        {
            label.set_text(&kind.label());
        }
        stack.set_visible_child_name("aggregate-empty");
        maybe_refresh_status(s, true);
    }
}

pub(super) fn open(s: &mut UiState, kind: AggregateKind) {
    // 再次选择固定 A 入口可恢复被隐藏的投影；源 tab 始终保持打开。
    if kind == AggregateKind::Agents {
        s.aggregate.hidden_agents.clear();
    }
    show(s, kind);
}

pub(super) fn reconcile(s: &mut UiState) {
    let Some(kind) = s.aggregate.kind else {
        return;
    };
    let projected = tabs(s);
    if kind == AggregateKind::Shells {
        if let Some(current) = projected.iter().find(|tab| {
            tab.source.workspace == s.active_workspace_key() && tab.source.tab == s.active_tab_id()
        }) {
            if s.aggregate
                .shell_last
                .as_ref()
                .is_some_and(|last| last.workspace == current.source.workspace)
            {
                s.aggregate.remember(current.source.clone());
            }
        }
    }
    if let Some(tab) = s.aggregate.selected(&projected) {
        if tab.source.workspace != s.active_workspace_key()
            || tab.source.tab != s.active_tab_id()
            || s.scenes.widget().visible_child_name().as_deref() == Some("aggregate-empty")
        {
            select_source(s, tab);
        }
    } else if s.scenes.widget().visible_child_name().as_deref() != Some("aggregate-empty") {
        show(s, kind);
    }
}

fn select_source(s: &mut UiState, tab: &AggregateTab) {
    let Some(id) = parse_workspace_id(&tab.source.workspace) else {
        return;
    };
    let kind = s.aggregate.kind;
    show_workspace_scene(s, id, false);
    s.aggregate.kind = kind;
    s.aggregate.remember(tab.source.clone());
    request_switch_tab(s, tab.source.tab);
    maybe_refresh_status(s, true);
}

pub(super) fn select(s: &mut UiState, display_id: u32) {
    if let Some(tab) = tabs(s).get(display_id.saturating_sub(1) as usize) {
        select_source(s, tab);
    }
}

pub(super) fn close(s: &mut UiState, display_id: u32) {
    let Some(tab) = tabs(s).get(display_id.saturating_sub(1) as usize).cloned() else {
        return;
    };
    if s.aggregate.kind == Some(AggregateKind::Agents) {
        s.aggregate.hidden_agents.insert(tab.source);
        show(s, AggregateKind::Agents);
    } else {
        s.command_queue
            .borrow_mut()
            .push(crate::frontend::command_queue::ClientCommand::Task {
                workspace_id: Some(tab.source.workspace),
                task: crate::frontend::utils::corebridge::ClientTask::CloseTab {
                    tab_id: tab.source.tab,
                },
            });
    }
}
