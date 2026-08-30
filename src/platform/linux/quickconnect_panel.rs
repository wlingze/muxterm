//! QuickConnect 面板：Recent + Project 快速连接（GTK Overlay）。
//!
//! 行为对齐 macOS `QuickConnectController`：搜索、badges、当前连接高亮、
//! 回车连接、双击编辑、末行 New Project。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gtk4::gdk::Key;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Entry, EventControllerKey, GestureClick, Label, ListBox, ListBoxRow,
    Orientation, Overlay, ScrolledWindow, SelectionMode, Window,
};

use crate::core::attention::engine::PaneAttention;
use crate::core::attention::state::PaneStatus;
use crate::core::discovery::existing::ExistingEntry;
use crate::core::transport::ssh::probe::SshReach;
use crate::platform::i18n::{self, Key as TextKey};
use crate::platform::linux::panel_model::{
    filter_attention_panel_rows, filter_workspace_rows, search_rows, AttentionPanelRow, PanelModel,
    PanelTab, SearchRow, SearchScope,
};
use crate::platform::linux::quick_pick;
use crate::platform::linux::quickconnect::model::{
    QuickBadge, QuickConnect, QuickConnectEntry, TargetConfig, TargetRuntime, TargetTransport,
};
use crate::platform::linux::quickconnect::store::QuickConnectStore;
use crate::platform::linux::workspace_sidebar::{AgentIndicator, AgentSidebarItem};

const NEW_PROJECT_ID: &str = "__new_project__";
const PANEL_ENTRY_HEIGHT: i32 = 36;
const PANEL_MAX_WIDTH: i32 = 640;
const PANEL_TEXT_MAX_CHARS: i32 = 64;

/// ListBox 保留搜索框焦点时不会自动跟随选中行滚动；按行坐标只移动
/// 必要距离，确保键盘选中的整行始终留在 ScrolledWindow 视口内。
fn reveal_selected_row(scroller: &ScrolledWindow, list: &ListBox, row: &ListBoxRow) {
    let Some(bounds) = row.compute_bounds(list) else {
        return;
    };
    scroller.vadjustment().clamp_page(
        f64::from(bounds.y()),
        f64::from(bounds.y() + bounds.height()),
    );
}

#[derive(Clone)]
enum VisibleAction {
    Connect(TargetConfig),
    ExistingConnect(TargetConfig),
    NewProject,
    Navigate(ExistingNav),
    Jump {
        workspace_id: String,
        pane_id: u32,
        seq: u64,
    },
    None,
}

type SearchCb = Box<dyn Fn(&str, SearchScope) -> Vec<SearchRow>>;

thread_local! {
    static PANEL_DISMISS: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

thread_local! {
    static PANEL_REFRESH: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

/// 已有的连接面板共享状态（window 侧更新，面板 rebuild 读取）。
#[derive(Debug, Clone, Default)]
pub struct ExistingPanelState {
    pub nav: ExistingNav,
    pub locals: Vec<ExistingEntry>,
    pub hosts: Vec<String>,
    pub remote: std::collections::HashMap<String, Vec<ExistingEntry>>,
    /// SSH 探测是否在跑：空 host + inflight → Loading；空 + 完成 → Empty。
    pub probe_inflight: bool,
}

/// 测试/生产共用：让当前面板按最新状态重建列表（SSH 探测回来再填）。
pub fn refresh_current() {
    PANEL_REFRESH.with(|slot| {
        if let Some(refresh) = slot.borrow().as_ref() {
            refresh();
        }
    });
}

/// 测试/生产共用：关闭当前 QuickConnect 面板（AppWindow 跳转后关面板，W15b）。
///
/// 独立面板测试（`linux_search_e2e`）不调用它，面板保持打开以便量宽度。
pub fn close_current() {
    PANEL_DISMISS.with(|slot| {
        if let Some(dismiss) = slot.borrow().as_ref() {
            dismiss();
        }
    });
    clear_panel_hooks();
}

/// 窗口销毁前拆掉 thread_local 回调，避免探测线程 refresh 已死 GTK 控件。
pub fn clear_panel_hooks() {
    PANEL_REFRESH.with(|slot| *slot.borrow_mut() = None);
    PANEL_DISMISS.with(|slot| *slot.borrow_mut() = None);
}

/// 面板回调。
pub struct QuickConnectCallbacks {
    pub on_connect: Box<dyn Fn(TargetConfig)>,
    pub on_edit: Box<dyn Fn(TargetConfig)>,
    pub on_new_project: Box<dyn Fn()>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelItem {
    Target(QuickConnectEntry, bool),
    NewProject,
    /// 目录（已有的连接 / 本地 / SSH）。
    Folder {
        id: &'static str,
        title: String,
    },
    /// 子目录返回。
    Back,
    /// 一条活着的 tmux session 或 Herdr workspace。
    Existing(ExistingEntry),
    /// SSH host 行（探测到至少一条 tmux 或 Herdr）。
    Host {
        alias: String,
    },
    /// SSH 探测中占位。
    Loading,
    /// 空目录占位。
    Empty {
        title: String,
    },
}

/// 已有的连接导航状态（纯逻辑）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ExistingNav {
    #[default]
    Root,
    Home,
    Local,
    SshHosts,
    SshHost {
        alias: String,
    },
}

/// 按查询过滤（纯逻辑，便于单测）。
pub(crate) fn filter_panel_items(items: &[PanelItem], query: &str) -> Vec<PanelItem> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return items.to_vec();
    }
    items
        .iter()
        .filter(|item| match item {
            PanelItem::Target(entry, _) => QuickConnect::search_text(&entry.config).contains(&q),
            PanelItem::NewProject => {
                let label = format!(
                    "new project {}",
                    i18n::tr(TextKey::NewProject).to_lowercase()
                );
                label.contains(&q)
            }
            PanelItem::Folder { title, .. } => title.to_lowercase().contains(&q),
            PanelItem::Back => true,
            PanelItem::Existing(e) => format!("{} {}", e.title, e.subtitle())
                .to_lowercase()
                .contains(&q),
            PanelItem::Host { alias } => alias.to_lowercase().contains(&q),
            PanelItem::Loading => true,
            PanelItem::Empty { title } => title.to_lowercase().contains(&q),
        })
        .cloned()
        .collect()
}

pub fn build_items(store: &QuickConnectStore, current: Option<&TargetConfig>) -> Vec<PanelItem> {
    let current_id = current.map(QuickConnect::unique_id);
    let mut items: Vec<PanelItem> = QuickConnect::entries(&store.recents, &store.projects, 5)
        .into_iter()
        .map(|entry| {
            let is_current = current_id
                .as_ref()
                .is_some_and(|id| QuickConnect::unique_id(&entry.config) == *id);
            PanelItem::Target(entry, is_current)
        })
        .collect();
    items.push(PanelItem::NewProject);
    items
}

/// W20b：根列表 = 第一项「已有的连接」Folder + 原 Recent/Project + New Project。
pub fn build_root_items(
    store: &QuickConnectStore,
    current: Option<&TargetConfig>,
) -> Vec<PanelItem> {
    let mut items = vec![PanelItem::Folder {
        id: "existing-connections",
        title: i18n::tr(TextKey::ExistingConnections),
    }];
    items.extend(build_items(store, current));
    items
}

/// W20c：已有的连接子目录内容（纯函数，可单测）。
///
/// - Home：Back + Local + SSH 两个 Folder
/// - Local：Back + 本地 tmux/Herdr 行（空 → Empty）
/// - SshHosts：Back + 探测到的 Host 行（探测中 → Loading）
/// - SshHost{alias}：Back + 该 host 的 tmux/Herdr 行
pub fn existing_items(
    nav: ExistingNav,
    locals: &[ExistingEntry],
    hosts: &[String],
    probe_inflight: bool,
    remote_of_alias: impl Fn(&str) -> Vec<ExistingEntry>,
) -> Vec<PanelItem> {
    let mut items = vec![PanelItem::Back];
    match nav {
        ExistingNav::Root | ExistingNav::Home => {
            // C9：扁平 runtime list。locals + 每个 connect 的远端行，双份不去重。
            let mut rows: Vec<ExistingEntry> = locals.to_vec();
            for host in hosts {
                rows.extend(remote_of_alias(host));
            }
            if rows.is_empty() {
                if probe_inflight {
                    items.push(PanelItem::Loading);
                } else {
                    items.push(PanelItem::Empty {
                        title: i18n::tr(TextKey::ExistingEmpty),
                    });
                }
            } else {
                items.extend(rows.into_iter().map(PanelItem::Existing));
            }
        }
        ExistingNav::Local => {
            if locals.is_empty() {
                items.push(PanelItem::Empty {
                    title: i18n::tr(TextKey::ExistingEmpty),
                });
            } else {
                items.extend(locals.iter().cloned().map(PanelItem::Existing));
            }
        }
        ExistingNav::SshHosts => {
            if hosts.is_empty() {
                if probe_inflight {
                    items.push(PanelItem::Loading);
                } else {
                    items.push(PanelItem::Empty {
                        title: i18n::tr(TextKey::ExistingEmpty),
                    });
                }
            } else {
                items.extend(hosts.iter().map(|alias| PanelItem::Host {
                    alias: alias.clone(),
                }));
            }
        }
        ExistingNav::SshHost { alias } => {
            let rows = remote_of_alias(&alias);
            if rows.is_empty() {
                items.push(PanelItem::Empty {
                    title: i18n::tr(TextKey::ExistingEmpty),
                });
            } else {
                items.extend(rows.into_iter().map(PanelItem::Existing));
            }
        }
    }
    items
}

fn visible_action_for_item(item: &PanelItem, nav: &ExistingNav) -> VisibleAction {
    match item {
        PanelItem::Target(entry, _) => VisibleAction::Connect(entry.config.clone()),
        PanelItem::NewProject => VisibleAction::NewProject,
        PanelItem::Folder { id, .. } => match *id {
            "existing-connections" => VisibleAction::Navigate(ExistingNav::Home),
            "existing-local" => VisibleAction::Navigate(ExistingNav::Local),
            "existing-ssh" => VisibleAction::Navigate(ExistingNav::SshHosts),
            _ => VisibleAction::None,
        },
        PanelItem::Back => VisibleAction::Navigate(match nav {
            ExistingNav::Home => ExistingNav::Root,
            ExistingNav::Local | ExistingNav::SshHosts => ExistingNav::Home,
            ExistingNav::SshHost { .. } => ExistingNav::SshHosts,
            ExistingNav::Root => ExistingNav::Root,
        }),
        PanelItem::Existing(existing_entry) => {
            VisibleAction::ExistingConnect(existing_entry_to_config(existing_entry))
        }
        PanelItem::Host { alias } => VisibleAction::Navigate(ExistingNav::SshHost {
            alias: alias.clone(),
        }),
        PanelItem::Loading | PanelItem::Empty { .. } => VisibleAction::None,
    }
}

/// 弹出 QuickConnect 面板。
/// 三 tab 面板参数（LINUX-PLAN §10 C3.2/C3.3）。
pub struct PanelShowArgs {
    pub initial_tab: PanelTab,
    pub workspaces: Vec<PanelItem>,
    pub agents: Vec<AgentSidebarItem>,
    pub attention: Vec<PaneAttention>,
    pub on_connect: Box<dyn Fn(TargetConfig)>,
    /// Existing 行专用回调：必须使用 attach-only 意图。
    pub on_existing_connect: Box<dyn Fn(TargetConfig)>,
    pub on_edit: Box<dyn Fn(TargetConfig)>,
    pub on_new_project: Box<dyn Fn()>,
    /// 跳转回调：`(ws, pane, seq)`。seq 是搜索命中的 PaneBuf 行号（W17c），
    /// Attention 跳转没有搜索语义传 0。
    pub on_jump_pane: Box<dyn Fn(String, u32, u64)>,
    /// Search tab：query → replica 命中行。
    pub search: SearchCb,
    /// 面板关闭回调（window 侧清 panel_open 状态）。
    pub on_close: Box<dyn Fn()>,
    /// SSH 别名 → 可达性（测试注入；生产由后台探测填充）。
    pub ssh_reach: HashMap<String, SshReach>,
    /// 已有的连接共享状态（nav + 本地/SSH 数据）。
    pub existing: Rc<RefCell<ExistingPanelState>>,
    /// 导航变化回调（window 侧触发 SSH 探测等）。
    pub on_existing_nav: Box<dyn Fn(ExistingNav)>,
}

/// 弹出三 tab QuickConnect 面板（普通 Overlay，不构造 AppWindow）。
pub fn show(parent: &impl IsA<Window>, args: PanelShowArgs) {
    let parent = parent.as_ref();
    let parent_h = parent.height().max(400);
    let (panel_h, list_h) = quick_pick::panel_list_heights(parent_h);
    let parent_w = parent.width();
    let panel_w = if parent_w > 0 {
        parent_w.saturating_sub(32).clamp(1, PANEL_MAX_WIDTH)
    } else {
        PANEL_MAX_WIDTH
    };

    let PanelShowArgs {
        initial_tab,
        workspaces,
        agents,
        attention,
        on_connect,
        on_existing_connect,
        on_edit,
        on_new_project,
        on_jump_pane,
        search,
        on_close,
        ssh_reach,
        existing,
        on_existing_nav,
    } = args;
    let ssh_reach = Rc::new(ssh_reach);
    let existing = Rc::new(existing);

    let overlay = ensure_overlay(parent);
    let backdrop = GtkBox::new(Orientation::Vertical, 0);
    backdrop.set_hexpand(true);
    backdrop.set_vexpand(true);
    backdrop.add_css_class("quick-pick-backdrop");

    let panel = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(0)
        .halign(Align::Center)
        .valign(Align::Start)
        .build();
    panel.add_css_class("quick-pick-root");
    panel.set_widget_name("muxterm-panel");
    panel.set_margin_top(28);
    panel.set_size_request(panel_w, panel_h);
    panel.set_hexpand(false);
    panel.set_vexpand(false);
    panel.set_overflow(gtk4::Overflow::Hidden);

    let entry = Entry::builder()
        .placeholder_text(i18n::tr(TextKey::QuickConnectPlaceholder))
        .hexpand(true)
        .build();
    entry.set_widget_name("muxterm-panel-entry");
    entry.add_css_class("quick-pick-entry");
    entry.set_margin_start(10);
    entry.set_margin_end(10);
    entry.set_margin_top(8);
    entry.set_size_request(-1, PANEL_ENTRY_HEIGHT);
    panel.append(&entry);

    // 三 tab 按钮
    let tab_bar = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(4)
        .margin_start(10)
        .margin_end(10)
        .margin_top(6)
        .build();
    let tab_workspaces = gtk4::ToggleButton::with_label(&i18n::tr(TextKey::PanelTabWorkspaces));
    tab_workspaces.set_widget_name("muxterm-panel-tab-workspaces");
    let tab_attention = gtk4::ToggleButton::with_label(&i18n::tr(TextKey::PanelTabAttention));
    tab_attention.set_widget_name("muxterm-panel-tab-attention");
    let tab_search = gtk4::ToggleButton::with_label(&i18n::tr(TextKey::PanelTabSearch));
    tab_search.set_widget_name("muxterm-panel-tab-search");
    tab_bar.append(&tab_workspaces);
    tab_bar.append(&tab_attention);
    tab_bar.append(&tab_search);
    panel.append(&tab_bar);

    let list = ListBox::new();
    list.set_selection_mode(SelectionMode::Browse);
    list.add_css_class("quick-pick-list");
    list.set_widget_name("muxterm-panel-list");

    let sw = ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .child(&list)
        .build();
    sw.set_margin_top(6);
    sw.set_size_request(-1, list_h);
    panel.append(&sw);

    // Tab3 占位（搜索中…行，阶段 C 前固定显示占位文案）
    let search_status = Label::new(Some(&i18n::tr(TextKey::SearchPlaceholderPhaseC)));
    search_status.set_widget_name("muxterm-search-status");
    search_status.set_halign(Align::Start);
    search_status.set_margin_start(16);
    search_status.set_margin_top(10);
    search_status.set_margin_bottom(10);
    search_status.set_visible(false);
    panel.append(&search_status);

    // W18f：搜索范围（当前 pane / 本工作区 / 全部）。
    let scope_bar = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(4)
        .margin_start(12)
        .margin_end(12)
        .build();
    let scope_pane = gtk4::ToggleButton::with_label("pane");
    scope_pane.set_widget_name("muxterm-search-scope-pane");
    let scope_workspace = gtk4::ToggleButton::with_label("workspace");
    scope_workspace.set_widget_name("muxterm-search-scope-workspace");
    let scope_all = gtk4::ToggleButton::with_label("all");
    scope_all.set_widget_name("muxterm-search-scope-all");
    scope_bar.append(&scope_pane);
    scope_bar.append(&scope_workspace);
    scope_bar.append(&scope_all);
    scope_bar.set_visible(false);
    panel.append(&scope_bar);

    overlay.add_overlay(&backdrop);
    overlay.add_overlay(&panel);

    let model = Rc::new(RefCell::new(PanelModel::open(initial_tab)));
    let all = Rc::new(workspaces);
    let agents = Rc::new(agents);
    let attention = Rc::new(attention);
    let callbacks = Rc::new(PanelShowArgs {
        initial_tab,
        workspaces: Vec::new(),
        agents: Vec::new(),
        attention: Vec::new(),
        on_connect,
        on_existing_connect,
        on_edit,
        on_new_project,
        on_jump_pane,
        search,
        on_close: std::boxed::Box::new(|| {}),
        ssh_reach: HashMap::new(),
        existing: (*existing).clone(),
        on_existing_nav,
    });
    let finished = Rc::new(RefCell::new(false));

    let on_close = Rc::new(on_close);
    let dismiss = {
        let overlay = overlay.clone();
        let backdrop = backdrop.clone();
        let panel = panel.clone();
        let finished = finished.clone();
        let on_close = on_close.clone();
        move || {
            if *finished.borrow() {
                return;
            }
            *finished.borrow_mut() = true;
            overlay.remove_overlay(&backdrop);
            overlay.remove_overlay(&panel);
            on_close();
        }
    };
    PANEL_DISMISS.with(|slot| *slot.borrow_mut() = Some(Box::new(dismiss.clone())));

    {
        let dismiss = dismiss.clone();
        let gesture = GestureClick::new();
        gesture.connect_released(move |_, _, _, _| dismiss());
        backdrop.add_controller(gesture);
    }

    // 工作区级状态：blocked 优先于 done（从 attention 行推导）。
    let workspace_status = {
        let mut map: std::collections::HashMap<String, PaneStatus> =
            std::collections::HashMap::new();
        for p in attention.iter() {
            let entry = map
                .entry(p.workspace_id.clone())
                .or_insert(PaneStatus::Idle);
            if p.status == PaneStatus::Blocked {
                *entry = PaneStatus::Blocked;
            } else if *entry != PaneStatus::Blocked && p.status == PaneStatus::Done {
                *entry = PaneStatus::Done;
            }
        }
        map
    };

    let visible_actions = Rc::new(RefCell::new(Vec::<VisibleAction>::new()));
    let rebuild = {
        let list = list.clone();
        let model = model.clone();
        let all = all.clone();
        let agents = agents.clone();
        let attention = attention.clone();
        let callbacks = callbacks.clone();
        let dismiss = dismiss.clone();
        let search_status = search_status.clone();
        let tab_workspaces = tab_workspaces.clone();
        let tab_attention = tab_attention.clone();
        let tab_search = tab_search.clone();
        let workspace_status = workspace_status.clone();
        let ssh_reach = ssh_reach.clone();
        let scope_pane = scope_pane.clone();
        let scope_workspace = scope_workspace.clone();
        let scope_all = scope_all.clone();
        let scope_bar = scope_bar.clone();
        let existing = existing.clone();
        let entry = entry.clone();
        let visible_actions = visible_actions.clone();
        move || {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            let mut actions = Vec::new();
            // 先取 tab/query 并释放 RefCell，再 set_active（toggled 会重入 rebuild）。
            let (tab, query) = {
                let m = model.borrow();
                (m.tab, m.query.clone())
            };
            // W20：非 Root 时列表来自已有的连接子目录。
            let all: Vec<PanelItem> = {
                let ex = existing.borrow();
                if ex.nav == ExistingNav::Root {
                    (*all).clone()
                } else {
                    existing_items(
                        ex.nav.clone(),
                        &ex.locals,
                        &ex.hosts,
                        ex.probe_inflight,
                        |alias| ex.remote.get(alias).cloned().unwrap_or_default(),
                    )
                }
            };
            tab_workspaces.set_active(tab == PanelTab::Workspaces);
            tab_attention.set_active(tab == PanelTab::Attention);
            tab_search.set_active(tab == PanelTab::Search);
            search_status.set_visible(tab == PanelTab::Search);
            scope_bar.set_visible(tab == PanelTab::Search);
            let scope = model.borrow().scope;
            scope_pane.set_active(scope == SearchScope::Pane);
            scope_workspace.set_active(scope == SearchScope::Workspace);
            scope_all.set_active(scope == SearchScope::All);
            match tab {
                PanelTab::Workspaces => {
                    let rows = filter_workspace_rows(&all, &query, |item| {
                        let id = match item {
                            PanelItem::Target(entry, _) => QuickConnect::unique_id(&entry.config),
                            _ => return None,
                        };
                        workspace_status.get(&id).copied()
                    });
                    for (i, row) in rows.iter().enumerate() {
                        let row_widget = ListBoxRow::new();
                        row_widget.set_activatable(true);
                        actions.push(visible_action_for_item(&row.item, &existing.borrow().nav));
                        match &row.item {
                            PanelItem::Target(entry, is_current) => {
                                row_widget.set_widget_name(&QuickConnect::unique_id(&entry.config));
                                if *is_current {
                                    row_widget.add_css_class("qc-current");
                                }
                                let reach = match &entry.config.transport {
                                    TargetTransport::Ssh { name } => ssh_reach.get(name).copied(),
                                    TargetTransport::Local => None,
                                };
                                let boxed = target_row(entry, *is_current, reach);
                                if let Some(status) = row.status {
                                    let mark = Label::new(Some(match status {
                                        PaneStatus::Blocked => "● ",
                                        PaneStatus::Done => "✓ ",
                                        _ => "",
                                    }));
                                    mark.add_css_class("qc-status-mark");
                                    boxed.prepend(&mark);
                                }
                                row_widget.set_child(Some(&boxed));
                                let cfg = entry.config.clone();
                                let dismiss = dismiss.clone();
                                let on_edit = {
                                    let callbacks = callbacks.clone();
                                    let cfg = cfg.clone();
                                    let dismiss = dismiss.clone();
                                    move || {
                                        dismiss();
                                        (callbacks.on_edit)(cfg.clone());
                                    }
                                };
                                let dbl = GestureClick::new();
                                dbl.set_button(1);
                                dbl.connect_pressed(move |g, n, _, _| {
                                    if n == 2 {
                                        on_edit();
                                        g.set_state(gtk4::EventSequenceState::Claimed);
                                    }
                                });
                                row_widget.add_controller(dbl);
                            }
                            PanelItem::NewProject => {
                                row_widget.set_widget_name(NEW_PROJECT_ID);
                                let label = Label::new(Some(&format!(
                                    "＋ {}",
                                    i18n::tr(TextKey::NewProject)
                                )));
                                label.set_halign(Align::Start);
                                label.set_margin_start(12);
                                label.set_margin_top(6);
                                label.set_margin_bottom(6);
                                row_widget.set_child(Some(&label));
                            }
                            PanelItem::Folder { id, title } => {
                                row_widget.set_widget_name(&format!("muxterm-{id}"));
                                let label = Label::new(Some(title));
                                label.set_halign(Align::Start);
                                label.set_margin_start(12);
                                label.set_margin_top(6);
                                label.set_margin_bottom(6);
                                row_widget.set_child(Some(&label));
                            }
                            PanelItem::Back => {
                                row_widget.set_widget_name("muxterm-existing-back");
                                let label = Label::new(Some(&i18n::tr(TextKey::ExistingBack)));
                                label.set_halign(Align::Start);
                                label.set_margin_start(12);
                                label.set_margin_top(6);
                                label.set_margin_bottom(6);
                                row_widget.set_child(Some(&label));
                            }
                            PanelItem::Existing(entry) => {
                                let identity = entry
                                    .herdr_workspace_id
                                    .as_deref()
                                    .or(entry.tmux_session.as_deref())
                                    .unwrap_or(&entry.title);
                                // workspace ids are only unique inside a Herdr
                                // named session.  Keep the session in the
                                // widget identity so two sessions exposing
                                // `w1` cannot make the first row win attach.
                                let identity = if entry.runtime == TargetRuntime::Herdr {
                                    format!(
                                        "{}-{}",
                                        identity,
                                        entry.herdr_session.as_deref().unwrap_or("default")
                                    )
                                } else {
                                    identity.to_string()
                                };
                                row_widget.set_widget_name(&format!(
                                    "muxterm-existing-row-{}-{}-{}",
                                    entry.runtime.as_str(),
                                    existing_connect_name(entry),
                                    identity,
                                ));
                                let boxed = existing_row(entry);
                                row_widget.set_child(Some(&boxed));
                            }
                            PanelItem::Host { alias } => {
                                row_widget
                                    .set_widget_name(&format!("muxterm-existing-host-{alias}"));
                                let reach = ssh_reach.get(alias).copied();
                                let label = Label::new(Some(alias));
                                label.set_halign(Align::Start);
                                label.set_margin_start(12);
                                label.set_margin_top(6);
                                label.set_margin_bottom(6);
                                let boxed = GtkBox::new(Orientation::Horizontal, 8);
                                boxed.append(&label);
                                if let Some(reach) = reach {
                                    let dot = reachability_dot(reach);
                                    boxed.append(&dot);
                                }
                                row_widget.set_child(Some(&boxed));
                            }
                            PanelItem::Loading => {
                                row_widget.set_widget_name("muxterm-existing-ssh-loading");
                                row_widget.set_activatable(false);
                                let label = Label::new(Some(&i18n::tr(TextKey::ExistingProbing)));
                                label.set_halign(Align::Start);
                                label.set_margin_start(12);
                                label.set_margin_top(6);
                                label.set_margin_bottom(6);
                                row_widget.set_child(Some(&label));
                            }
                            PanelItem::Empty { title } => {
                                row_widget.set_widget_name("muxterm-existing-empty");
                                row_widget.set_activatable(false);
                                let label = Label::new(Some(title));
                                label.set_halign(Align::Start);
                                label.set_margin_start(12);
                                label.set_margin_top(6);
                                label.set_margin_bottom(6);
                                row_widget.set_child(Some(&label));
                            }
                        }
                        list.append(&row_widget);
                        if i == 0 {
                            list.select_row(Some(&row_widget));
                        }
                    }
                }
                PanelTab::Attention => {
                    let rows = filter_attention_panel_rows(&agents, &attention, &query);
                    let count = Label::new(Some(&i18n::tr_args(
                        TextKey::AttentionCount,
                        &[("n", &rows.len().to_string())],
                    )));
                    count.set_halign(Align::Start);
                    count.add_css_class("qc-attention-count");
                    count.set_margin_start(12);
                    count.set_margin_top(4);
                    count.set_margin_bottom(2);
                    let count_row = ListBoxRow::new();
                    count_row.set_activatable(false);
                    count_row.set_child(Some(&count));
                    list.append(&count_row);
                    actions.push(VisibleAction::None);
                    if rows.is_empty() {
                        let empty = Label::new(Some(&i18n::tr(TextKey::AttentionEmpty)));
                        empty.set_halign(Align::Start);
                        empty.set_margin_start(12);
                        empty.set_margin_top(6);
                        empty.set_margin_bottom(6);
                        let empty_row = ListBoxRow::new();
                        empty_row.set_activatable(false);
                        empty_row.set_child(Some(&empty));
                        list.append(&empty_row);
                        actions.push(VisibleAction::None);
                    }
                    for (i, row) in rows.iter().enumerate() {
                        let row_widget = ListBoxRow::new();
                        row_widget.set_activatable(true);
                        row_widget.set_widget_name(&format!(
                            "muxterm-attention-{}-{}",
                            row.workspace_id, row.pane_id
                        ));
                        row_widget.set_child(Some(&attention_panel_row(row)));
                        list.append(&row_widget);
                        actions.push(VisibleAction::Jump {
                            workspace_id: row.workspace_id.clone(),
                            pane_id: row.pane_id,
                            seq: 0,
                        });
                        if i == 0 {
                            list.select_row(Some(&row_widget));
                        }
                    }
                }
                PanelTab::Search => {
                    let scope = model.borrow().scope;
                    let hits = (callbacks.search)(&query, scope);
                    let (rows, placeholder) = search_rows(&query, hits);
                    search_status.set_visible(placeholder);
                    for (i, row) in rows.iter().enumerate() {
                        let row_widget = ListBoxRow::new();
                        row_widget.set_widget_name(&format!(
                            "muxterm-search-hit-{}-{}-{}",
                            row.workspace_id, row.pane_id, row.seq
                        ));
                        let text = format!("{} · {} · {}", row.workspace_id, row.pane_id, row.line);
                        let label = Label::new(Some(&text));
                        label.set_halign(Align::Start);
                        label.set_hexpand(true);
                        label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                        label.set_max_width_chars(PANEL_TEXT_MAX_CHARS);
                        label.set_margin_start(12);
                        label.set_margin_top(5);
                        label.set_margin_bottom(5);
                        row_widget.set_child(Some(&label));
                        list.append(&row_widget);
                        actions.push(VisibleAction::Jump {
                            workspace_id: row.workspace_id.clone(),
                            pane_id: row.pane_id,
                            seq: row.seq,
                        });
                        if i == 0 {
                            list.select_row(Some(&row_widget));
                        }
                    }
                }
            }
            *visible_actions.borrow_mut() = actions;
            entry.grab_focus();
        }
    };
    rebuild();
    let rebuild_pending = Rc::new(Cell::new(false));
    let schedule_rebuild = {
        let rebuild = rebuild.clone();
        let rebuild_pending = rebuild_pending.clone();
        let finished = finished.clone();
        move || {
            if *finished.borrow() || rebuild_pending.replace(true) {
                return;
            }
            let rebuild = rebuild.clone();
            let rebuild_pending = rebuild_pending.clone();
            let finished = finished.clone();
            glib::idle_add_local_once(move || {
                rebuild_pending.set(false);
                if !*finished.borrow() {
                    rebuild();
                }
            });
        }
    };
    {
        let schedule_rebuild = schedule_rebuild.clone();
        PANEL_REFRESH.with(|slot| *slot.borrow_mut() = Some(Box::new(schedule_rebuild)));
    }

    // 搜索框持续持有输入焦点，因此 ListBox 自身不会替选中行滚动。
    // 所有键盘/程序化选中统一从这里保证可见。
    {
        let sw = sw.clone();
        list.connect_row_selected(move |list, row| {
            if let Some(row) = row {
                reveal_selected_row(&sw, list, row);
            }
        });
    }

    {
        let model = model.clone();
        let schedule_rebuild = schedule_rebuild.clone();
        entry.connect_changed(move |e| {
            model.borrow_mut().query = e.text().to_string();
            schedule_rebuild();
        });
    }

    // 搜索范围按钮：切换 scope 并重建（W18f）。
    for (btn, scope) in [
        (scope_pane.clone(), SearchScope::Pane),
        (scope_workspace.clone(), SearchScope::Workspace),
        (scope_all.clone(), SearchScope::All),
    ] {
        let model = model.clone();
        let schedule_rebuild = schedule_rebuild.clone();
        btn.connect_toggled(move |b| {
            if b.is_active() {
                let changed = {
                    let mut m = model.borrow_mut();
                    if m.scope == scope {
                        false
                    } else {
                        m.scope = scope;
                        true
                    }
                };
                if changed {
                    schedule_rebuild();
                }
            }
        });
    }

    // tab 按钮点击
    for (btn, tab) in [
        (tab_workspaces.clone(), PanelTab::Workspaces),
        (tab_attention.clone(), PanelTab::Attention),
        (tab_search.clone(), PanelTab::Search),
    ] {
        let model = model.clone();
        let schedule_rebuild = schedule_rebuild.clone();
        btn.connect_toggled(move |b| {
            if b.is_active() {
                // 只有 tab 真正变化才重建，避免 set_active 重入死循环。
                let changed = {
                    let mut m = model.borrow_mut();
                    if m.tab == tab {
                        false
                    } else {
                        m.tab = tab;
                        true
                    }
                };
                if changed {
                    schedule_rebuild();
                }
            }
        });
    }

    let activate = {
        let list = list.clone();
        let callbacks = callbacks.clone();
        let dismiss = dismiss.clone();
        let existing = existing.clone();
        let visible_actions = visible_actions.clone();
        let schedule_rebuild = schedule_rebuild.clone();
        move || {
            let Some(row) = list.selected_row() else {
                return;
            };
            let idx = row.index() as usize;
            let action = visible_actions
                .borrow()
                .get(idx)
                .cloned()
                .unwrap_or(VisibleAction::None);
            match action {
                VisibleAction::Connect(config) => {
                    dismiss();
                    (callbacks.on_connect)(config);
                }
                VisibleAction::ExistingConnect(config) => {
                    dismiss();
                    (callbacks.on_existing_connect)(config);
                }
                VisibleAction::NewProject => {
                    dismiss();
                    (callbacks.on_new_project)();
                }
                VisibleAction::Navigate(next) => {
                    existing.borrow_mut().nav = next.clone();
                    (callbacks.on_existing_nav)(next);
                    schedule_rebuild();
                }
                VisibleAction::Jump {
                    workspace_id,
                    pane_id,
                    seq,
                } => {
                    // 搜索跳转由 window 侧关闭；独立面板测试会保留面板量宽度。
                    (callbacks.on_jump_pane)(workspace_id, pane_id, seq);
                }
                VisibleAction::None => {}
            }
        }
    };

    list.connect_row_activated({
        let activate = activate.clone();
        move |_, _| activate()
    });

    {
        let dismiss = dismiss.clone();
        let activate = activate.clone();
        let model = model.clone();
        let schedule_rebuild = schedule_rebuild.clone();
        let list = list.clone();
        let entry = entry.clone();
        let entry_for_keys = entry.clone();
        let controller = EventControllerKey::new();
        controller.connect_key_pressed(move |_c, key, _code, mods| match key {
            Key::Escape => {
                dismiss();
                glib::Propagation::Stop
            }
            Key::Return | Key::KP_Enter => {
                activate();
                glib::Propagation::Stop
            }
            Key::Tab => {
                model
                    .borrow_mut()
                    .cycle_tab(mods.contains(gtk4::gdk::ModifierType::SHIFT_MASK));
                schedule_rebuild();
                glib::Propagation::Stop
            }
            Key::Up | Key::Down => {
                let mut rows = 0i32;
                while list.row_at_index(rows).is_some() {
                    rows += 1;
                }
                if rows == 0 {
                    return glib::Propagation::Stop;
                }
                let step = if key == Key::Down { 1 } else { -1 };
                let mut next = if let Some(row) = list.selected_row() {
                    row.index() + step
                } else if step > 0 {
                    0
                } else {
                    rows - 1
                };
                while next >= 0 && next < rows {
                    let Some(row) = list.row_at_index(next) else {
                        break;
                    };
                    if row.is_activatable() {
                        list.select_row(Some(&row));
                        break;
                    }
                    next += step;
                }
                entry_for_keys.grab_focus();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        entry.add_controller(controller);
    }

    entry.grab_focus();
    gtk4::prelude::GtkWindowExt::set_focus(parent, Some(&entry));
    let parent_focus = parent.clone();
    let entry_focus = entry.clone();
    glib::timeout_add_local_once(Duration::from_millis(1), move || {
        gtk4::prelude::GtkWindowExt::set_focus(&parent_focus, Some(&entry_focus));
        entry_focus.grab_focus();
    });
}

fn attention_panel_row(item: &AttentionPanelRow) -> GtkBox {
    let content = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_top(4)
        .margin_bottom(4)
        .build();
    let dot = Label::new(Some("●"));
    dot.set_widget_name("muxterm-attention-status-dot");
    dot.add_css_class("muxterm-sidebar-agent-dot");
    dot.add_css_class(match item.indicator {
        AgentIndicator::Running => "running",
        AgentIndicator::NeedsAttention => "needs-attention",
        AgentIndicator::None => "seen",
    });

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
        .max_width_chars(PANEL_TEXT_MAX_CHARS)
        .build();
    title.set_widget_name("muxterm-attention-title");
    title.add_css_class("quick-pick-label");
    let detail = Label::builder()
        .label(&item.detail)
        .halign(Align::Start)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk4::pango::EllipsizeMode::End)
        .max_width_chars(PANEL_TEXT_MAX_CHARS)
        .build();
    detail.set_widget_name("muxterm-attention-detail");
    detail.add_css_class("quick-pick-detail");
    detail.set_tooltip_text(Some(&item.detail));
    labels.append(&title);
    labels.append(&detail);
    content.append(&dot);
    content.append(&labels);
    content
}

fn target_row(entry: &QuickConnectEntry, is_current: bool, reach: Option<SshReach>) -> GtkBox {
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
    if let (Some(reach), TargetTransport::Ssh { name }) = (reach, &entry.config.transport) {
        let dot = Label::new(Some("●"));
        dot.set_widget_name(&crate::core::transport::ssh::probe::ssh_dot_widget_name(
            name,
        ));
        dot.add_css_class(crate::core::transport::ssh::probe::ssh_dot_css_class(reach));
        dot.set_tooltip_text(Some(match reach {
            SshReach::Ok => "SSH reachable",
            SshReach::Err => "SSH unreachable",
            SshReach::Unknown => "SSH reachability unknown",
        }));
        title_row.append(&dot);
    }
    let name = Label::new(Some(&entry.config.name));
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
    let subtitle = QuickConnect::subtitle(&entry.config);
    let detail = if entry.config.path.trim().is_empty() {
        subtitle
    } else {
        format!("{subtitle} · {}", entry.config.path)
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
fn existing_connect_name(entry: &ExistingEntry) -> String {
    match &entry.transport {
        TargetTransport::Local => "local".to_string(),
        TargetTransport::Ssh { name } => name.clone(),
    }
}

fn existing_row(entry: &ExistingEntry) -> GtkBox {
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
    if let TargetTransport::Ssh { name } = &entry.transport {
        let dot = Label::new(Some("●"));
        dot.set_widget_name(&crate::core::transport::ssh::probe::ssh_dot_widget_name(
            name,
        ));
        dot.add_css_class(crate::core::transport::ssh::probe::ssh_dot_css_class(
            SshReach::Unknown,
        ));
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
fn reachability_dot(reach: SshReach) -> Label {
    let dot = Label::new(Some("●"));
    dot.add_css_class(crate::core::transport::ssh::probe::ssh_dot_css_class(reach));
    dot.set_tooltip_text(Some(match reach {
        SshReach::Ok => "SSH reachable",
        SshReach::Err => "SSH unreachable",
        SshReach::Unknown => "SSH reachability unknown",
    }));
    dot
}

/// W20：ExistingEntry → TargetConfig（attach only；socket/session 带上）。
fn existing_entry_to_config(entry: &ExistingEntry) -> TargetConfig {
    let mut cfg = TargetConfig::new(
        entry.title.clone(),
        entry.runtime,
        entry.transport.clone(),
        if entry.runtime == TargetRuntime::Herdr {
            String::new()
        } else {
            "~".into()
        },
    );
    cfg.socket = match entry.runtime {
        TargetRuntime::Tmux => entry.tmux_socket.clone(),
        TargetRuntime::Herdr => entry.herdr_socket.clone(),
        TargetRuntime::Shell => None,
    };
    cfg.session = match entry.runtime {
        TargetRuntime::Tmux => entry.tmux_session.clone(),
        TargetRuntime::Herdr => entry.herdr_session.clone(),
        TargetRuntime::Shell => None,
    };
    cfg.workspace_id = entry.herdr_workspace_id.clone();
    cfg
}

fn badge_label(badge: QuickBadge) -> String {
    match badge {
        QuickBadge::Recent => i18n::tr(TextKey::Recent).to_uppercase(),
        QuickBadge::Project => i18n::tr(TextKey::Project).to_uppercase(),
    }
}

fn ensure_overlay(parent: &Window) -> Overlay {
    crate::platform::linux::quick_pick::ensure_overlay(parent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::linux::quickconnect::model::{TargetRuntime, TargetTransport};

    fn cfg(name: &str) -> TargetConfig {
        TargetConfig::new(name, TargetRuntime::Tmux, TargetTransport::Local, "~/x")
    }

    #[test]
    fn filter_keeps_new_project_on_empty_query() {
        let items = vec![
            PanelItem::Target(
                QuickConnectEntry::new(cfg("muxterm"), vec![QuickBadge::Project]),
                false,
            ),
            PanelItem::NewProject,
        ];
        assert_eq!(filter_panel_items(&items, "").len(), 2);
        let hit = filter_panel_items(&items, "mux");
        assert_eq!(hit.len(), 1);
        assert!(matches!(hit[0], PanelItem::Target(_, _)));
        let new_only = filter_panel_items(&items, "new");
        assert_eq!(new_only.len(), 1);
        assert!(matches!(new_only[0], PanelItem::NewProject));
    }

    #[test]
    fn build_items_marks_current_and_appends_new_project() {
        let mut store = QuickConnectStore::in_memory();
        let recent = cfg("recent");
        let project = cfg("project");
        store.recents.push(recent.clone());
        store.projects.push(project.clone());
        let items = build_items(&store, Some(&recent));
        assert_eq!(items.len(), 3);
        assert!(matches!(
            &items[0],
            PanelItem::Target(entry, true) if entry.config == recent
        ));
        assert!(matches!(
            &items[1],
            PanelItem::Target(entry, false) if entry.config == project
        ));
        assert!(matches!(items[2], PanelItem::NewProject));
    }

    #[test]
    fn build_items_dedupes_recent_and_project() {
        let mut store = QuickConnectStore::in_memory();
        let dup = cfg("dup");
        store.recents.push(dup.clone());
        store.projects.push(dup.clone());
        let items = build_items(&store, None);
        assert_eq!(items.len(), 2, "重复目标只出现一次 + New Project");
        assert!(matches!(&items[0], PanelItem::Target(entry, false) if entry.config == dup));
        assert!(matches!(items[1], PanelItem::NewProject));
    }

    /// W20b：根列表第 0 项是「已有的连接」Folder，末项 New Project。
    #[test]
    fn build_root_items_puts_existing_connections_first() {
        let mut store = QuickConnectStore::in_memory();
        let project = cfg("project");
        store.projects.push(project.clone());
        let items = build_root_items(&store, None);
        assert_eq!(items.len(), 3, "Folder + project + NewProject");
        assert!(matches!(
            &items[0],
            PanelItem::Folder {
                id: "existing-connections",
                ..
            }
        ));
        assert!(matches!(items[2], PanelItem::NewProject));
    }

    /// C9：Home 是扁平 runtime list，不是 local/SSH 目录。
    #[test]
    fn existing_items_home_is_flat_local_and_ssh_self() {
        let local = ExistingEntry {
            title: "mux-dup".into(),
            runtime: TargetRuntime::Tmux,
            transport: TargetTransport::Local,
            tmux_session: Some("mux-dup".into()),
            tmux_socket: None,
            herdr_session: None,
            herdr_workspace_id: None,
            herdr_socket: None,
        };
        let ssh_self = ExistingEntry {
            title: "mux-dup".into(),
            runtime: TargetRuntime::Tmux,
            transport: TargetTransport::Ssh {
                name: "self".into(),
            },
            tmux_session: Some("mux-dup".into()),
            tmux_socket: None,
            herdr_session: None,
            herdr_workspace_id: None,
            herdr_socket: None,
        };
        let items = existing_items(
            ExistingNav::Home,
            &[local],
            &["self".to_string()],
            false,
            |_| vec![ssh_self.clone()],
        );
        assert!(matches!(items[0], PanelItem::Back));
        assert!(
            !items.iter().any(|i| matches!(
                i,
                PanelItem::Folder {
                    id: "existing-local" | "existing-ssh",
                    ..
                }
            )),
            "禁止本地/SSH 目录: {items:?}"
        );
        assert!(
            !items.iter().any(|i| matches!(i, PanelItem::Host { .. })),
            "禁止 Host 行: {items:?}"
        );
        let existing: Vec<&ExistingEntry> = items
            .iter()
            .filter_map(|i| match i {
                PanelItem::Existing(e) => Some(e),
                _ => None,
            })
            .collect();
        assert_eq!(existing.len(), 2, "local + ssh-self 必须双份: {items:?}");
        assert!(existing
            .iter()
            .any(|e| { e.title == "mux-dup" && matches!(e.transport, TargetTransport::Local) }));
        assert!(existing.iter().any(|e| {
            e.title == "mux-dup"
                && matches!(&e.transport, TargetTransport::Ssh { name } if name == "self")
        }));
    }

    /// C9：widget_name 含 connect name，双份行才能共存。
    #[test]
    fn existing_row_widget_includes_connect_name() {
        let src = include_str!("quickconnect_panel.rs");
        let start = src
            .find("PanelItem::Existing(entry)")
            .expect("Existing 行渲染应存在");
        let chunk = &src[start..];
        assert!(
            chunk[..chunk.find("PanelItem::Host").unwrap_or(chunk.len())]
                .contains("muxterm-existing-row-{}-{}-{}"),
            "Existing 行 widget_name 必须是 runtime-connect-id: {chunk}"
        );
    }

    /// C9：Home 空 + 探测中 → Loading；探测完空 → Empty；有行 → Existing。
    #[test]
    fn existing_items_home_empty_or_loading() {
        let loading = existing_items(ExistingNav::Home, &[], &[], true, |_| vec![]);
        assert!(matches!(loading[1], PanelItem::Loading));

        let empty = existing_items(ExistingNav::Home, &[], &[], false, |_| vec![]);
        assert!(matches!(empty[1], PanelItem::Empty { .. }));
    }

    #[test]
    fn existing_attach_config_preserves_target_identity() {
        let tmux = existing_entry_to_config(&ExistingEntry {
            title: "matrix".into(),
            runtime: TargetRuntime::Tmux,
            transport: TargetTransport::Local,
            tmux_session: Some("matrix".into()),
            tmux_socket: Some("muxterm-test-existing".into()),
            herdr_session: None,
            herdr_workspace_id: None,
            herdr_socket: None,
        });
        assert_eq!(tmux.session.as_deref(), Some("matrix"));
        assert_eq!(tmux.socket.as_deref(), Some("muxterm-test-existing"));
        assert_eq!(tmux.path, "~");

        let herdr = existing_entry_to_config(&ExistingEntry {
            title: "worktree".into(),
            runtime: TargetRuntime::Herdr,
            transport: TargetTransport::Local,
            tmux_session: None,
            tmux_socket: None,
            herdr_session: Some("named".into()),
            herdr_workspace_id: Some("w223".into()),
            herdr_socket: Some("/tmp/herdr.sock".into()),
        });
        assert_eq!(herdr.workspace_id.as_deref(), Some("w223"));
        assert_eq!(herdr.session.as_deref(), Some("named"));
        assert_eq!(herdr.socket.as_deref(), Some("/tmp/herdr.sock"));
        assert_eq!(herdr.path, "");
    }

    /// W20：filter 对 Folder/Existing/Back 生效，Back 始终保留。
    #[test]
    fn filter_handles_existing_variants() {
        let items = vec![
            PanelItem::Folder {
                id: "existing-connections",
                title: "已有的连接".into(),
            },
            PanelItem::Back,
            PanelItem::Existing(ExistingEntry {
                title: "w1".into(),
                runtime: TargetRuntime::Herdr,
                transport: TargetTransport::Local,
                tmux_session: None,
                tmux_socket: None,
                herdr_session: Some("default".into()),
                herdr_workspace_id: Some("w1".into()),
                herdr_socket: None,
            }),
        ];
        let hit = filter_panel_items(&items, "已有");
        assert_eq!(hit.len(), 2, "Folder 按 title 过滤 + Back 始终保留");
        assert!(matches!(hit[0], PanelItem::Folder { .. }));
        let back = filter_panel_items(&items, "zzz");
        assert_eq!(back.len(), 1, "Back 始终保留");
        assert!(matches!(back[0], PanelItem::Back));
        let herdr = filter_panel_items(&items, "herdr @");
        assert_eq!(herdr.len(), 2, "Existing 按 subtitle 过滤 + Back 始终保留");
        assert!(matches!(herdr[1], PanelItem::Existing(_)));
    }

    /// C7：探测结束后空 host 表必须是 Empty，不能继续 Loading。
    #[test]
    fn ssh_hosts_empty_after_probe_must_not_stay_loading() {
        let src = include_str!("quickconnect_panel.rs");
        let start = src
            .find("pub struct ExistingPanelState")
            .expect("ExistingPanelState 应存在");
        let rest = &src[start..];
        let end = rest.find("\n///").unwrap_or(rest.len());
        let struct_src = &rest[..end.min(500)];
        assert!(
            struct_src.contains("probe_inflight"),
            "ExistingPanelState 必须有 probe_inflight（探测中 true / 完成后 false），空 host 才能从 Loading 变成 Empty。struct={struct_src}"
        );
    }

    #[test]
    fn filter_matches_subtitle_and_path() {
        let ssh = TargetConfig::new(
            "srv",
            TargetRuntime::Tmux,
            TargetTransport::Ssh {
                name: "ryzen".into(),
            },
            "~/work",
        );
        let items = vec![PanelItem::Target(
            QuickConnectEntry::new(ssh, vec![]),
            false,
        )];
        assert_eq!(filter_panel_items(&items, "ryzen").len(), 1);
        assert_eq!(filter_panel_items(&items, "work").len(), 1);
        assert_eq!(filter_panel_items(&items, "nomatch").len(), 0);
    }
}
