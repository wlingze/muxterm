//! 目标配置窗口：runtime / transport / SSH alias / path / name。
//!
//! 对齐 macOS `TargetConfigWindow`：单选卡片、目录异步补全（防抖 +
//! generation 防竞态）、name 自动派生（手动编辑后不覆盖）。

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, ComboBoxText, Entry, Label, ListBox, Orientation, ScrolledWindow,
    ToggleButton, Window,
};

use crate::core::transport::ssh::probe::{ssh_dot_css_class, ssh_dot_widget_name, SshReach};
use crate::platform::i18n::{self, Key};
use crate::platform::linux::ffi_bridge::{CoreBridge, SshHostEntry};
use crate::platform::linux::quickconnect::directory::{
    DirectoryListingResponse, DirectorySuggestionController,
};
use crate::platform::linux::quickconnect::model::{
    QuickConnect, TargetConfig, TargetRuntime, TargetTransport,
};
use crate::platform::linux::quickconnect::options::TargetOptionSelection;
use crate::platform::linux::quickconnect::store::QuickConnectStore;

/// 目录补全 debounce：用 generation 作废旧回调。
///
/// 不能对已触发的 `glib::SourceId` 再 `remove()`：glib 0.20 会 unwrap
/// `Failed to remove source`，且发生在 GTK `toggled` trampoline 里无法 unwind，
/// 表现为点 SSH 卡片直接 abort。
#[derive(Default)]
pub(crate) struct ListingDebounce {
    generation: Cell<u64>,
}

impl ListingDebounce {
    pub(crate) fn bump(&self) -> u64 {
        let next = self.generation.get().wrapping_add(1);
        self.generation.set(next);
        next
    }

    pub(crate) fn is_current(&self, token: u64) -> bool {
        self.generation.get() == token
    }
}

/// SSH 未选 alias 时不要发远程 list_dir。
pub(crate) fn should_skip_directory_listing(is_ssh: bool, alias: Option<&str>) -> bool {
    is_ssh && alias.map(str::trim).filter(|s| !s.is_empty()).is_none()
}

/// 打开新建/编辑 Project 窗口。
pub fn show(
    parent: &impl IsA<Window>,
    editing: Option<TargetConfig>,
    store: QuickConnectStore,
    ssh_hosts: Vec<SshHostEntry>,
    on_save: impl Fn(TargetConfig) + 'static,
    on_cancel: impl Fn() + 'static,
) -> Window {
    let parent = parent.as_ref();
    let win = Window::builder()
        .transient_for(parent)
        .modal(true)
        .title(if editing.is_none() {
            i18n::tr(Key::NewProject)
        } else {
            i18n::tr(Key::EditProject)
        })
        .default_width(520)
        .default_height(420)
        .build();

    let root = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(10)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();

    let state = Rc::new(RefCell::new(EditorState {
        selection: TargetOptionSelection::default(),
        path: DirectorySuggestionController::new("~"),
        name_manual: false,
        name: String::new(),
        ssh_alias: String::new(),
    }));

    if let Some(cfg) = &editing {
        let mut s = state.borrow_mut();
        s.selection = TargetOptionSelection::new(cfg.runtime, cfg.transport.clone());
        s.path =
            DirectorySuggestionController::new(if cfg.path.is_empty() { "~" } else { &cfg.path });
        s.name = cfg.name.clone();
        s.name_manual = true;
        if let TargetTransport::Ssh { name } = &cfg.transport {
            s.ssh_alias = name.clone();
            let _ = s.path.set_transport(true, Some(name.as_str()));
        }
    }

    root.append(&section_label(&i18n::tr(Key::Runtime)));
    let runtime_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(10)
        .build();
    let tmux_btn = option_card(
        "muxterm-runtime-tmux",
        "tmux",
        &i18n::tr(Key::AttachCreateTmux),
        state.borrow().selection.runtime == TargetRuntime::Tmux,
    );
    let shell_btn = option_card(
        "muxterm-runtime-shell",
        "shell",
        &i18n::tr(Key::PlainShell),
        state.borrow().selection.runtime == TargetRuntime::Shell,
    );
    let herdr_btn = option_card(
        "muxterm-runtime-herdr",
        "herdr",
        &i18n::tr(Key::AttachCreateHerdr),
        state.borrow().selection.runtime == TargetRuntime::Herdr,
    );
    runtime_row.append(&tmux_btn);
    runtime_row.append(&shell_btn);
    runtime_row.append(&herdr_btn);
    root.append(&runtime_row);

    root.append(&section_label(&i18n::tr(Key::Transport)));
    let transport_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(10)
        .build();
    let local_btn = option_card(
        "muxterm-transport-local",
        "local",
        &i18n::tr(Key::LocalTransport),
        matches!(state.borrow().selection.transport, TargetTransport::Local),
    );
    let ssh_btn = option_card(
        "muxterm-transport-ssh",
        "ssh",
        &i18n::tr(Key::SshTransport),
        state.borrow().selection.transport.is_ssh(),
    );
    transport_row.append(&local_btn);
    transport_row.append(&ssh_btn);
    root.append(&transport_row);

    let aliases: Vec<String> = ssh_hosts.iter().map(|h| h.alias.clone()).collect();
    let ssh_section = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(6)
        .build();
    ssh_section.append(&section_label(&i18n::tr(Key::SshName)));
    let ssh_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .build();
    let ssh_combo = ComboBoxText::with_entry();
    ssh_combo.set_hexpand(true);
    for alias in &aliases {
        ssh_combo.append(Some(alias), alias);
    }
    let current_alias = state.borrow().ssh_alias.clone();
    if !current_alias.is_empty() {
        let _ = ssh_combo.set_active_id(Some(&current_alias));
        if let Some(entry) = ssh_combo.child().and_downcast::<Entry>() {
            entry.set_text(&current_alias);
        }
    }
    // W15d：host picker 与 QC 列表共用同一套 ssh_dot_widget_name / ssh_dot_css_class。
    // 后台探测（BatchMode + ConnectTimeout=2），结果经 channel 回主线程更新灯。
    let ssh_dot = Label::new(Some("●"));
    ssh_dot.set_halign(Align::Start);
    ssh_dot.set_valign(Align::Center);
    ssh_dot.add_css_class(ssh_dot_css_class(SshReach::Unknown));
    let (probe_tx, probe_rx) = mpsc::channel::<(String, SshReach)>();
    for alias in &aliases {
        let tx = probe_tx.clone();
        let alias = alias.clone();
        std::thread::spawn(move || {
            let args = crate::core::transport::ssh::probe::ssh_probe_args(&alias, 2);
            let status = std::process::Command::new("ssh")
                .args(&args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            let reach = match status {
                Ok(st) => crate::core::transport::ssh::probe::classify_ssh_probe(st.code()),
                Err(_) => SshReach::Err,
            };
            let _ = tx.send((alias, reach));
        });
    }
    let update_dot = {
        let ssh_dot = ssh_dot.clone();
        let ssh_combo = ssh_combo.clone();
        move || {
            while let Ok((alias, reach)) = probe_rx.try_recv() {
                let selected = ssh_combo.active_id().map(|s| s.to_string());
                if selected.as_deref() == Some(alias.as_str()) {
                    ssh_dot.set_widget_name(&ssh_dot_widget_name(&alias));
                    ssh_dot.remove_css_class(ssh_dot_css_class(SshReach::Unknown));
                    ssh_dot.remove_css_class(ssh_dot_css_class(SshReach::Ok));
                    ssh_dot.remove_css_class(ssh_dot_css_class(SshReach::Err));
                    ssh_dot.add_css_class(ssh_dot_css_class(reach));
                }
            }
        }
    };
    let update_dot = Rc::new(update_dot);
    update_dot();
    let poll_dot = Rc::new(Cell::new(Some(glib::timeout_add_local(
        Duration::from_millis(200),
        {
            let update_dot = update_dot.clone();
            move || {
                update_dot();
                glib::ControlFlow::Continue
            }
        },
    ))));
    ssh_combo.connect_changed({
        let update_dot = update_dot.clone();
        move |_| update_dot()
    });
    ssh_row.append(&ssh_combo);
    ssh_row.append(&ssh_dot);
    ssh_section.append(&ssh_row);
    root.append(&ssh_section);
    win.connect_close_request({
        let poll_dot = poll_dot.clone();
        move |_| {
            if let Some(id) = poll_dot.take() {
                id.remove();
            }
            glib::Propagation::Proceed
        }
    });

    root.append(&section_label(&i18n::tr(Key::Path)));
    let path_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .build();
    let path_entry = Entry::builder()
        .placeholder_text(i18n::tr(Key::Path))
        .hexpand(true)
        .text(state.borrow().path.text.as_str())
        .build();
    let up_btn = Button::with_label(&i18n::tr(Key::GoUpDirectory));
    path_row.append(&path_entry);
    path_row.append(&up_btn);
    root.append(&path_row);

    let suggest = ListBox::new();
    suggest.set_selection_mode(gtk4::SelectionMode::Single);
    let suggest_sw = ScrolledWindow::builder()
        .min_content_height(80)
        .max_content_height(120)
        .child(&suggest)
        .build();
    root.append(&suggest_sw);

    root.append(&section_label(&i18n::tr(Key::Name)));
    let name_entry = Entry::builder()
        .placeholder_text(i18n::tr(Key::Name))
        .text(state.borrow().name.as_str())
        .build();
    let name_hint = Label::new(Some(&i18n::tr_args(
        Key::DefaultNameHint,
        &[(
            "name",
            &QuickConnect::default_name(&state.borrow().path.text),
        )],
    )));
    name_hint.set_halign(Align::Start);
    name_hint.add_css_class("dim-label");
    root.append(&name_entry);
    root.append(&name_hint);

    let buttons = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(10)
        .halign(Align::End)
        .build();
    let cancel = Button::with_label(&i18n::tr(Key::Cancel));
    let save = Button::with_label(&i18n::tr(Key::Save));
    save.set_widget_name("muxterm-target-config-save");
    save.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&save);
    root.append(&buttons);

    win.set_child(Some(&root));

    let debounce = Rc::new(ListingDebounce::default());
    let alive = Rc::new(Cell::new(true));

    let refresh_ssh_visible = {
        let ssh_section = ssh_section.clone();
        let state = state.clone();
        move || {
            ssh_section.set_visible(state.borrow().selection.transport.is_ssh());
        }
    };
    refresh_ssh_visible();

    let schedule_listing = {
        let state = state.clone();
        let suggest = suggest.clone();
        let debounce = debounce.clone();
        let alive = alive.clone();
        move || {
            let token = debounce.bump();
            let state = state.clone();
            let suggest = suggest.clone();
            let debounce = debounce.clone();
            let alive = alive.clone();
            glib::timeout_add_local(Duration::from_millis(120), move || {
                if !alive.get() || !debounce.is_current(token) {
                    return glib::ControlFlow::Break;
                }
                start_listing(&state, &suggest, &alive);
                glib::ControlFlow::Break
            });
        }
    };

    {
        let state = state.clone();
        let tmux_btn = tmux_btn.clone();
        let shell_btn = shell_btn.clone();
        let herdr_btn = herdr_btn.clone();
        tmux_btn.connect_toggled({
            let state = state.clone();
            let shell_btn = shell_btn.clone();
            let herdr_btn = herdr_btn.clone();
            move |btn| {
                if btn.is_active() {
                    state
                        .borrow_mut()
                        .selection
                        .select_runtime(TargetRuntime::Tmux);
                    shell_btn.set_active(false);
                    herdr_btn.set_active(false);
                }
            }
        });
        shell_btn.connect_toggled({
            let state = state.clone();
            let tmux_btn = tmux_btn.clone();
            let herdr_btn = herdr_btn.clone();
            move |btn| {
                if btn.is_active() {
                    state
                        .borrow_mut()
                        .selection
                        .select_runtime(TargetRuntime::Shell);
                    tmux_btn.set_active(false);
                    herdr_btn.set_active(false);
                }
            }
        });
        herdr_btn.connect_toggled({
            let state = state.clone();
            let tmux_btn = tmux_btn.clone();
            let shell_btn = shell_btn.clone();
            move |btn| {
                if btn.is_active() {
                    state
                        .borrow_mut()
                        .selection
                        .select_runtime(TargetRuntime::Herdr);
                    tmux_btn.set_active(false);
                    shell_btn.set_active(false);
                }
            }
        });
    }

    {
        let state = state.clone();
        let local_btn = local_btn.clone();
        let ssh_btn = ssh_btn.clone();
        let refresh_ssh_visible = refresh_ssh_visible.clone();
        let schedule_listing = schedule_listing.clone();
        local_btn.connect_toggled({
            let state = state.clone();
            let ssh_btn = ssh_btn.clone();
            let refresh_ssh_visible = refresh_ssh_visible.clone();
            let schedule_listing = schedule_listing.clone();
            move |btn| {
                if btn.is_active() {
                    state
                        .borrow_mut()
                        .selection
                        .select_transport(TargetTransport::Local);
                    ssh_btn.set_active(false);
                    let _ = state.borrow_mut().path.set_transport(false, None);
                    refresh_ssh_visible();
                    schedule_listing();
                }
            }
        });
        ssh_btn.connect_toggled({
            let state = state.clone();
            let refresh_ssh_visible = refresh_ssh_visible.clone();
            let schedule_listing = schedule_listing.clone();
            move |btn| {
                if btn.is_active() {
                    let alias = state.borrow().ssh_alias.clone();
                    state
                        .borrow_mut()
                        .selection
                        .select_transport(TargetTransport::Ssh {
                            name: alias.clone(),
                        });
                    local_btn.set_active(false);
                    let _ = state
                        .borrow_mut()
                        .path
                        .set_transport(true, if alias.is_empty() { None } else { Some(&alias) });
                    refresh_ssh_visible();
                    schedule_listing();
                }
            }
        });
    }

    {
        let state = state.clone();
        let schedule_listing = schedule_listing.clone();
        ssh_combo.connect_changed(move |combo| {
            let alias = combo_alias(combo);
            state.borrow_mut().ssh_alias = alias.clone();
            if state.borrow().selection.transport.is_ssh() {
                state
                    .borrow_mut()
                    .selection
                    .select_transport(TargetTransport::Ssh {
                        name: alias.clone(),
                    });
                let _ = state.borrow_mut().path.set_transport(
                    true,
                    if alias.is_empty() {
                        None
                    } else {
                        Some(alias.as_str())
                    },
                );
                schedule_listing();
            }
        });
    }

    {
        let state = state.clone();
        let path_entry = path_entry.clone();
        let name_entry = name_entry.clone();
        let name_hint = name_hint.clone();
        let schedule_listing = schedule_listing.clone();
        path_entry.connect_changed(move |e| {
            let text = e.text().to_string();
            let req = state.borrow_mut().path.update_input(&text);
            let _ = req;
            auto_name(&state, &name_entry, &name_hint);
            schedule_listing();
        });
    }

    {
        let state = state.clone();
        let path_entry = path_entry.clone();
        let name_entry = name_entry.clone();
        let name_hint = name_hint.clone();
        let schedule_listing = schedule_listing.clone();
        up_btn.connect_clicked(move |_| {
            let req = state.borrow_mut().path.go_up();
            path_entry.set_text(&state.borrow().path.text);
            let _ = req;
            auto_name(&state, &name_entry, &name_hint);
            schedule_listing();
        });
    }

    {
        let state = state.clone();
        let path_entry = path_entry.clone();
        let name_entry = name_entry.clone();
        let name_hint = name_hint.clone();
        let schedule_listing = schedule_listing.clone();
        suggest.connect_row_activated(move |_, row| {
            if let Some(label) = row.child().and_downcast::<Label>() {
                let cand = label.text().to_string();
                let _ = state.borrow_mut().path.select(&cand);
                path_entry.set_text(&state.borrow().path.text);
                auto_name(&state, &name_entry, &name_hint);
                schedule_listing();
            }
        });
    }

    name_entry.connect_changed({
        let state = state.clone();
        move |e| {
            let text = e.text().to_string();
            let mut s = state.borrow_mut();
            if text != QuickConnect::default_name(&s.path.text) {
                s.name_manual = true;
            }
            s.name = text;
        }
    });

    let finished = Rc::new(Cell::new(false));
    {
        let win = win.clone();
        let on_cancel = Rc::new(on_cancel);
        cancel.connect_clicked({
            let win = win.clone();
            let on_cancel = on_cancel.clone();
            let finished = finished.clone();
            move |_| {
                if finished.replace(true) {
                    return;
                }
                win.close();
                on_cancel();
            }
        });
        win.connect_close_request({
            let on_cancel = on_cancel.clone();
            let finished = finished.clone();
            let debounce = debounce.clone();
            let alive = alive.clone();
            move |_| {
                alive.set(false);
                debounce.bump();
                if !finished.replace(true) {
                    on_cancel();
                }
                glib::Propagation::Proceed
            }
        });
    }

    {
        let win = win.clone();
        let state = state.clone();
        let finished = finished.clone();
        save.connect_clicked(move |_| {
            if finished.replace(true) {
                return;
            }
            let s = state.borrow();
            let name = if s.name.trim().is_empty() {
                QuickConnect::default_name(&s.path.text)
            } else {
                s.name.clone()
            };
            let transport = if s.selection.transport.is_ssh() {
                TargetTransport::Ssh {
                    name: s.ssh_alias.clone(),
                }
            } else {
                TargetTransport::Local
            };
            let cfg = TargetConfig::new(name, s.selection.runtime, transport, s.path.text.clone());
            drop(s);
            let mut store = store.clone();
            store.upsert_project(&cfg);
            win.close();
            on_save(cfg);
        });
    }

    schedule_listing();
    win.present();
    win
}

struct EditorState {
    selection: TargetOptionSelection,
    path: DirectorySuggestionController,
    name_manual: bool,
    name: String,
    ssh_alias: String,
}

fn section_label(text: &str) -> Label {
    let l = Label::new(Some(text));
    l.set_halign(Align::Start);
    l.add_css_class("heading");
    l
}

fn option_card(widget_name: &str, title: &str, subtitle: &str, active: bool) -> ToggleButton {
    let btn = ToggleButton::new();
    btn.set_widget_name(widget_name);
    btn.set_active(active);
    let col = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(2)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();
    let t = Label::new(Some(title));
    t.add_css_class("title-4");
    let s = Label::new(Some(subtitle));
    s.add_css_class("dim-label");
    col.append(&t);
    col.append(&s);
    btn.set_child(Some(&col));
    btn
}

fn combo_alias(combo: &ComboBoxText) -> String {
    if let Some(entry) = combo.child().and_downcast::<Entry>() {
        return entry.text().to_string();
    }
    combo
        .active_text()
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn auto_name(state: &Rc<RefCell<EditorState>>, name_entry: &Entry, hint: &Label) {
    let default = QuickConnect::default_name(&state.borrow().path.text);
    hint.set_text(&i18n::tr_args(Key::DefaultNameHint, &[("name", &default)]));
    if !state.borrow().name_manual {
        name_entry.set_text(&default);
        state.borrow_mut().name = default;
    }
}

fn start_listing(state: &Rc<RefCell<EditorState>>, suggest: &ListBox, alive: &Rc<Cell<bool>>) {
    let request = state.borrow().path.request();
    if should_skip_directory_listing(request.is_ssh, request.alias.as_deref()) {
        while let Some(c) = suggest.first_child() {
            suggest.remove(&c);
        }
        return;
    }
    let transport = if request.is_ssh { "ssh" } else { "local" };
    let target = request.alias.clone();
    let path = request.path.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = CoreBridge::list_dir(transport, target.as_deref(), &path);
        let _ = tx.send(result);
    });
    let state = state.clone();
    let suggest = suggest.clone();
    let alive = alive.clone();
    glib::timeout_add_local(Duration::from_millis(40), move || {
        if !alive.get() {
            return glib::ControlFlow::Break;
        }
        match rx.try_recv() {
            Ok(Ok(entries)) => {
                let dirs: Vec<String> = entries
                    .into_iter()
                    .filter(|e| e.is_dir)
                    .map(|e| e.name)
                    .collect();
                let response = DirectoryListingResponse {
                    request: request.clone(),
                    directories: dirs,
                };
                if state.borrow_mut().path.apply(&response) {
                    rebuild_suggestions(&suggest, &state.borrow().path.candidates);
                }
                glib::ControlFlow::Break
            }
            Ok(Err(_)) => {
                let response = DirectoryListingResponse {
                    request: request.clone(),
                    directories: Vec::new(),
                };
                let _ = state.borrow_mut().path.apply(&response);
                rebuild_suggestions(&suggest, &[]);
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

fn rebuild_suggestions(suggest: &ListBox, cands: &[String]) {
    while let Some(c) = suggest.first_child() {
        suggest.remove(&c);
    }
    for c in cands {
        let row = gtk4::ListBoxRow::new();
        let label = Label::new(Some(c));
        label.set_halign(Align::Start);
        row.set_child(Some(&label));
        suggest.append(&row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_debounce_invalidates_previous_token() {
        let d = ListingDebounce::default();
        let first = d.bump();
        assert!(d.is_current(first));
        let second = d.bump();
        assert!(!d.is_current(first), "旧 debounce 回调必须作废");
        assert!(d.is_current(second));
        d.bump();
        assert!(!d.is_current(second));
    }

    #[test]
    fn listing_debounce_close_bumps_away_pending() {
        let d = ListingDebounce::default();
        let pending = d.bump();
        d.bump(); // 窗口关闭
        assert!(!d.is_current(pending));
    }

    #[test]
    fn skip_remote_listing_until_ssh_alias_chosen() {
        assert!(should_skip_directory_listing(true, None));
        assert!(should_skip_directory_listing(true, Some("")));
        assert!(should_skip_directory_listing(true, Some("  ")));
        assert!(!should_skip_directory_listing(true, Some("ryzen")));
        assert!(!should_skip_directory_listing(false, None));
    }

    #[test]
    fn selecting_ssh_keeps_empty_alias_until_combo_changes() {
        let mut sel = TargetOptionSelection::default();
        sel.select_transport(TargetTransport::Ssh {
            name: String::new(),
        });
        assert!(sel.transport.is_ssh());
        assert_eq!(sel.transport.create_backend(), ("ssh", Some("")));
    }
}
