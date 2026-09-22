//! Keyboard, command-palette, clipboard, and pane action wiring.
//!
//! The parent window owns state and the event loop; this module keeps
//! user actions as a separate controller boundary.

use crate::frontend::linux::lifecycle::cycle_pane_id;

use super::window_config::open_preferences;
use super::window_connection::{open_ssh_connect, open_tmux_attach};
use super::window_event_pump::enqueue_workspace_input;
use super::window_layout::refresh_ui;
use super::window_overlay::{open_panel, open_quick_connect};
use super::window_scene::{request_switch_tab, switch_tab_n, switch_workspace_n};
use super::window_status::maybe_refresh_status;
use super::*;

pub(super) fn prepare_core_tab_mutation(s: &mut UiState, task: &ClientTask) {
    let clear = match task {
        ClientTask::NewTab => true,
        ClientTask::CloseTab { tab_id } => *tab_id == s.active_tab,
        _ => false,
    };
    if clear {
        let workspace_key = s.active_workspace_key();
        s.visible_tabs.remove(&workspace_key);
        s.local_tab_overrides.remove(&workspace_key);
    }
}

pub(super) fn handle_action(
    s: &mut UiState,
    action: Action,
    window: &Window,
    state: &Rc<RefCell<UiState>>,
) {
    sync_focused_pane(s);
    match action {
        Action::NewTab | Action::NewWindow => {
            super::window_aggregate::new_tab(s);
            // Accepted 不得手工 refresh：等 16ms 批里 LayoutChanged/MutationSettled。
            return;
        }
        Action::NewPane => {
            let pane = s.active_pane;
            let _ = s.execute_active_task(ClientTask::SplitPane {
                pane_id: pane,
                horizontal: true,
            });
            return;
        }
        Action::NewPaneVertical => {
            let pane = s.active_pane;
            let _ = s.execute_active_task(ClientTask::SplitPane {
                pane_id: pane,
                horizontal: false,
            });
            return;
        }
        Action::SwitchTab1 => switch_tab_n(s, 1),
        Action::SwitchTab2 => switch_tab_n(s, 2),
        Action::SwitchTab3 => switch_tab_n(s, 3),
        Action::SwitchTab4 => switch_tab_n(s, 4),
        Action::SwitchTab5 => switch_tab_n(s, 5),
        Action::SwitchTab6 => switch_tab_n(s, 6),
        Action::SwitchTab7 => switch_tab_n(s, 7),
        Action::SwitchTab8 => switch_tab_n(s, 8),
        Action::SwitchTab9 => switch_tab_n(s, 9),
        Action::SwitchTabLast => {
            let workspace_id = active_workspace_key(s);
            if let Some(tab_id) = s
                .view_store
                .workspace(&workspace_id)
                .and_then(|view| view.tabs.last().map(|tab| tab.id))
            {
                request_switch_tab(s, tab_id);
            }
        }
        Action::SwitchWorkspace1 => switch_workspace_n(s, 1),
        Action::SwitchWorkspace2 => switch_workspace_n(s, 2),
        Action::SwitchWorkspace3 => switch_workspace_n(s, 3),
        Action::SwitchWorkspace4 => switch_workspace_n(s, 4),
        Action::SwitchWorkspace5 => switch_workspace_n(s, 5),
        Action::SwitchWorkspace6 => switch_workspace_n(s, 6),
        Action::SwitchWorkspace7 => switch_workspace_n(s, 7),
        Action::SwitchWorkspace8 => switch_workspace_n(s, 8),
        Action::SwitchWorkspace9 => switch_workspace_n(s, 9),
        Action::SwitchWorkspaceLast => switch_workspace_n(s, 0),
        Action::SwitchPaneNext | Action::SwitchPanePrev => {
            switch_pane_offset(s, matches!(action, Action::SwitchPaneNext));
        }
        Action::CommandPalette => {
            open_command_palette(s, window, state);
            return;
        }
        Action::Search | Action::Unknown => {}
        Action::QuickConnect => {
            // 调用方（快捷键/命令面板）必须先释放 RefMut 再打开面板；
            // 这里只做标记，由 handle_action 的调用方处理。
            let _ = (s, window, state);
            return;
        }
        Action::Quit => {
            window.close();
            return;
        }
        Action::Copy => {
            copy_active_pane(s);
            return;
        }
        Action::Paste => {
            paste_active_pane(s, state);
            return;
        }
        Action::IncreaseFontSize => adjust_font(s, state, 1),
        Action::DecreaseFontSize => adjust_font(s, state, -1),
        Action::ResetFontSize => reset_font(s),
        Action::TogglePaneFullscreen => toggle_fullscreen(s),
    }
    refresh_ui(s);
}

pub(super) fn sync_focused_pane(s: &mut UiState) {
    let key = s.active_workspace_key();
    let focused = s
        .view_store
        .workspace(&key)
        .and_then(|view| view.panes.get(&s.active_tab_id()))
        .and_then(|panes| {
            panes.iter().find(|pane| {
                s.active_layout()
                    .pane(pane.id)
                    .is_some_and(|view| view.terminal().has_focus())
            })
        })
        .map(|pane| pane.id);
    if let Some(pane_id) = focused.filter(|pane| *pane != s.active_pane) {
        s.active_pane = pane_id;
        let _ = s.execute_active_task(ClientTask::SwitchPane { pane_id });
    }
}

/// 与 macOS `movePane` 对齐：用当前 tab 快照算目标，发 SwitchPane。
/// 不要发 NextPane——tmux 布局树若没解析完会落到无效的
/// `select-pane -t @N -N/-P`（2219.log 14:41:29）。
fn switch_pane_offset(s: &mut UiState, forward: bool) {
    let workspace_id = s.active_workspace_key();
    let panes = s
        .view_store
        .workspace(&workspace_id)
        .and_then(|view| view.panes.get(&s.active_tab))
        .cloned()
        .unwrap_or_default();
    let ids: Vec<u32> = panes.iter().map(|pane| pane.id).collect();
    let active = if ids.contains(&s.active_pane) {
        s.active_pane
    } else {
        panes
            .iter()
            .find(|pane| pane.is_active)
            .map(|pane| pane.id)
            .unwrap_or(s.active_pane)
    };
    if let Some(target) = cycle_pane_id(&ids, active, forward) {
        s.active_pane = target;
        if s.active_layout().fullscreen_pane().is_some() {
            s.active_layout_mut().set_fullscreen_pane(Some(target));
            refresh_ui(s);
            s.active_pane = target;
        }
        if let Some(view) = s.active_layout().pane(target) {
            view.grab_focus();
        }
        let _ = s.execute_active_task(ClientTask::SwitchPane { pane_id: target });
    }
}

pub(super) fn open_command_palette(s: &UiState, window: &Window, state: &Rc<RefCell<UiState>>) {
    let parent = window.clone();
    let callback_parent = parent.clone();
    let callback_window = parent.clone();
    let callback_state = state.clone();
    let uses_tmux = s.uses_tmux();
    let next_theme = if s.theme_name.eq_ignore_ascii_case("dark") {
        "Light"
    } else {
        "Dark"
    };
    let next_status_mode = match s.status_mode {
        StatusBarMode::Tmux => StatusBarMode::Theme.as_str(),
        StatusBarMode::Theme => StatusBarMode::Tmux.as_str(),
    };
    crate::frontend::linux::command_palette::show_for_runtime(
        &parent,
        uses_tmux,
        next_theme,
        next_status_mode,
        move |id| {
            run_palette_command(&callback_state, &callback_window, &callback_parent, id);
        },
    );
}

pub(super) fn run_palette_command(
    state: &Rc<RefCell<UiState>>,
    window: &Window,
    parent: &Window,
    id: &str,
) {
    let Some(action) = parse_palette_action(id) else {
        tracing::warn!(target = "muxterm::linux", "未知命令面板动作: {id}");
        return;
    };
    match action {
        PaletteAction::Language => {
            let language_parent = parent.clone();
            let callback_state = state.clone();
            crate::frontend::linux::command_palette::show_language(&language_parent, move |_| {
                let mut s = callback_state.borrow_mut();
                maybe_refresh_status(&mut s, true);
            });
        }
        PaletteAction::TmuxDetach => {
            let mut s = state.borrow_mut();
            let id = s.active_ws_id();
            super::window_sidebar::close_sidebar_workspace(&mut s, &id);
        }
        PaletteAction::SshDisconnect => {
            let s = state.borrow();
            if s.uses_tmux() {
                let _ = s.execute_active_task(ClientTask::Detach);
            }
        }
        PaletteAction::QuickConnect => {
            open_quick_connect(state, window);
        }
        PaletteAction::ToggleTheme => {
            let mut s = state.borrow_mut();
            toggle_theme(&mut s);
        }
        PaletteAction::ToggleStatusBarMode => {
            let mut s = state.borrow_mut();
            toggle_status_mode(&mut s);
        }
        PaletteAction::TogglePaneFullscreen => {
            let mut s = state.borrow_mut();
            toggle_fullscreen(&mut s);
            refresh_ui(&mut s);
        }
        PaletteAction::IncreaseFontSize => {
            let mut s = state.borrow_mut();
            adjust_font(&mut s, state, 1);
        }
        PaletteAction::DecreaseFontSize => {
            let mut s = state.borrow_mut();
            adjust_font(&mut s, state, -1);
        }
        PaletteAction::ResetFontSize => {
            let mut s = state.borrow_mut();
            reset_font(&mut s);
        }
        PaletteAction::Quit => {
            request_quit_close(state, window);
        }
        PaletteAction::NewTab => {
            let mut s = state.borrow_mut();
            super::window_aggregate::new_tab(&mut s);
            // Accepted 不得手工 refresh：等 LayoutChanged/MutationSettled。
        }
        PaletteAction::NewPane => {
            let s = state.borrow();
            let pane = s.active_pane;
            let _ = s.execute_active_task(ClientTask::SplitPane {
                pane_id: pane,
                horizontal: true,
            });
        }
        PaletteAction::NewPaneVertical => {
            let s = state.borrow();
            let pane = s.active_pane;
            let _ = s.execute_active_task(ClientTask::SplitPane {
                pane_id: pane,
                horizontal: false,
            });
        }
        PaletteAction::ClosePane => {
            let mut s = state.borrow_mut();
            let pane = s.active_pane;
            let _ = s.execute_active_task(ClientTask::ClosePane { pane_id: pane });
            refresh_ui(&mut s);
        }
        PaletteAction::CloseTab => {
            let mut s = state.borrow_mut();
            if s.aggregate.kind.is_some() {
                let index = super::window_aggregate::tabs(&s).iter().position(|tab| {
                    tab.source.workspace == s.active_workspace_key()
                        && tab.source.tab == s.active_tab_id()
                });
                if let Some(index) = index {
                    super::window_aggregate::close(&mut s, index as u32 + 1);
                }
                return;
            }
            let tab = s.active_tab;
            let task = ClientTask::CloseTab { tab_id: tab };
            prepare_core_tab_mutation(&mut s, &task);
            let _ = s.execute_active_task(task);
            refresh_ui(&mut s);
        }
        PaletteAction::CloseWindow => window.close(),
        PaletteAction::SwitchPaneNext => {
            let mut s = state.borrow_mut();
            switch_pane_offset(&mut s, true);
            refresh_ui(&mut s);
        }
        PaletteAction::SwitchPanePrev => {
            let mut s = state.borrow_mut();
            switch_pane_offset(&mut s, false);
            refresh_ui(&mut s);
        }
        PaletteAction::SwitchTab(n) => {
            let mut s = state.borrow_mut();
            switch_tab_n(&mut s, n);
        }
        PaletteAction::TmuxAttach => open_tmux_attach(state, parent, false),
        PaletteAction::TmuxNew => open_tmux_attach(state, parent, true),
        PaletteAction::SshConnect => open_ssh_connect(state, parent),
        PaletteAction::SearchPanes => {
            open_panel(state, parent, PanelTab::Search);
        }
        PaletteAction::RenamePane => {
            tracing::info!(target = "muxterm::linux", "命令 {id} 尚未接到 GTK 对话框");
        }
        PaletteAction::Preferences => {
            open_preferences(state, parent);
        }
        PaletteAction::CheckUpdates => {
            super::window_update::check_for_updates(&mut state.borrow_mut());
        }
        PaletteAction::ReloadConfig | PaletteAction::OpenConfig => {
            tracing::info!(target = "muxterm::linux", "命令 {id} 尚未接到 GTK 对话框");
        }
    }
}

pub(super) fn toggle_fullscreen(s: &mut UiState) {
    let pane = s.active_pane;
    if s.uses_tmux() {
        let _ = s.execute_active_task(ClientTask::TogglePaneFullscreen { pane_id: pane });
    } else {
        let next = match s.active_layout().fullscreen_pane() {
            Some(id) if id == pane => None,
            _ => Some(pane),
        };
        s.active_layout_mut().set_fullscreen_pane(next);
    }
}

/// 通过统一 FFI client 的 Core 配置事务写回 config.toml（唯一事实源）。
/// 平台禁止直接解析或写 TOML；失败只记日志，不覆盖用户文件。
pub(super) fn persist_config(client: &FfiClient, dotted: &str, value: serde_json::Value) {
    if let Err(error) = client.config_apply_path(dotted, value) {
        tracing::warn!(target = "muxterm::config", "保存设置失败: {error}");
    }
}

pub(super) fn adjust_font(s: &mut UiState, state: &Rc<RefCell<UiState>>, direction: i32) {
    window_appearance::adjust_font(s, state, direction);
}

pub(super) fn reset_font(s: &mut UiState) {
    window_appearance::reset_font(s);
}

pub(super) fn toggle_theme(s: &mut UiState) {
    window_appearance::toggle_theme(s);
}

pub(super) fn apply_config_snapshot(s: &mut UiState, snapshot: ClientConfigSnapshot) {
    window_appearance::apply_config_snapshot(s, snapshot);
}

pub(super) fn toggle_status_mode(s: &mut UiState) {
    window_appearance::toggle_status_mode(s);
}

pub(super) fn report_all_pane_colours(s: &mut UiState) {
    window_appearance::report_all_pane_colours(s);
}

pub(super) fn copy_pane(s: &UiState, pane_id: u32) {
    if let Some(view) = s.active_layout().pane(pane_id) {
        view.copy_clipboard();
    }
}

pub(super) fn paste_pane(s: &UiState, state: &Rc<RefCell<UiState>>, pane_id: u32) {
    let Some(view) = s.active_layout().pane(pane_id).cloned() else {
        return;
    };
    let pane_id = view.pane_id();
    let bracketed = view.bracketed_paste();
    let owner = s.active_ws_id();
    let workspace_id = owner.as_str();
    let st = Rc::downgrade(state);
    let clipboard = view.widget().clipboard();
    if clipboard
        .formats()
        .contains_type(gtk4::gdk::Texture::static_type())
    {
        clipboard.read_texture_async(gtk4::gio::Cancellable::NONE, move |result| {
            let Some(st) = st.upgrade() else {
                return;
            };
            let mut s = st.borrow_mut();
            if !resident_pane_view(&s, &owner, pane_id)
                .is_some_and(|current| Rc::ptr_eq(&current, &view))
            {
                image_paste_error(&mut s, "Original pane was closed; image was not pasted.");
                return;
            }
            let result = (|| -> anyhow::Result<Vec<u8>> {
                let texture = result?.ok_or_else(|| anyhow::anyhow!("Clipboard has no image"))?;
                let pixels = i64::from(texture.width()) * i64::from(texture.height());
                if texture.width() > 16384 || texture.height() > 16384 || pixels > 64 * 1024 * 1024
                {
                    anyhow::bail!("Clipboard image is too large");
                }
                let pixbuf = gtk4::gdk::pixbuf_get_from_texture(&texture)
                    .ok_or_else(|| anyhow::anyhow!("Cannot decode clipboard image"))?;
                Ok(pixbuf.save_to_bufferv("png", &[])?)
            })();
            match result {
                Ok(png) if !s.image_paste_pending && s.image_paste_request.is_none() => {
                    s.image_paste_request = Some((workspace_id, pane_id, png, view.widget()));
                }
                Ok(_) => image_paste_error(&mut s, "An image paste is already in progress."),
                Err(error) => image_paste_error(&mut s, &error.to_string()),
            }
        });
        return;
    }
    clipboard.read_text_async(gtk4::gio::Cancellable::NONE, move |result| {
        let Ok(Some(text)) = result else {
            return;
        };
        let text = crate::frontend::mirror::sanitize_paste(text.as_str(), bracketed);
        let data = crate::frontend::mirror::encode_clipboard_paste(&text, bracketed);
        if data.is_empty() {
            return;
        }
        let Some(st) = st.upgrade() else {
            return;
        };
        let s = st.borrow();
        if !resident_pane_view(&s, &owner, pane_id)
            .is_some_and(|current| Rc::ptr_eq(&current, &view))
        {
            return;
        }
        enqueue_workspace_input(&s, &workspace_id, pane_id, &data, false);
    });
}

fn image_paste_error(s: &mut UiState, message: &str) {
    s.notification_log.push(format!("Image paste: {message}"));
    let dialog = gtk4::MessageDialog::builder()
        .text(crate::frontend::utils::i18n::tr(
            crate::frontend::utils::i18n::TextKey::ImagePasteFailed,
        ))
        .secondary_text(message)
        .buttons(gtk4::ButtonsType::Close)
        .build();
    if let Some(parent) = s
        .scenes
        .widget()
        .root()
        .and_then(|root| root.downcast::<gtk4::Window>().ok())
    {
        dialog.set_transient_for(Some(&parent));
    }
    dialog.connect_response(|dialog, _| dialog.close());
    dialog.present();
}

pub(super) fn poll_image_paste(s: &mut UiState) {
    if let Some((workspace, pane, png, widget)) = s.image_paste_request.take() {
        if !parse_workspace_id(&workspace)
            .and_then(|id| resident_pane_view(s, &id, pane))
            .is_some_and(|view| view.widget() == widget)
        {
            image_paste_error(s, "Original pane was closed; image was not pasted.");
            return;
        }
        match s
            .event_pump
            .client()
            .start_image_paste(&workspace, pane, &png)
        {
            Ok(()) => {
                s.image_paste_pending = true;
                let indicator = gtk4::Label::new(Some(&crate::frontend::utils::i18n::tr(
                    crate::frontend::utils::i18n::TextKey::ImagePasteProgress,
                )));
                indicator.set_widget_name("muxterm-image-paste-progress");
                s.status.container.append(&indicator);
                s.image_paste_indicator = Some(indicator);
            }
            Err(error) => image_paste_error(s, &error.to_string()),
        }
    }
    if s.image_paste_pending {
        match s.event_pump.client().poll_image_paste() {
            Ok(None) => {}
            Ok(Some(path)) => {
                s.image_paste_pending = false;
                s.notification_log.push(format!("Image pasted: {path}"));
            }
            Err(error) => {
                s.image_paste_pending = false;
                image_paste_error(s, &error.to_string());
            }
        }
    }
    if !s.image_paste_pending {
        if let Some(indicator) = s.image_paste_indicator.take() {
            s.status.container.remove(&indicator);
        }
    }
}

pub(super) fn copy_active_pane(s: &UiState) {
    copy_pane(s, s.active_pane);
}

pub(super) fn paste_active_pane(s: &UiState, state: &Rc<RefCell<UiState>>) {
    paste_pane(s, state, s.active_pane);
}

pub(super) fn split_pane_from_menu(s: &mut UiState, pane_id: u32, vertical: bool) {
    let _ = s.execute_active_task(ClientTask::SplitPane {
        pane_id,
        horizontal: !vertical,
    });
}

pub(super) fn handle_pane_menu_action(
    state: &Rc<RefCell<UiState>>,
    pane_id: u32,
    action: PaneMenuAction,
) {
    match action {
        PaneMenuAction::Break => {
            let s = state.borrow();
            if s.aggregate.kind.is_none()
                && s.active_supports(ClientRuntimeCapability::SharedClientResize)
            {
                let _ = s.execute_active_task(ClientTask::BreakPane { pane_id });
            }
        }
        PaneMenuAction::Close => {
            let s = state.borrow();
            let _ = s.execute_active_task(ClientTask::ClosePane { pane_id });
        }
        PaneMenuAction::Copy => {
            let s = state.borrow();
            copy_pane(&s, pane_id);
        }
        PaneMenuAction::Paste => {
            let s = state.borrow();
            paste_pane(&s, state, pane_id);
        }
        PaneMenuAction::SplitVertical => {
            let mut s = state.borrow_mut();
            split_pane_from_menu(&mut s, pane_id, true);
        }
        PaneMenuAction::SplitHorizontal => {
            let mut s = state.borrow_mut();
            split_pane_from_menu(&mut s, pane_id, false);
        }
    }
}
