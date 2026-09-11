//! QuickConnect 面板的行视图。
//!
//! 面板状态、筛选和导航留在 `quickconnect_panel`；这里仅负责把已经选定的
//! 数据行投影成 GTK widget，避免页面控制器继续承担绘制细节。

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Label, Orientation, Overlay, Window};

use crate::frontend::i18n::{self, Key as TextKey};
use crate::frontend::linux::panel_model::AttentionPanelRow;
use crate::frontend::linux::quickconnect::existing::{ExistingEntry, ExistingTransport};
use crate::frontend::linux::quickconnect::model::{
    QuickBadge, QuickConnect, QuickConnectEntry, TargetTransport,
};
use crate::frontend::linux::workspace_sidebar::ActivityIndicator;
use crate::frontend::ssh_probe::{ssh_dot_css_class, ssh_dot_widget_name, SshReach};

pub(super) fn attention_panel_row(item: &AttentionPanelRow) -> GtkBox {
    let content = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_top(4)
        .margin_bottom(4)
        .build();
    let labels = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(1)
        .hexpand(true)
        .build();
    let title = Label::builder()
        .label(&item.title)
        .halign(Align::Start)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk4::pango::EllipsizeMode::End)
        .max_width_chars(super::PANEL_TEXT_MAX_CHARS)
        .build();
    title.set_widget_name("muxterm-attention-title");
    title.add_css_class("quick-pick-label");
    let detail = Label::builder()
        .label(&item.detail)
        .halign(Align::Start)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk4::pango::EllipsizeMode::End)
        .max_width_chars(super::PANEL_TEXT_MAX_CHARS)
        .build();
    detail.set_widget_name("muxterm-attention-detail");
    detail.add_css_class("quick-pick-detail");
    detail.set_tooltip_text(Some(&item.detail));
    labels.append(&title);
    labels.append(&detail);
    if item.indicator != ActivityIndicator::None {
        let dot = Label::new(Some("●"));
        dot.set_widget_name("muxterm-attention-status-dot");
        dot.add_css_class("muxterm-sidebar-agent-dot");
        dot.add_css_class(match item.indicator {
            ActivityIndicator::Running => "running",
            ActivityIndicator::Done => "done",
            ActivityIndicator::None => unreachable!("None does not create a status dot"),
        });
        content.append(&dot);
    }
    content.append(&labels);
    content
}

pub(super) fn target_row(
    entry: &QuickConnectEntry,
    is_current: bool,
    reach: Option<SshReach>,
) -> GtkBox {
    let col = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(1)
        .margin_start(12)
        .margin_end(12)
        .margin_top(4)
        .margin_bottom(4)
        .build();
    if is_current {
        col.add_css_class("qc-current-row");
    }
    let title_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(6)
        .build();
    // SSH 可达性灯（W15d）：与 host picker 共用 ssh_dot_widget_name / ssh_dot_css_class。
    if let (Some(reach), TargetTransport::Ssh { name }) = (reach, &entry.draft.transport) {
        let dot = Label::new(Some("●"));
        dot.set_widget_name(&ssh_dot_widget_name(name));
        dot.add_css_class(ssh_dot_css_class(reach));
        dot.set_tooltip_text(Some(match reach {
            SshReach::Ok => "SSH reachable",
            SshReach::Err => "SSH unreachable",
            SshReach::Unknown => "SSH reachability unknown",
        }));
        title_row.append(&dot);
    }
    let name = Label::new(Some(&entry.draft.name));
    name.set_halign(Align::Start);
    name.add_css_class("qc-name");
    title_row.append(&name);
    for badge in &entry.badges {
        let b = Label::new(Some(&badge_label(*badge)));
        b.add_css_class("qc-badge");
        match badge {
            QuickBadge::Recent => b.add_css_class("qc-badge-recent"),
            QuickBadge::Project => b.add_css_class("qc-badge-project"),
        }
        title_row.append(&b);
    }
    if is_current {
        let cur = Label::new(Some(&i18n::tr(TextKey::Current).to_uppercase()));
        cur.add_css_class("qc-badge");
        cur.add_css_class("qc-badge-current");
        title_row.append(&cur);
    }
    let subtitle = QuickConnect::subtitle(&entry.draft);
    let detail = if entry.draft.path.trim().is_empty() {
        subtitle
    } else {
        format!("{subtitle} · {}", entry.draft.path)
    };
    let sub = Label::new(Some(&detail));
    sub.set_halign(Align::Start);
    sub.set_hexpand(true);
    sub.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    sub.add_css_class("qc-sub");
    col.append(&title_row);
    col.append(&sub);
    col
}

/// W20：已有的连接行（title + `runtime @ transport` 副标题，与 Project 行同款）。
/// C9：connect name：本机 "local" / SSH Host alias。
pub(super) fn existing_connect_name(entry: &ExistingEntry) -> String {
    match &entry.transport {
        ExistingTransport::Local => "local".to_string(),
        ExistingTransport::Ssh { name } => name.clone(),
    }
}

pub(super) fn existing_row(entry: &ExistingEntry) -> GtkBox {
    let col = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(1)
        .margin_start(12)
        .margin_end(12)
        .margin_top(4)
        .margin_bottom(4)
        .build();
    let title_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(6)
        .build();
    if let ExistingTransport::Ssh { name } = &entry.transport {
        let dot = Label::new(Some("●"));
        dot.set_widget_name(&ssh_dot_widget_name(name));
        dot.add_css_class(ssh_dot_css_class(SshReach::Unknown));
        title_row.append(&dot);
    }
    let name = Label::new(Some(&entry.title));
    name.set_halign(Align::Start);
    name.add_css_class("qc-name");
    title_row.append(&name);
    let connect = existing_connect_name(entry);
    let sub = Label::new(Some(&format!("{} @ {}", entry.runtime.as_str(), connect)));
    sub.set_halign(Align::Start);
    sub.add_css_class("qc-sub");
    col.append(&title_row);
    col.append(&sub);
    col
}

/// W20：SSH host 行可达性灯（与 host picker 同款）。
pub(super) fn reachability_dot(reach: SshReach) -> Label {
    let dot = Label::new(Some("●"));
    dot.add_css_class(ssh_dot_css_class(reach));
    dot.set_tooltip_text(Some(match reach {
        SshReach::Ok => "SSH reachable",
        SshReach::Err => "SSH unreachable",
        SshReach::Unknown => "SSH reachability unknown",
    }));
    dot
}

fn badge_label(badge: QuickBadge) -> String {
    match badge {
        QuickBadge::Recent => i18n::tr(TextKey::Recent).to_uppercase(),
        QuickBadge::Project => i18n::tr(TextKey::Project).to_uppercase(),
    }
}

pub(super) fn ensure_overlay(parent: &Window) -> Overlay {
    crate::frontend::linux::quick_pick::ensure_overlay(parent)
}
