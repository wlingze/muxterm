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

/// 打开新建/编辑 Project 窗口。
pub fn show(
    parent: &impl IsA<Window>,
    editing: Option<TargetConfig>,
    store: QuickConnectStore,
    ssh_hosts: Vec<SshHostEntry>,
    on_save: impl Fn(TargetConfig) + 'static,
    on_cancel: impl Fn() + 'static,
) {
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
        "tmux",
        &i18n::tr(Key::AttachCreateTmux),
        state.borrow().selection.runtime == TargetRuntime::Tmux,
    );
    let shell_btn = option_card(
        "shell",
        &i18n::tr(Key::PlainShell),
        state.borrow().selection.runtime == TargetRuntime::Shell,
    );
    runtime_row.append(&tmux_btn);
    runtime_row.append(&shell_btn);
    root.append(&runtime_row);

    root.append(&section_label(&i18n::tr(Key::Transport)));
    let transport_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(10)
        .build();
    let local_btn = option_card(
        "local",
        &i18n::tr(Key::LocalTransport),
        matches!(state.borrow().selection.transport, TargetTransport::Local),
    );
    let ssh_btn = option_card(
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
    ssh_section.append(&ssh_combo);
    root.append(&ssh_section);

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
    save.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&save);
    root.append(&buttons);

    win.set_child(Some(&root));

    let debounce = Rc::new(Cell::new(None::<glib::SourceId>));

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
        move || {
            if let Some(id) = debounce.take() {
                id.remove();
            }
            let state = state.clone();
            let suggest = suggest.clone();
            let id = glib::timeout_add_local(Duration::from_millis(120), move || {
                start_listing(&state, &suggest);
                glib::ControlFlow::Break
            });
            debounce.set(Some(id));
        }
    };

    {
        let state = state.clone();
        let tmux_btn = tmux_btn.clone();
        let shell_btn = shell_btn.clone();
        tmux_btn.connect_toggled({
            let state = state.clone();
            let shell_btn = shell_btn.clone();
            move |btn| {
                if btn.is_active() {
                    state
                        .borrow_mut()
                        .selection
                        .select_runtime(TargetRuntime::Tmux);
                    shell_btn.set_active(false);
                }
            }
        });
        shell_btn.connect_toggled(move |btn| {
            if btn.is_active() {
                state
                    .borrow_mut()
                    .selection
                    .select_runtime(TargetRuntime::Shell);
                tmux_btn.set_active(false);
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
            move |_| {
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

fn option_card(title: &str, subtitle: &str, active: bool) -> ToggleButton {
    let btn = ToggleButton::new();
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

fn start_listing(state: &Rc<RefCell<EditorState>>, suggest: &ListBox) {
    let request = state.borrow().path.request();
    if request.is_ssh && request.alias.is_none() {
        while let Some(c) = suggest.first_child() {
            suggest.remove(&c);
        }
        return;
    }
    let backend = if request.is_ssh { "ssh" } else { "local" };
    let target = request.alias.clone();
    let path = request.path.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = CoreBridge::list_dir(backend, target.as_deref(), &path);
        let _ = tx.send(result);
    });
    let state = state.clone();
    let suggest = suggest.clone();
    glib::timeout_add_local(Duration::from_millis(40), move || match rx.try_recv() {
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
