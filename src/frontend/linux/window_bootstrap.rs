//! AppWindow startup wiring.
//!
//! Core ownership, initial ViewStore synchronization, the resident Scene
//! tree, and the single GTK EventPump loop are assembled here.

use gtk4::ApplicationWindow;

use super::window_actions::{
    handle_action, handle_pane_menu_action, prepare_core_tab_mutation, report_all_pane_colours,
};
use super::window_activity::{
    drain_attention_notifications, scroll_to_command_text, update_command_marks, update_jump_latest,
};
use super::window_discovery::{drain_local_existing, drain_ssh_probes, maybe_schedule_reconnect};
use super::window_event_pump::{
    activity_snapshot, drain_surface_input, flush_command_queue, poll_event_store,
};
use super::window_layout::refresh_ui;
use super::window_render::{
    mark_active_attention_visible, refresh_event_workspaces, sync_pane_outputs,
};
use super::window_resize::sync_window_size;
use super::window_scene::{activate_existing, request_switch_tab};
use super::window_sidebar::{
    activate_sidebar_activity, close_sidebar_workspace, maybe_warn_workspace_capacity,
    refresh_sidebar_if_open,
};
use super::window_status::{
    maybe_refresh_status, refresh_attention_chrome, refresh_connection_summary,
};
use super::*;

impl AppWindow {
    pub(super) fn new_with_keybindings(
        cfg: ClientConfig,
        theme: Theme,
        keybindings: Vec<ClientKeyBinding>,
    ) -> Self {
        let window = ApplicationWindow::builder()
            .title("muxterm")
            .default_width(960)
            .default_height(640)
            .build();
        let window: Window = window.upcast();

        let socket = {
            let s = cfg.tmux.socket.trim();
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        };
        let session = {
            let s = cfg.tmux.default_session.trim();
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        };

        let requested_tmux = socket.is_some();
        // Core FFI handle 是唯一 live WorkspacePool owner；GTK 只消费 owned DTO。
        let client = if requested_tmux {
            match FfiClient::new_connect("tmux", socket.as_deref(), session.as_deref(), None, None)
            {
                Ok(client) => client,
                Err(error) => {
                    tracing::error!(target = "muxterm::linux", "启动 tmux Core 失败: {error}");
                    FfiClient::new_connect("local", None, None, None, Some(""))
                        .expect("local runtime 必须可用")
                }
            }
        } else {
            FfiClient::new_connect("local", None, None, None, Some(""))
                .expect("local runtime 必须可用")
        };
        let attention_config = cfg.attention.clone();
        if let Err(error) = client.configure_attention(&attention_config) {
            tracing::warn!(
                target = "muxterm::linux",
                %error,
                "应用当前窗口的 Core attention 配置失败"
            );
        }
        let event_pump = EventPump::new(client);
        let mut view_store = ViewStore::default();
        event_pump
            .sync_view_store(&mut view_store)
            .expect("Core workspace snapshot 必须可用");
        let runtime_info = event_pump.client().runtime_list().unwrap_or_else(|error| {
            tracing::warn!(
                target = "muxterm::linux",
                %error,
                "读取 runtime capability snapshot 失败"
            );
            Vec::new()
        });
        let projects = match event_pump.client().config_describe() {
            Ok(snapshot) => match serde_json::from_value(snapshot.values["projects"].clone()) {
                Ok(projects) => projects,
                Err(error) => {
                    tracing::warn!(
                        target = "muxterm::config",
                        "从 Core FFI 快照读取 Project 失败: {error}"
                    );
                    Vec::new()
                }
            },
            Err(error) => {
                tracing::warn!(
                    target = "muxterm::config",
                    "通过 Core FFI 读取 Project 失败: {error}"
                );
                Vec::new()
            }
        };
        let startup_key = view_store
            .active_workspace_id()
            .or_else(|| view_store.workspace_ids().next())
            .expect("Core 必须返回 startup workspace");
        let startup_id = parse_workspace_id(startup_key)
            .expect("Core workspace id 必须保持五段格式")
            .clone();
        let mut startup_sockets = std::collections::HashMap::new();
        if requested_tmux {
            // 启动 attach 的工作区也要登记 socket（W17a 重连要用）。
            startup_sockets.insert(startup_id.clone(), socket.clone());
        }

        let theme_name = cfg.theme.name.clone().to_ascii_lowercase();
        apply_chrome_css(&theme);
        let config_font_size = cfg.font.size;
        let font = FontSettings {
            family: cfg.font.family.clone(),
            size: cfg.font.size,
            fallback: cfg.font.fallback.clone(),
        };
        let status_mode = StatusBarMode::from_toml(Some(&cfg.statusbar.mode));
        let uses_tmux = view_store
            .workspace(startup_key)
            .and_then(|view| view.workspace.as_ref())
            .is_some_and(|workspace| {
                matches!(workspace.runtime.as_str(), "tmux" | "ssh" | "tmux-ssh")
            });
        let layout = LayoutHost::new(theme.clone(), font.clone(), uses_tmux, cfg.scrollback.lines);
        let scenes = WorkspaceScenes::new(startup_id.clone(), layout);
        let scene_stack_widget = scenes.widget();
        let AppShell {
            sidebar,
            status,
            overlay,
            header,
        } = AppShell::new(&window, &scene_stack_widget, status_mode, theme.clone());

        let keymap = KeyMap::from_bindings(&keybindings);
        let qc_store = QuickConnectStore::from_project_documents(&projects);
        let state = Rc::new(RefCell::new(UiState {
            event_pump,
            command_queue: RefCell::new(CommandQueue::default()),
            scenes,
            view_store,
            visible_workspace: startup_id.clone(),
            runtime_info,
            mounted_ws: Some(startup_id.clone()),
            snapshot_seeded_this_batch: HashSet::new(),
            qc_store,
            poll_source: None,
            font,
            config_font_size,
            theme,
            theme_name,
            status,
            sidebar,
            header,
            status_mode,
            last_status_at: Instant::now()
                .checked_sub(Duration::from_secs(10))
                .unwrap_or_else(Instant::now),
            status_interval: Duration::from_secs(1),
            keymap,
            visible_tabs: HashMap::new(),
            local_tab_overrides: HashSet::new(),
            active_tab: 0,
            active_pane: 0,
            last_client_size: None,
            last_pane_sizes: HashMap::new(),
            hold_pane_resize_until: None,
            pending_client_size: None,
            pending_client_hits: 0,
            on_last_pane_exit: cfg.behavior.on_last_pane_exit,
            pending_close: false,
            compatibility_activity: CompatibilityActivity::new(cfg.attention.clone()),
            notification_log: Vec::new(),
            notification_sink: std::boxed::Box::new(GioSink::new(None)),
            panel_open: None,
            quit_requested: false,
            capacity_limit: cfg.pool.max_slots.max(1) as usize,
            capacity_warning_presented_for_slot_count: None,
            runtime_status: crate::ffi::types::BACKEND_STATUS_CONNECTED,
            status_left: None,
            status_right: None,
            workspace_sockets: startup_sockets,
            last_traffic: None,
            last_traffic_at: None,
            pending_ssh_probes: std::collections::VecDeque::new(),
            ssh_reach_cache: std::collections::HashMap::new(),
            existing: Rc::new(RefCell::new(ExistingPanelState::default())),
            existing_ssh_probing: false,
            pending_existing_ssh: std::collections::VecDeque::new(),
            pending_local_probe: std::collections::VecDeque::new(),
            last_raw_input: Vec::new(),
            surface_input_queue: Rc::new(RefCell::new(VecDeque::new())),
            reconnecting: false,
            reconnect_retry_at: None,
            reconnect_attempts: 0,
            overlay,
            last_seen: std::collections::HashMap::new(),
            scrollback_lines: cfg.scrollback.lines,
            default_socket: socket.clone(),
            self_weak: std::rc::Weak::new(),
        }));
        state.borrow_mut().self_weak = Rc::downgrade(&state);

        {
            let st = state.clone();
            let mut s = state.borrow_mut();
            if let Some(layout) = s.scenes.get_mut(&startup_id) {
                layout.set_menu_callback(move |pane_id, action| {
                    handle_pane_menu_action(&st, pane_id, action);
                });
            }
        }

        {
            let quick_state = state.clone();
            let settings_state = state.clone();
            let quick_window = window.clone();
            let settings_window = window.clone();
            let s = state.borrow();
            s.header.connect_actions(
                move || open_quick_connect(&quick_state, &quick_window),
                move || open_preferences(&settings_state, &settings_window),
            );
        }

        {
            let activate_state = state.clone();
            let close_state = state.clone();
            let agent_state = state.clone();
            let command_state = state.clone();
            let toggle_state = state.clone();
            let s = state.borrow();
            s.sidebar.connect_actions(
                move |id| {
                    activate_existing(&mut activate_state.borrow_mut(), id.clone());
                },
                move |id| {
                    close_sidebar_workspace(&mut close_state.borrow_mut(), id);
                },
                move |id, pane| {
                    activate_sidebar_activity(&mut agent_state.borrow_mut(), id, pane);
                },
                move |id, pane| {
                    activate_sidebar_activity(&mut command_state.borrow_mut(), id, pane);
                },
                move |is_active| {
                    if is_active {
                        refresh_sidebar_if_open(&mut toggle_state.borrow_mut());
                    }
                },
            );
        }

        {
            let s = state.borrow();
            let activity = activity_snapshot(&s);
            let active_workspace = s.active_workspace_key();
            s.sidebar
                .refresh_from_views(&s.view_store, Some(&active_workspace), &activity);
        }

        // status bar 业务入口 → 交给 frontend-visible scene / Core command queue。
        {
            let tab_state = state.clone();
            let attention_state = state.clone();
            let new_tab_state = state.clone();
            let worktree_state = state.clone();
            let attention_window = window.clone();
            let worktree_window = window.clone();
            let s = state.borrow();
            s.status.connect_actions(
                move |tab_id| {
                    request_switch_tab(&mut tab_state.borrow_mut(), tab_id);
                },
                move || {
                    let n = activity_snapshot(&attention_state.borrow()).blocked_count;
                    let tab = if n > 0 {
                        PanelTab::Attention
                    } else {
                        PanelTab::Workspaces
                    };
                    open_panel(&attention_state, &attention_window, tab);
                },
                move || {
                    let mut s = new_tab_state.borrow_mut();
                    prepare_core_tab_mutation(&mut s, &ClientTask::NewTab);
                    let _ = s.execute_active_task(ClientTask::NewTab);
                },
                move || {
                    show_worktree_create_dialog(&worktree_state, &worktree_window);
                },
            );
        }

        // 命令刻度点击：滚到对应命令文本所在行（W18h）。
        {
            let ok_state = state.clone();
            let fail_state = state.clone();
            let last_seen_state = state.clone();
            let find_state = state.clone();
            let jump_state = state.clone();
            let ok_text = state.borrow().overlay.command_ok_text.clone();
            let fail_text = state.borrow().overlay.command_fail_text.clone();
            let s = state.borrow();
            s.overlay.connect_actions(
                move || scroll_to_command_text(&ok_state, &ok_text),
                move || scroll_to_command_text(&fail_state, &fail_text),
                move || {
                    let s = last_seen_state.borrow();
                    let ws = active_workspace_id(&s);
                    let pane = s.active_pane;
                    if let Some(text) = s.last_seen.get(&(ws.clone(), pane)).cloned() {
                        let lines = s
                            .event_pump
                            .client()
                            .workspace_pane_last_n_lines(&ws, pane, 10_000)
                            .unwrap_or_default();
                        if let Some(row) = lines.iter().position(|l| l.contains(&text)) {
                            if let Some(view) = s.active_layout().pane(pane).cloned() {
                                if let Some(adj) = view.terminal().vadjustment() {
                                    adj.set_value(adj.lower() + row as f64);
                                }
                            }
                        }
                    }
                    s.overlay.last_seen.set_visible(false);
                },
                move |query| {
                    if query.is_empty() {
                        return;
                    }
                    let s = find_state.borrow();
                    let pane = s.active_pane;
                    let workspace_replica = active_workspace_id(&s);
                    let workspace_key = active_workspace_key(&s);
                    let hit = s
                        .event_pump
                        .client()
                        .search_all(query)
                        .ok()
                        .and_then(|hits| {
                            hits.into_iter().find(|hit| {
                                hit.workspace_id == workspace_replica && hit.pane_id == pane
                            })
                        });
                    if let Some(hit) = hit {
                        if let Some(row) = s.event_pump.client().workspace_pane_viewport_for_seq(
                            &workspace_key,
                            pane,
                            hit.seq,
                        ) {
                            if let Some(view) = s.active_layout().pane(pane).cloned() {
                                if let Some(adj) = view.terminal().vadjustment() {
                                    adj.set_value(adj.lower() + row as f64);
                                }
                            }
                        }
                    }
                },
                move || {
                    let mut s = jump_state.borrow_mut();
                    s.overlay.jump_unseen = 0;
                    if let Some(view) = s.active_layout().pane(s.active_pane).cloned() {
                        if let Some(adj) = view.terminal().vadjustment() {
                            adj.set_value(adj.upper());
                        }
                    }
                },
            );
        }

        // 快捷键
        {
            let st = state.clone();
            let window_for_palette = window.clone();
            connect_key_handler(&window, move |keyval, mods| {
                let action = {
                    let s = st.borrow();
                    s.keymap.lookup(keyval, mods)
                };
                let Some(action) = action else {
                    return glib::Propagation::Proceed;
                };
                // Ctrl+Q 必须在放下 RefCell 之后再 close：close-request 会再借同一把锁。
                if action == Action::Quit {
                    st.borrow_mut().quit_requested = true;
                    window_for_palette.close();
                    return glib::Propagation::Stop;
                }
                // QuickConnect 面板的 rebuild 会同步 borrow state：先放锁再打开。
                if action == Action::QuickConnect {
                    open_panel(&st, &window_for_palette, PanelTab::Workspaces);
                    return glib::Propagation::Stop;
                }
                if action == Action::Search {
                    open_panel(&st, &window_for_palette, PanelTab::Search);
                    return glib::Propagation::Stop;
                }
                // W18f：Ctrl+F = 当前 pane 内查找（生产路径，与 test_open_pane_find 同）。
                if keyval == gdk::Key::f
                    && mods.contains(gdk::ModifierType::CONTROL_MASK)
                    && !mods.contains(gdk::ModifierType::SHIFT_MASK)
                {
                    open_pane_find(&st, &window_for_palette);
                    return glib::Propagation::Stop;
                }
                let mut s = st.borrow_mut();
                handle_action(&mut s, action, &window_for_palette, &st);
                glib::Propagation::Stop
            });
        }

        // 关闭窗口：非 Quit 动作隐藏并保持 16ms 轮询；Quit 才真正关闭。
        // try_borrow：命令面板 Detach 可能仍握着 RefMut 时同步 close（dogfood 0826）。
        {
            let st = state.clone();
            let win = window.clone();
            connect_close_handler(&window, move || {
                let quit = st.try_borrow().map(|s| s.quit_requested).unwrap_or(false);
                match close_intent(quit) {
                    CloseIntent::Quit => glib::Propagation::Proceed,
                    CloseIntent::HideKeepPolling => {
                        win.set_visible(false);
                        glib::Propagation::Stop
                    }
                }
            });
        }

        // 首次刷新 + 窗口级 16ms 轮询（切连接后仍打到当前 active slot）
        {
            let mut s = state.borrow_mut();
            let _ = poll_event_store(&mut s);
            refresh_ui(&mut s);
            mark_active_attention_visible(&s);
            report_all_pane_colours(&mut s);
            maybe_refresh_status(&mut s, true);
        }

        {
            let st_weak = Rc::downgrade(&state);
            let win_weak = window.downgrade();
            let id = glib::timeout_add_local(Duration::from_millis(16), move || {
                // W19e：glib trampoline 不能 unwind；panic 先在这里接住，
                // 报告 + 弹窗后继续轮询（Break 会让轮询停掉 = 假死）。
                let outcome = crate::frontend::linux::fault_gtk::run("linux.poll", || {
                    if win_weak.upgrade().is_none() {
                        return glib::ControlFlow::Break;
                    }
                    let Some(st) = st_weak.upgrade() else {
                        return glib::ControlFlow::Break;
                    };
                    let pending_close = {
                        drain_ssh_probes(&st);
                        drain_local_existing(&st);
                        maybe_schedule_reconnect(&st);
                        if let Some(w) = win_weak.upgrade() {
                            maybe_warn_workspace_capacity(&st, &w);
                        }
                        let mut s = st.borrow_mut();
                        // EventPump 是唯一事件消费者：Core 的 workspace 批次先写入
                        // owned ViewStore，再由常驻 Scene 消费 render mailbox。
                        let events = poll_event_store(&mut s);
                        let structural = events.iter().any(|event| event.event.is_topology());
                        refresh_event_workspaces(&mut s, &events);
                        // blocked 与 done 通知都要在 16ms poll 里收编（W17d）：
                        // test_poll_once 的 drain 可能在 16ms poll 应用信号之前运行，
                        // 只 drain blocked 会让后台 Done 的通知永远等不到下一次 poll。
                        drain_attention_notifications(&mut s);
                        sync_pane_outputs(&mut s);
                        sync_window_size(&mut s);
                        // 输入必须在本轮 topology/snapshot/geometry 收编之后
                        // 写入，避免 attach 新 pane 尚未完成首帧时丢掉 send-keys。
                        drain_surface_input(&mut s);
                        flush_command_queue(&s);
                        maybe_refresh_status(&mut s, structural);
                        refresh_connection_summary(&mut s);
                        update_command_marks(&s);
                        update_jump_latest(&s);
                        refresh_sidebar_if_open(&mut s);
                        if let Some(w) = win_weak.upgrade() {
                            refresh_attention_chrome(&s, &w);
                        }
                        let close = s.pending_close;
                        if close {
                            s.pending_close = false;
                        }
                        close
                    };
                    if pending_close {
                        st.borrow_mut().quit_requested = true;
                        if let Some(w) = win_weak.upgrade() {
                            w.close();
                        }
                    }
                    glib::ControlFlow::Continue
                });
                // 接住 panic 后进程必须继续：fault_gtk::run 已弹窗，
                // 这里统一 Continue（不 Break，避免轮询停掉）。
                outcome.unwrap_or(glib::ControlFlow::Continue)
            });
            state.borrow_mut().poll_source = Some(id);
        }

        Self {
            window,
            _state: state,
        }
    }
}
