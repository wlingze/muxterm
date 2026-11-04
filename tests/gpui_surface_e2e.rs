//! GPUI 前端的端到端闭环：真实本地 shell 里跑一条命令，输出必须经
//! Core → EventPump → 本前端的 CellGrid 落到格子上。
//!
//! 用隔离 tmux socket 只为拿到确定性画面；键盘字节走的是 gpui 前端同一套
//! `KeyPress` → `encode` → `CommandQueue::Input` 路径。

#![cfg(feature = "gpui")]

use std::time::{Duration, Instant};

use muxterm::frontend::command_queue::{ClientCommand, CommandQueue};
use muxterm::frontend::event_pump::EventPump;
use muxterm::frontend::gpui::terminal::{encode, KeyPress, Surface};
use muxterm::frontend::utils::corebridge::{ClientEventKind, FfiClient};
use muxterm::frontend::view_store::ViewStore;

fn press(text: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    for ch in text.chars() {
        let key = ch.to_string();
        let encoded = encode(
            &KeyPress {
                key: key.clone(),
                key_char: Some(key),
                ..Default::default()
            },
            false,
        )
        .expect("可打印字符必须能编码");
        bytes.extend_from_slice(&encoded);
    }
    bytes
}

fn enter() -> Vec<u8> {
    encode(
        &KeyPress {
            key: String::from("enter"),
            key_char: None,
            ..Default::default()
        },
        false,
    )
    .expect("enter 必须能编码")
}

/// 把一批事件喂进常驻 Surface，返回是否收到过渲染数据。
fn apply_events(store: &mut ViewStore, surface: &mut Surface, workspace: &str, pane: u32) -> bool {
    let mut fed = false;
    for event in store.take_pane_render_events(workspace, pane) {
        match event.kind() {
            ClientEventKind::PaneSnapshot | ClientEventKind::PaneFrame => {
                surface.feed_full(&event.data);
                fed = true;
            }
            ClientEventKind::PaneOutput => {
                surface.feed(&event.data);
                fed = true;
            }
            _ => {}
        }
    }
    fed
}

fn screen(surface: &Surface) -> String {
    let grid = surface.grid();
    (0..grid.rows())
        .map(|row| grid.row(row).iter().map(|cell| cell.ch).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn gpui_surface_renders_shell_output_end_to_end() {
    let socket = format!(
        "muxterm-test-gpui-e2e-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.subsec_nanos())
            .unwrap_or(0)
    );
    let session = format!("muxterm-test-gpui-{socket}");

    // 隔离 socket：绝不碰用户默认 tmux server。
    let created = std::process::Command::new("tmux")
        .args(["-L", &socket, "new-session", "-d", "-s", &session])
        .status()
        .expect("tmux 必须可用");
    assert!(created.success(), "隔离 tmux session 必须创建成功");

    // tmux 是 control runtime：不给初始尺寸它不会发 attach 快照，
    // 所以这里和无 Surface 的 frontend 一样先报一个基准尺寸。
    let client =
        FfiClient::new_connect_sized("tmux", Some(&socket), Some(&session), None, None, 80, 24)
            .expect("隔离 socket 必须能连接");
    let pump = EventPump::new(client);
    let mut store = ViewStore::default();
    pump.sync_view_store(&mut store).expect("初始快照必须可用");

    let workspace = store
        .active_workspace_id()
        .expect("必须有 startup workspace")
        .to_string();
    let (pane_id, cols, rows) = store
        .workspace(&workspace)
        .and_then(|view| {
            let tab = view.active_tab_id()?;
            view.panes_for_tab(tab)
                .first()
                .map(|pane| (pane.id, pane.cols, pane.rows))
        })
        .expect("workspace 必须有一个 pane");

    let mut surface = Surface::new(cols as usize, rows as usize);

    // 泵一批拓扑与首帧，把 Surface 播种好。
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        pump.poll_into(&mut store);
        apply_events(&mut store, &mut surface, &workspace, pane_id);
        if screen(&surface).contains('$') || surface.grid().cursor() != (0, 0) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "首次播种超时，当前画面:\n{}",
            screen(&surface)
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // 走前端真实的输入路径：KeyPress → encode → CommandQueue → Core。
    let mut commands = CommandQueue::default();
    let mut typed = press("echo gpui-frame-ok");
    typed.extend_from_slice(&enter());
    commands.push(ClientCommand::Input {
        workspace_id: Some(workspace.clone()),
        pane_id,
        data: typed,
        quiet: false,
    });
    let codes = commands.flush(pump.client());
    assert_eq!(codes, vec![0], "输入提交必须成功");

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        pump.poll_into(&mut store);
        apply_events(&mut store, &mut surface, &workspace, pane_id);
        if screen(&surface).contains("gpui-frame-ok") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Surface 必须在超时前看到命令输出，当前画面:\n{}",
            screen(&surface)
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // 清理也必须带同一个 -L，只杀自己的测试 server。
    let _ = std::process::Command::new("tmux")
        .args(["-L", &socket, "kill-server"])
        .status();
}
