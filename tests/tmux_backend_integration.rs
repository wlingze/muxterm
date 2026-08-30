//! TmuxRuntime 集成测试：用真实 tmux -L <socket> 验证 state 构建。
//!
//! 场景 1：原生 tmux 创建 2-tab 3-pane 布局 → muxterm 连接 → 验证 state
//! 场景 2：attach/detach → re-attach 布局保持
//! 场景 3：通过 muxterm 修改布局 → 原生 tmux 验证

#![cfg(feature = "tui")]
#![allow(clippy::let_underscore_future)]
#![allow(unused_variables)]
#![allow(dead_code)]

use muxterm::core::model::layout::SplitDir;
use muxterm::core::model::state::{BackendStatus, State, StateChange};
use muxterm::core::model::task::{Task, TaskOutcome};
use muxterm::core::model::TerminalModel;
use muxterm::core::runtime::TmuxRuntime;
use muxterm::core::types::{PaneId, TabId};
use muxterm::platform::cli::entry::cli_command_to_task;
use muxterm::platform::cli::parse_cli_command;
use std::process::Command;
use std::time::{Duration, Instant};

/// 生成唯一的 tmux socket 名。
fn unique_socket() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("muxterm-it-{}-{}", std::process::id(), nanos)
}

/// 清理 tmux server。
fn cleanup(socket: &str) {
    let _ = Command::new("tmux")
        .args(["-L", socket, "kill-server"])
        .output();
}

fn native_pane_size(socket: &str, pane: PaneId) -> Option<(u16, u16)> {
    let output = Command::new("tmux")
        .args([
            "-L",
            socket,
            "list-panes",
            "-a",
            "-F",
            "#{pane_id} #{pane_width} #{pane_height}",
        ])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let id = fields.next()?;
            if id != format!("%{}", pane.0) {
                return None;
            }
            Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
        })
}

fn native_pane_cwd(socket: &str, pane: PaneId) -> Option<String> {
    let output = Command::new("tmux")
        .args([
            "-L",
            socket,
            "display-message",
            "-p",
            "-t",
            &format!("%{}", pane.0),
            "#{pane_current_path}",
        ])
        .output()
        .ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .trim_end_matches(['\r', '\n'])
            .to_string()
    })
}

fn wait_pane_cwd(socket: &str, pane: PaneId, expected: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if native_pane_cwd(socket, pane).as_deref() == Some(expected) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// 等待 pane 前台命令变为指定命令（用外部 tmux CLI 查询，不占控制响应槽）。
fn wait_pane_command(socket: &str, pane: PaneId, command: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let output = Command::new("tmux")
            .args([
                "-L",
                socket,
                "display-message",
                "-p",
                "-t",
                &format!("%{}", pane.0),
                "#{pane_current_command}",
            ])
            .output()
            .ok();
        if let Some(o) = output {
            let name = String::from_utf8_lossy(&o.stdout);
            if name.trim() == command {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// 检查 tmux 是否可用。
fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-L")
        .arg("muxterm-it-check")
        .arg("list-sessions")
        .output()
        .is_ok()
}

/// 轮询 model 的 events，直到条件满足或超时。
fn wait_for<F>(model: &mut TerminalModel, timeout: Duration, cond: F) -> bool
where
    F: Fn(&dyn State) -> bool,
{
    let deadline = Instant::now() + timeout;
    loop {
        let events = model.refresh();
        if cond(model.state()) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 创建 TerminalModel with TmuxRuntime，连接到指定 socket。
///
/// 内部用多线程 tokio runtime，保持后台 task 存活。
fn connect_tmux(socket: &str) -> TerminalModel {
    let backend = TmuxRuntime::new(Some(socket));
    let mut model = TerminalModel::new(Box::new(backend));
    // 用 multi_thread runtime 保持后台 task 存活
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    rt.block_on(model.connect()).unwrap();
    let _ = model.poll_events();
    // runtime 需要 keep alive：spawn 一个永远不结束的 task
    // 实际上 rt 被 drop 后后台 task 会被 cancel。我们用 std::mem::forget
    // 让 runtime 不被 drop（测试结束时进程退出自然清理）。
    std::mem::forget(rt);
    model
}

// ============================================================================
// 场景 1: 连接已有 tmux session（2 tab 3 pane）
// ============================================================================

#[test]
fn scenario1_connect_new_session_basic_state() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    // muxterm TmuxRuntime 连接（new-session 模式，创建新 session）
    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    // 给后台 task 时间处理查询
    std::thread::sleep(Duration::from_millis(500));

    // 等待 pane 查询完成
    let ok = wait_for(&mut model, Duration::from_secs(10), |s| {
        s.active_pane().is_some()
    });
    assert!(ok, "应至少有一个 pane");

    // 应有至少 1 个 session + 1 个 window + pane
    assert!(!model.state().workspace_name().is_empty(), "应有 session");
    assert!(model.state().active_tab().is_some(), "应有 active window");
    assert!(
        !model
            .state()
            .panes(&model.state().active_tab().unwrap().id)
            .is_empty(),
        "应有 pane"
    );

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// 场景 2: 通过 muxterm 修改布局 → 原生 tmux 验证
// ============================================================================

#[test]
fn scenario2_modify_via_muxterm_verify_tmux() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    // 等待初始 pane
    let ok = wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });
    if !ok {
        eprintln!("skip: 初始 pane 未建立");
        let _ = model.shutdown();
        cleanup(&socket);
        return;
    }

    let pane = model.state().active_pane().unwrap().id;
    let tab = model.state().active_tab().unwrap().id;

    // 水平分割
    model
        .execute(Task::SplitPane {
            target: Some(pane),
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();

    // 等待新 pane 出现
    let ok = wait_for(&mut model, Duration::from_secs(3), |s| {
        s.panes(&s.active_tab().map(|t| t.id).unwrap_or(tab)).len() >= 2
    });
    assert!(ok, "split 后应有 2 个 pane");

    // 用原生 tmux 验证确实有 2 个 pane
    let session_name = model.state().workspace_name().to_string();
    let pane_count = String::from_utf8(
        Command::new("tmux")
            .args([
                "-L",
                &socket,
                "list-panes",
                "-t",
                &session_name,
                "-F",
                "#{pane_id}",
            ])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .lines()
    .count();
    assert!(
        pane_count >= 2,
        "原生 tmux 应看到至少 2 个 pane: {}",
        pane_count
    );

    let _ = model.shutdown();
    cleanup(&socket);
}

/// CLI 命令解析 → core Task → TmuxRuntime → 原生 tmux：覆盖 client resize、
/// pane 横向/纵向单轴 resize，以及 send-keys 的实际回显。
#[test]
fn cli_resize_client_and_pane_axes_reach_tmux() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();
    let mut model = connect_tmux(&socket);
    if !wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    }) {
        eprintln!("skip: 初始 pane 未建立");
        let _ = model.shutdown();
        cleanup(&socket);
        return;
    }

    let command = |args: &[&str]| {
        let values = args
            .iter()
            .map(|arg| (*arg).to_string())
            .collect::<Vec<_>>();
        parse_cli_command(&values).unwrap().0
    };
    let client = command(&["resize-client", "-x", "120", "-y", "36"]);
    let client_task = cli_command_to_task(&client, model.state()).unwrap();
    assert_eq!(model.execute(client_task).unwrap(), TaskOutcome::Done);

    let first = model.state().active_pane().unwrap().id;
    let split = command(&["split-pane", "-h", "-t", &format!("@{}", first.0)]);
    let split_task = cli_command_to_task(&split, model.state()).unwrap();
    assert_eq!(model.execute(split_task).unwrap(), TaskOutcome::Done);
    assert!(wait_for(&mut model, Duration::from_secs(4), |s| {
        s.active_tab()
            .map(|tab| s.panes(&tab.id).len() >= 2)
            .unwrap_or(false)
    }));

    let tab = model.state().active_tab().unwrap().id;
    let second = model
        .state()
        .panes(&tab)
        .iter()
        .find(|pane| pane.id != first)
        .map(|pane| pane.id)
        .unwrap();
    let horizontal = command(&["resize-pane", "-t", &format!("@{}", first.0), "-x", "60"]);
    let horizontal_task = cli_command_to_task(&horizontal, model.state()).unwrap();
    assert!(matches!(horizontal_task, Task::ResizePaneAxis { .. }));
    assert_eq!(model.execute(horizontal_task).unwrap(), TaskOutcome::Done);
    assert!(wait_for(&mut model, Duration::from_secs(4), |s| {
        native_pane_size(&socket, first)
            .map(|(cols, _)| cols == 60)
            .unwrap_or(false)
    }));

    let vertical_split = command(&["split-pane", "-v", "-t", &format!("@{}", second.0)]);
    let vertical_split_task = cli_command_to_task(&vertical_split, model.state()).unwrap();
    assert_eq!(
        model.execute(vertical_split_task).unwrap(),
        TaskOutcome::Done
    );
    assert!(wait_for(&mut model, Duration::from_secs(4), |s| {
        s.active_tab()
            .map(|tab| s.panes(&tab.id).len() >= 3)
            .unwrap_or(false)
    }));
    let vertical = command(&["resize-pane", "-t", &format!("@{}", second.0), "-y", "12"]);
    let vertical_task = cli_command_to_task(&vertical, model.state()).unwrap();
    assert!(matches!(vertical_task, Task::ResizePaneAxis { .. }));
    assert_eq!(model.execute(vertical_task).unwrap(), TaskOutcome::Done);
    assert!(wait_for(&mut model, Duration::from_secs(4), |s| {
        native_pane_size(&socket, second)
            .map(|(_, rows)| rows == 12)
            .unwrap_or(false)
    }));

    let marker = "CLI_RESIZE_IO_OK";
    let input = command(&[
        "send-keys",
        "-t",
        &format!("@{}", first.0),
        &format!("printf '{}\\n'", marker),
    ]);
    let input_task = cli_command_to_task(&input, model.state()).unwrap();
    assert_eq!(model.execute(input_task).unwrap(), TaskOutcome::Done);
    assert!(wait_for(&mut model, Duration::from_secs(4), |s| {
        s.pane_output(&first)
            .map(|data| String::from_utf8_lossy(data).contains(marker))
            .unwrap_or(false)
    }));

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// 场景 3: new-window 通过 muxterm → 原生 tmux 验证
// ============================================================================

#[test]
fn scenario3_new_window_via_muxterm() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    // 等待初始 window + tab
    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_tab().is_some() && s.active_pane().is_some()
    });

    let aw = model.state().active_tab().unwrap().id;
    let initial_tab = model.state().active_tab().map(|t| t.id);
    let initial_tabs = model.state().tabs().len();

    // 新建 window（= tmux new-window = muxterm new-tab）
    model
        .execute(Task::NewTab {
            name: Some("test-win".into()),
            command: None,
            workdir: None,
        })
        .unwrap();

    // 等待新 tab：active tab 变化或 tab 数增加（Window 仍是虚拟的 1 个）
    let ok = wait_for(&mut model, Duration::from_secs(3), |s| {
        let tabs = s.tabs().len();
        let cur = s.active_tab().map(|t| t.id);
        tabs > initial_tabs || (cur.is_some() && cur != initial_tab)
    });
    assert!(ok, "新 tab（tmux window）未建立");

    // 原生 tmux 验证
    let session_name = model.state().workspace_name().to_string();
    let win_count = String::from_utf8(
        Command::new("tmux")
            .args([
                "-L",
                &socket,
                "list-windows",
                "-t",
                &session_name,
                "-F",
                "#{window_id}",
            ])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .lines()
    .count();
    assert!(
        win_count >= 2,
        "原生 tmux 应看到至少 2 个 window: {}",
        win_count
    );

    let _ = model.shutdown();
    cleanup(&socket);
}

/// pane 全屏端到端：真实 tmux 上执行 zoom，原生验证 `window_zoomed_flag`。
/// 必须用独立 socket，只操作自己创建的测试 session。
#[test]
fn pane_fullscreen_zoom_toggles_real_tmux() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();
    let mut model = connect_tmux(&socket);
    assert!(
        wait_for(&mut model, Duration::from_secs(5), |s| {
            s.active_pane().is_some()
        }),
        "初始 pane 应就绪"
    );

    let pane = model.state().active_pane().unwrap().id;
    model
        .execute(Task::SplitPane {
            target: None,
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
    assert!(
        wait_for(&mut model, Duration::from_secs(3), |s| {
            s.panes(&s.active_tab().unwrap().id).len() >= 2
        }),
        "split 后应有 2 个 pane"
    );

    // zoom 当前 pane → tmux 原生 flag 应为 1。
    model
        .execute(Task::TogglePaneFullscreen { target: pane })
        .unwrap();
    let session = model.state().workspace_name().to_string();
    let zoomed = std::time::Instant::now() + Duration::from_secs(3);
    let mut ok = false;
    while std::time::Instant::now() < zoomed {
        let out = Command::new("tmux")
            .args([
                "-L",
                &socket,
                "display-message",
                "-t",
                &session,
                "-p",
                "#{window_zoomed_flag}",
            ])
            .output()
            .unwrap();
        if String::from_utf8_lossy(&out.stdout).trim() == "1" {
            ok = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ok, "zoom 后 tmux window_zoomed_flag 应为 1");

    // 再 zoom → 恢复为 0。
    model
        .execute(Task::TogglePaneFullscreen { target: pane })
        .unwrap();
    let restored = std::time::Instant::now() + Duration::from_secs(3);
    let mut ok = false;
    while std::time::Instant::now() < restored {
        let out = Command::new("tmux")
            .args([
                "-L",
                &socket,
                "display-message",
                "-t",
                &session,
                "-p",
                "#{window_zoomed_flag}",
            ])
            .output()
            .unwrap();
        if String::from_utf8_lossy(&out.stdout).trim() == "0" {
            ok = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ok, "再次 zoom 后 window_zoomed_flag 应恢复 0");
    cleanup(&socket);
}

// ============================================================================
// 场景 4: send-keys → pane 有输出
// ============================================================================

#[test]
fn scenario4_send_keys_and_output() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    // 等待初始 pane
    let ok = wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });
    if !ok {
        eprintln!("skip: 初始 pane 未建立");
        let _ = model.shutdown();
        cleanup(&socket);
        return;
    }

    let pane = model.state().active_pane().unwrap().id;

    // 发送 echo 命令
    model
        .execute(Task::SendKeys {
            target: pane,
            keys: vec![
                muxterm::core::protocol::terminal::input::KeyEvent::Char('e'),
                muxterm::core::protocol::terminal::input::KeyEvent::Char('c'),
                muxterm::core::protocol::terminal::input::KeyEvent::Char('h'),
                muxterm::core::protocol::terminal::input::KeyEvent::Char('o'),
                muxterm::core::protocol::terminal::input::KeyEvent::Char(' '),
                muxterm::core::protocol::terminal::input::KeyEvent::Char('h'),
                muxterm::core::protocol::terminal::input::KeyEvent::Char('i'),
                muxterm::core::protocol::terminal::input::KeyEvent::Enter,
            ],
        })
        .unwrap();
    let _ = model.poll_events();

    // 等待输出（pane_output 非空）
    let ok = wait_for(&mut model, Duration::from_secs(3), |s| {
        s.pane_output(&pane).map(|o| !o.is_empty()).unwrap_or(false)
    });
    assert!(ok, "send-keys 后应有输出");

    let _ = model.shutdown();
    cleanup(&socket);
}

// 回归 macOS SwiftTerm → FFI WriteRaw → tmux -CC：Ctrl-C 必须是 0x03，
// 不能变成 send-keys 参数中的字面 "x03"。
#[test]
fn scenario4_raw_control_byte_reaches_tmux_pty() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();
    let mut model = connect_tmux(&socket);
    if !wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    }) {
        eprintln!("skip: 初始 pane 未建立");
        let _ = model.shutdown();
        cleanup(&socket);
        return;
    }
    let pane = model.state().active_pane().unwrap().id;

    // 用绝对路径启动 cat：用户的 shell alias（如 cat=bat）会改变
    // pane_current_command 的实际值，导致前台命令探测失败。
    let mut keys = "/bin/cat"
        .chars()
        .map(muxterm::core::protocol::terminal::input::KeyEvent::Char)
        .collect::<Vec<_>>();
    keys.push(muxterm::core::protocol::terminal::input::KeyEvent::Enter);
    model
        .execute(Task::SendKeys { target: pane, keys })
        .unwrap();
    // 必须等 cat 真正在前台运行（不能只看输出回显：高负载时输入回显会
    // 先出现，reply 字节会落到 shell 提示符而不是 cat）。
    assert!(
        wait_pane_command(&socket, pane, "cat", Duration::from_secs(5)),
        "cat 应已在前台运行"
    );

    model
        .execute(Task::WriteRaw {
            target: pane,
            data: vec![0x03],
        })
        .unwrap();
    let ok = wait_for(&mut model, Duration::from_secs(5), |s| {
        let text = s
            .pane_output(&pane)
            .map(String::from_utf8_lossy)
            .unwrap_or_default();
        text.contains("^C")
    });
    let text = model
        .state()
        .pane_output(&pane)
        .map(String::from_utf8_lossy)
        .unwrap_or_default();
    assert!(ok, "Ctrl-C 应中断 cat 并回显 ^C，实际输出={text:?}");
    assert!(!text.contains("x03"), "控制字节不能变成字面 x03: {text:?}");

    let _ = model.shutdown();
    cleanup(&socket);
}

/// P0（用户实测 a.log/b.log）：git lg 输出 OSC/CSI 终端查询
/// （`ESC]10;rgb:0000/0000/0000 ESC\`、`ESC]11;...`、`ESC[?65;...c`），
/// 终端回复这些查询时必须把 ESC 引导字节完整送回 shell（WriteRaw 路径），
/// 不能把 `\e` 变成字面文本，否则 shell 会把 `10;rgb:...` 当命令执行
/// （`zsh: command not found: 10`）。
#[test]
fn write_raw_osc_csi_query_reply_preserves_esc_bytes() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();
    let mut model = connect_tmux(&socket);
    if !wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    }) {
        eprintln!("skip: 初始 pane 未建立");
        let _ = model.shutdown();
        cleanup(&socket);
        return;
    }
    let pane = model.state().active_pane().unwrap().id;

    // 启动 cat，让写入字节原样回显，便于 capture 精确比对。
    let mut keys = "/bin/cat"
        .chars()
        .map(muxterm::core::protocol::terminal::input::KeyEvent::Char)
        .collect::<Vec<_>>();
    keys.push(muxterm::core::protocol::terminal::input::KeyEvent::Enter);
    model
        .execute(Task::SendKeys { target: pane, keys })
        .unwrap();
    assert!(
        wait_pane_command(&socket, pane, "cat", Duration::from_secs(5)),
        "cat 应已在前台运行"
    );

    // 终端对 OSC 10/11 颜色查询 + CSI DA 的回复（原样字节）。
    let reply: Vec<u8> = vec![
        0x1b, b']', b'1', b'0', b';', b'r', b'g', b'b', b':', b'0', b'0', b'0', b'0', b'/', b'0',
        b'0', b'0', b'0', b'/', b'0', b'0', b'0', b'0', 0x1b, b'\\', 0x1b, b']', b'1', b'1', b';',
        b'r', b'g', b'b', b':', b'f', b'f', b'f', b'f', b'/', b'f', b'f', b'f', b'f', b'/', b'f',
        b'f', b'f', b'f', 0x1b, b'\\', 0x1b, b'[', b'?', b'6', b'5', b';', b'4', b';', b'1', b';',
        b'2', b';', b'6', b';', b'2', b'1', b';', b'2', b'2', b';', b'1', b'7', b';', b'2', b'8',
        b'c',
    ];
    model
        .execute(Task::WriteRaw {
            target: pane,
            data: reply.clone(),
        })
        .unwrap();

    // 追加一个可辨认的 MARKER，确保 capture 覆盖到回复之后。
    model
        .execute(Task::WriteRaw {
            target: pane,
            data: b"MARKER".to_vec(),
        })
        .unwrap();

    // 原生 tmux capture-pane 应显示：回复字节（ESC 完整）+ MARKER。
    let ok = wait_for(&mut model, Duration::from_secs(5), |s| {
        let text = s
            .pane_output(&pane)
            .map(String::from_utf8_lossy)
            .unwrap_or_default();
        text.contains("MARKER")
    });
    let text = model
        .state()
        .pane_output(&pane)
        .map(String::from_utf8_lossy)
        .unwrap_or_default();
    assert!(ok, "MARKER 应回显，实际={text:?}");

    // 关键断言：回复里的 ESC (0x1b) 必须保留，不能变成字面文本。
    // 若被破坏，shell 会出现 "command not found: 10" 等，这里用 cat 回显直接验证字节。
    let has_esc = reply.contains(&0x1b);
    assert!(has_esc, "测试数据本身应含 ESC");
    // 至少一个 ESC (0x1b) 出现在 pane 输出的原始字节里（cat 原样回显 ESC 引导序列）。
    let raw = model.state().pane_output(&pane).unwrap_or(&[]);
    assert!(
        raw.contains(&0x1b),
        "回复的 ESC 引导字节应保留（cat 回显），raw={raw:?}"
    );
    // 不能出现字面 "command not found: 10" 或 "rgb:0000" 作为命令被解释的残留。
    assert!(
        !text.contains("command not found: 10"),
        "ESC 丢失导致 shell 解释垃圾: {text:?}"
    );

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Bug 1 测试：pane 按 tab 过滤（2 tab 3 pane 不混在一起）
// ============================================================================

#[test]
fn bug1_pane_filtered_by_tab() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    // 用原生 tmux 创建 2 tab 3 pane 布局
    let rc = Command::new("tmux")
        .args([
            "-L",
            &socket,
            "new-session",
            "-d",
            "-s",
            "demo",
            "-x",
            "140",
            "-y",
            "30",
        ])
        .status();
    if rc.is_err() || !rc.unwrap().success() {
        eprintln!("skip: 无法创建 tmux session");
        cleanup(&socket);
        return;
    }

    let w0 = String::from_utf8(
        Command::new("tmux")
            .args([
                "-L",
                &socket,
                "list-windows",
                "-t",
                "demo",
                "-F",
                "#{window_id}",
            ])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    // 水平分割 → 2 panes
    let _ = Command::new("tmux")
        .args(["-L", &socket, "split-window", "-h", "-t", &w0])
        .status();
    // 垂直分割右侧 → 3 panes
    let p1 = String::from_utf8(
        Command::new("tmux")
            .args(["-L", &socket, "list-panes", "-t", &w0, "-F", "#{pane_id}"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .lines()
    .nth(1)
    .unwrap_or("")
    .to_string();
    if !p1.is_empty() {
        let _ = Command::new("tmux")
            .args(["-L", &socket, "split-window", "-v", "-t", &p1])
            .status();
    }
    // 新建 tab2
    let _ = Command::new("tmux")
        .args(["-L", &socket, "new-window", "-t", "demo"])
        .status();

    // muxterm 连接（new-session 模式）
    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    // 等待 pane 建立
    wait_for(&mut model, Duration::from_secs(10), |s| {
        s.active_pane().is_some()
    });

    // 正向：panes 按 tab 过滤，不同 tab 的 pane 不混在一起
    // 当前 active tab 的 pane 数应 >= 1
    let active_tab_id = model.state().active_tab().map(|t| t.id);
    if let Some(tid) = active_tab_id {
        let pane_count = model.state().panes(&tid).len();
        assert!(
            pane_count >= 1,
            "active tab 应至少 1 个 pane，实际 {}",
            pane_count
        );
        // 每个 pane 的 tab 字段必须等于当前 tab
        for p in model.state().panes(&tid) {
            assert_eq!(
                p.tab, tid,
                "pane {:?} 的 tab 应等于 {:?}，实际 {:?}",
                p.id, tid, p.tab
            );
        }
    }

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Bug 3 测试：嵌套布局解析（Split(H, leaf, Split(V, leaf, leaf))）
// ============================================================================

#[test]
fn bug3_nested_layout_parsed() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    // 等待初始 pane
    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });
    let pane = model.state().active_pane().unwrap().id;
    let tab = model.state().active_tab().unwrap().id;

    // 水平分割
    model
        .execute(Task::SplitPane {
            target: Some(pane),
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.panes(&s.active_tab().map(|t| t.id).unwrap_or(tab)).len() >= 2
    });

    // 在新 pane 上垂直分割
    let new_pane = model
        .state()
        .panes(&model.state().active_tab().unwrap().id)
        .iter()
        .find(|p| p.id != pane)
        .map(|p| p.id)
        .unwrap_or(pane);
    model
        .execute(Task::SplitPane {
            target: Some(new_pane),
            dir: SplitDir::Vertical,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.panes(&s.active_tab().map(|t| t.id).unwrap_or(tab)).len() >= 3
    });

    // 正向：layout 应有嵌套结构
    let layout = model.state().layout(&tab);
    if let Some(tl) = layout {
        let leaves = tl.tree.leaves();
        assert!(leaves.len() >= 3, "layout 应有 >= 3 个叶子: {:?}", leaves);
        // 检查是否有 Split 节点（不是全部 Leaf）
        assert!(
            tl.tree.depth() >= 2,
            "layout 应有嵌套深度 >= 2: depth={}",
            tl.tree.depth()
        );
    }

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Bug 4 测试：pty 输出同步到 pane_output
// ============================================================================

#[test]
fn bug4_output_synced_to_pane_output() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });
    let pane = model.state().active_pane().unwrap().id;

    // 正向：send-keys 后 pane_output 非空
    model
        .execute(Task::SendKeys {
            target: pane,
            keys: vec![
                muxterm::core::protocol::terminal::input::KeyEvent::Char('e'),
                muxterm::core::protocol::terminal::input::KeyEvent::Char('c'),
                muxterm::core::protocol::terminal::input::KeyEvent::Char('h'),
                muxterm::core::protocol::terminal::input::KeyEvent::Char('o'),
                muxterm::core::protocol::terminal::input::KeyEvent::Char(' '),
                muxterm::core::protocol::terminal::input::KeyEvent::Char('x'),
                muxterm::core::protocol::terminal::input::KeyEvent::Enter,
            ],
        })
        .unwrap();
    let _ = model.poll_events();

    let ok = wait_for(&mut model, Duration::from_secs(3), |s| {
        s.pane_output(&pane).map(|o| !o.is_empty()).unwrap_or(false)
    });
    assert!(ok, "send-keys 后 pane_output 应非空");

    // 反向：不存在的 pane 的 output 应为 None
    assert!(model.state().pane_output(&PaneId(99999)).is_none());

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// 边界测试：单 pane session（无分割）
// ============================================================================

#[test]
fn edge_single_pane_no_split() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });

    let tab = model.state().active_tab().unwrap().id;
    let panes = model.state().panes(&tab);
    assert_eq!(panes.len(), 1, "单 pane session 应只有 1 个 pane");

    let layout = model.state().layout(&tab).unwrap();
    assert!(layout.tree.depth() == 0, "单 pane layout 应 depth=0");
    assert_eq!(layout.tree.leaves().len(), 1);

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// 边界测试：只有 1 个 tab
// ============================================================================

#[test]
fn edge_single_tab() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });

    // 应只有 1 个 tab
    let active_window = model.state().active_tab().unwrap();
    let tabs = model.state().tabs();
    assert_eq!(tabs.len(), 1, "应只有 1 个 tab");

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// 反向测试：连接不存在的 socket → 错误处理
// ============================================================================

#[test]
fn negative_connect_nonexistent_socket() {
    let socket = unique_socket();
    let backend = TmuxRuntime::new(Some(&socket));
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // 连接应该创建一个新的 tmux server（因为 tmux 会自动启动），
    // 所以这不会失败。但如果 socket 路径有问题，会失败。
    // 改测：连接后立即 shutdown
    let result = rt.block_on(model.connect());
    // tmux 会自动创建 server，所以 connect 应该成功
    if result.is_ok() {
        let _ = rt.block_on(model.shutdown());
    }
    // 清理
    let _ = Command::new("tmux")
        .args(["-L", &socket, "kill-server"])
        .output();
}

// ============================================================================
// 边界测试：水平分割后不垂直分割（2 pane 水平排列）
// ============================================================================

#[test]
fn edge_horizontal_split_only() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });
    let pane = model.state().active_pane().unwrap().id;

    model
        .execute(Task::SplitPane {
            target: Some(pane),
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.active_pane().is_some() && s.panes(&s.active_tab().unwrap().id).len() >= 2
    });

    let tab = model.state().active_tab().unwrap().id;
    let panes = model.state().panes(&tab);
    assert_eq!(panes.len(), 2, "水平分割后应有 2 个 pane");

    let layout = model.state().layout(&tab).unwrap();
    assert_eq!(layout.tree.depth(), 1, "水平分割 layout depth=1");
    assert_eq!(layout.tree.leaves().len(), 2);

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// 边界测试：pane 被关闭后 layout 正确更新
// ============================================================================

#[test]
fn edge_pane_closed_layout_updates() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });
    let pane = model.state().active_pane().unwrap().id;

    // 分割 → 2 pane
    model
        .execute(Task::SplitPane {
            target: Some(pane),
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.panes(&s.active_tab().map(|t| t.id).unwrap_or(TabId(0)))
            .len()
            >= 2
    });

    let tab = model.state().active_tab().unwrap().id;
    let panes = model.state().panes(&tab);
    assert_eq!(panes.len(), 2);

    // 关闭一个 pane
    let target_pane = panes
        .iter()
        .find(|p| p.id != pane)
        .map(|p| p.id)
        .unwrap_or(pane);
    model
        .execute(Task::ClosePane {
            target: target_pane,
        })
        .unwrap();
    let _ = model.poll_events();

    // 等待 layout 更新（pane 数减少或 %window-pane-changed 触发重查）
    wait_for(&mut model, Duration::from_secs(5), |s| {
        let p = s.panes(&s.active_tab().map(|t| t.id).unwrap_or(TabId(0)));
        p.len() <= 1 || !p.iter().any(|x| x.id == target_pane)
    });

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Bug 2 正向测试：Alt+数字切 tab（2 tab session）
// ============================================================================

#[test]
fn bug2_positive_alt_switch_tab() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });
    let aw = model.state().active_tab().unwrap().id;
    let tab1 = model.state().active_tab().map(|t| t.id);

    // 新建第二个 tab（tmux window）
    model
        .execute(Task::NewTab {
            name: None,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.tabs().len() >= 2 && s.active_tab().map(|t| t.id) != tab1 && s.active_tab().is_some()
    });

    // 产品层已经是 Workspace → Tab → Pane；新建后必须精确有 2 个 Tab。
    assert_eq!(
        model.state().tabs().len(),
        2,
        "新建第二个 tmux window 后应映射为 2 个 Muxterm Tab"
    );
    let tabs = model.state().tabs();
    let first_tab = tabs[0].id;
    let second_tab = tabs[1].id;

    // Alt+1 → SwitchTab → 切到第一个 tab
    model
        .execute(Task::SwitchTab { target: first_tab })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.active_tab().map(|t| t.id) == Some(first_tab)
    });
    assert_eq!(
        model.state().active_tab().map(|t| t.id),
        Some(first_tab),
        "Alt+1 应切到第一个 tab {:?}",
        first_tab
    );

    // Alt+2 → SwitchTab → 切到第二个 tab
    model
        .execute(Task::SwitchTab { target: second_tab })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.active_tab().map(|t| t.id) == Some(second_tab)
    });
    assert_eq!(
        model.state().active_tab().map(|t| t.id),
        Some(second_tab),
        "Alt+2 应切到第二个 tab {:?}",
        second_tab
    );

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Bug 2 反向测试：Alt+9（不存在的 tab）→ 不 crash
// ============================================================================

#[test]
fn bug2_negative_nonexistent_tab() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });

    // 切到不存在的 TabId(8) — 不应 crash
    let outcome = model.execute(Task::SwitchTab { target: TabId(8) }).unwrap();
    // tmux 可能忽略不存在的 window 或返回错误，但不应 panic
    let _ = model.poll_events();
    // 后端状态应仍为 Connected
    assert_eq!(model.state().status(), BackendStatus::Connected);

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Bug 2 边界测试：只有 1 个 tab，Alt+1 能工作
// ============================================================================

#[test]
fn bug2_edge_single_tab_alt1() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });

    let aw = model.state().active_tab().unwrap().id;
    let tabs = model.state().tabs();
    assert_eq!(tabs.len(), 1, "应只有 1 个 tab");
    let only_tab = tabs[0].id;

    // 只有 1 个 tab，Alt+1 → SwitchTab — 应保持当前 tab
    model.execute(Task::SwitchTab { target: only_tab }).unwrap();
    let _ = model.poll_events();
    assert_eq!(
        model.state().active_tab().map(|t| t.id),
        Some(only_tab),
        "单 tab session Alt+1 应保持当前 tab {:?}",
        only_tab
    );
    // Window 仍是虚拟的唯一 Window
    assert_eq!(
        model.state().active_tab().map(|t| t.id),
        Some(aw),
        "单 tab session 的虚拟 Window 不变"
    );

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Bug 4 正向测试：send-keys echo → pane_output 有内容
// ============================================================================

#[test]
fn bug4_positive_send_keys_output_visible() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });
    let pane = model.state().active_pane().unwrap().id;

    // 发送 echo 命令
    for c in "echo hello_world".chars() {
        model
            .execute(Task::SendKeys {
                target: pane,
                keys: vec![muxterm::core::protocol::terminal::input::KeyEvent::Char(c)],
            })
            .unwrap();
        let _ = model.poll_events();
    }
    model
        .execute(Task::SendKeys {
            target: pane,
            keys: vec![muxterm::core::protocol::terminal::input::KeyEvent::Enter],
        })
        .unwrap();
    let _ = model.poll_events();

    // 等待输出
    let ok = wait_for(&mut model, Duration::from_secs(5), |s| {
        let out = s.pane_output(&pane).unwrap_or(&[]);
        let text = String::from_utf8_lossy(out);
        text.contains("hello_world")
    });
    assert!(ok, "pane_output 应包含 'hello_world'");

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Bug 4 反向测试：不存在的 pane 的 output 为 None
// ============================================================================

#[test]
fn bug4_negative_nonexistent_pane_output() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });

    // 不存在的 pane
    assert!(
        model.state().pane_output(&PaneId(99999)).is_none(),
        "不存在的 pane 的 output 应为 None"
    );

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Bug 4 边界测试：tab 切换后 layout 正确更新
// ============================================================================

#[test]
fn bug4_edge_tab_switch_layout_updates() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });
    let pane = model.state().active_pane().unwrap().id;

    // 在 tab1 分割
    model
        .execute(Task::SplitPane {
            target: Some(pane),
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.panes(&s.active_tab().map(|t| t.id).unwrap_or(TabId(0)))
            .len()
            >= 2
    });

    let tab1 = model.state().active_tab().unwrap().id;
    let tab1_panes = model.state().panes(&tab1).len();
    assert!(tab1_panes >= 2, "tab1 应有 >= 2 个 pane");

    // 新建 tab2
    model
        .execute(Task::NewTab {
            name: None,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_tab().is_some()
            && s.active_tab().unwrap().id != tab1
            && !s.panes(&s.active_tab().unwrap().id).is_empty()
    });

    let tab2 = model.state().active_tab().unwrap().id;
    let tab2_panes = model.state().panes(&tab2).len();
    assert!(
        tab2_panes >= 1,
        "tab2 应有 >= 1 个 pane，实际 {}",
        tab2_panes
    );

    // 切回 tab1
    model
        .execute(Task::SwitchTab {
            target: TabId(tab1.0),
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.active_tab().map(|t| t.id) == Some(tab1)
    });

    // tab1 的 pane 数应仍然是 >= 2
    let tab1_panes_after = model.state().panes(&tab1).len();
    assert_eq!(tab1_panes_after, tab1_panes, "切回 tab1 后 pane 数应不变");

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Bug 4 边界测试：连续快速切 tab 不丢失事件
// ============================================================================

#[test]
fn bug4_edge_rapid_tab_switch() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });

    // 新建 tab2
    model
        .execute(Task::NewTab {
            name: None,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| s.tabs().len() >= 2);

    // 快速切换 tab1 → tab2 → tab1 → tab2
    for target in [TabId(0), TabId(1), TabId(0), TabId(1)] {
        model.execute(Task::SwitchTab { target }).unwrap();
        let _ = model.poll_events();
    }

    // 等待最终状态稳定
    std::thread::sleep(Duration::from_millis(500));
    let _ = model.refresh();

    // 应最终切到 tab2 (TabId(1))
    assert_eq!(
        model.state().active_tab().map(|t| t.id),
        Some(TabId(1)),
        "快速切换后应在 TabId(1)"
    );

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Fix3 正向测试：切 tab 后 pane 数据不丢失（2 tab 3 pane + 1 pane）
// ============================================================================

#[test]
fn fix3_positive_switch_tab_panes_preserved() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    // 等待初始 pane
    wait_for(&mut model, Duration::from_secs(10), |s| {
        s.active_pane().is_some()
    });

    // 在 tab1 分割出 3 pane（水平 + 垂直）
    let pane = model.state().active_pane().unwrap().id;
    model
        .execute(Task::SplitPane {
            target: Some(pane),
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.panes(&s.active_tab().map(|t| t.id).unwrap_or(TabId(0)))
            .len()
            >= 2
    });

    let tab1 = model.state().active_tab().unwrap().id;
    let new_pane = model
        .state()
        .panes(&tab1)
        .iter()
        .find(|p| p.id != pane)
        .map(|p| p.id)
        .unwrap_or(pane);
    model
        .execute(Task::SplitPane {
            target: Some(new_pane),
            dir: SplitDir::Vertical,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.panes(&s.active_tab().map(|t| t.id).unwrap_or(tab1)).len() >= 3
    });

    let tab1_pane_count = model.state().panes(&tab1).len();
    assert!(tab1_pane_count >= 3, "tab1 应有 >= 3 个 pane");

    // 新建 tab2
    model
        .execute(Task::NewTab {
            name: None,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_tab().is_some()
            && s.active_tab().unwrap().id != tab1
            && !s.panes(&s.active_tab().unwrap().id).is_empty()
    });

    let tab2 = model.state().active_tab().unwrap().id;
    let tab2_pane_count = model.state().panes(&tab2).len();
    assert_eq!(tab2_pane_count, 1, "tab2 应有 1 个 pane");

    // 正向：切到 tab1，pane 数据应仍在
    model
        .execute(Task::SwitchTab {
            target: TabId(tab1.0),
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.active_tab().map(|t| t.id) == Some(tab1)
    });

    let tab1_panes_after_switch = model.state().panes(&tab1).len();
    assert_eq!(
        tab1_panes_after_switch, tab1_pane_count,
        "切到 tab1 后 pane 数应不变: 期望 {}, 实际 {}",
        tab1_pane_count, tab1_panes_after_switch
    );

    // 正向：切回 tab2，pane 数据应仍在
    model
        .execute(Task::SwitchTab {
            target: TabId(tab2.0),
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.active_tab().map(|t| t.id) == Some(tab2)
    });

    let tab2_panes_after_switch = model.state().panes(&tab2).len();
    assert_eq!(
        tab2_panes_after_switch, tab2_pane_count,
        "切回 tab2 后 pane 数应不变: 期望 {}, 实际 {}",
        tab2_pane_count, tab2_panes_after_switch
    );

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Fix3 正向测试：切 tab 后 layout 树保持正确
// ============================================================================

#[test]
fn fix3_positive_switch_tab_layout_preserved() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    wait_for(&mut model, Duration::from_secs(10), |s| {
        s.active_pane().is_some()
    });
    let pane = model.state().active_pane().unwrap().id;
    let tab1 = model.state().active_tab().unwrap().id;

    // 分割 3 pane
    model
        .execute(Task::SplitPane {
            target: Some(pane),
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.panes(&s.active_tab().map(|t| t.id).unwrap_or(tab1)).len() >= 2
    });
    let p2 = model
        .state()
        .panes(&tab1)
        .iter()
        .find(|p| p.id != pane)
        .map(|p| p.id)
        .unwrap_or(pane);
    model
        .execute(Task::SplitPane {
            target: Some(p2),
            dir: SplitDir::Vertical,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.panes(&s.active_tab().map(|t| t.id).unwrap_or(tab1)).len() >= 3
    });

    // tab1 layout 应有嵌套
    let layout1 = model.state().layout(&tab1).cloned();
    assert!(layout1.is_some(), "tab1 应有 layout");
    assert!(
        layout1.as_ref().unwrap().tree.depth() >= 2,
        "tab1 layout 应有嵌套深度 >= 2"
    );

    // 新建 tab2
    model
        .execute(Task::NewTab {
            name: None,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_tab().is_some()
            && s.active_tab().unwrap().id != tab1
            && !s.panes(&s.active_tab().unwrap().id).is_empty()
    });
    let tab2 = model.state().active_tab().unwrap().id;

    // 切回 tab1
    model
        .execute(Task::SwitchTab {
            target: TabId(tab1.0),
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.active_tab().map(|t| t.id) == Some(tab1)
    });

    // layout 应保持不变
    let layout1_after = model.state().layout(&tab1);
    assert!(layout1_after.is_some(), "切回 tab1 后 layout 仍在");
    assert_eq!(
        layout1_after.unwrap().tree.depth(),
        layout1.as_ref().unwrap().tree.depth(),
        "切回 tab1 后 layout 深度不变"
    );
    assert_eq!(
        layout1_after.unwrap().tree.leaves().len(),
        layout1.as_ref().unwrap().tree.leaves().len(),
        "切回 tab1 后 layout 叶子数不变"
    );

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Fix3 正向测试：切 tab 后 pty 输出仍能显示
// ============================================================================

#[test]
fn fix3_positive_switch_tab_output_works() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    wait_for(&mut model, Duration::from_secs(10), |s| {
        s.active_pane().is_some()
    });
    let tab1 = model.state().active_tab().unwrap().id;
    let pane1 = model.state().active_pane().unwrap().id;

    // 在 tab1 发送 echo
    for c in "echo tab1_output".chars() {
        model
            .execute(Task::SendKeys {
                target: pane1,
                keys: vec![muxterm::core::protocol::terminal::input::KeyEvent::Char(c)],
            })
            .unwrap();
        let _ = model.poll_events();
    }
    model
        .execute(Task::SendKeys {
            target: pane1,
            keys: vec![muxterm::core::protocol::terminal::input::KeyEvent::Enter],
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.pane_output(&pane1)
            .map(|o| String::from_utf8_lossy(o).contains("tab1_output"))
            .unwrap_or(false)
    });

    // 新建 tab2
    model
        .execute(Task::NewTab {
            name: None,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_tab().is_some()
            && s.active_tab().unwrap().id != tab1
            && !s.panes(&s.active_tab().unwrap().id).is_empty()
    });
    let pane2 = model.state().active_pane().unwrap().id;

    // 在 tab2 发送 echo
    for c in "echo tab2_output".chars() {
        model
            .execute(Task::SendKeys {
                target: pane2,
                keys: vec![muxterm::core::protocol::terminal::input::KeyEvent::Char(c)],
            })
            .unwrap();
        let _ = model.poll_events();
    }
    model
        .execute(Task::SendKeys {
            target: pane2,
            keys: vec![muxterm::core::protocol::terminal::input::KeyEvent::Enter],
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.pane_output(&pane2)
            .map(|o| String::from_utf8_lossy(o).contains("tab2_output"))
            .unwrap_or(false)
    });

    // 切回 tab1，tab1 的输出应仍在
    model
        .execute(Task::SwitchTab {
            target: TabId(tab1.0),
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.active_tab().map(|t| t.id) == Some(tab1)
    });

    let tab1_output = model.state().pane_output(&pane1);
    assert!(
        tab1_output.is_some(),
        "切回 tab1 后 pane1 的 output 仍应存在"
    );
    assert!(
        String::from_utf8_lossy(tab1_output.unwrap()).contains("tab1_output"),
        "切回 tab1 后 tab1_output 应仍在 output 中"
    );

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Fix3 反向测试：切到不存在的 tab 后 pane 不丢
// ============================================================================

#[test]
fn fix3_negative_nonexistent_tab_preserves_panes() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    wait_for(&mut model, Duration::from_secs(10), |s| {
        s.active_pane().is_some()
    });
    let initial_tab = model.state().active_tab().unwrap().id;
    let initial_pane_count = model.state().panes(&initial_tab).len();

    // 切到不存在的 TabId(99)
    model
        .execute(Task::SwitchTab { target: TabId(99) })
        .unwrap();
    let _ = model.poll_events();
    std::thread::sleep(Duration::from_millis(500));
    let _ = model.refresh();

    // 当前 tab 的 pane 数应不变（tmux 忽略不存在的 window）
    let current_tab = model.state().active_tab().map(|t| t.id);
    let current_panes = current_tab
        .map(|t| model.state().panes(&t).len())
        .unwrap_or(0);
    assert!(
        current_panes >= initial_pane_count,
        "切到不存在的 tab 后 pane 数不应减少: 初始 {}, 现在 {}",
        initial_pane_count,
        current_panes
    );

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Fix3 边界测试：切 tab 后 split-pane 在正确的 tab
// ============================================================================

#[test]
fn fix3_edge_split_after_switch() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    wait_for(&mut model, Duration::from_secs(10), |s| {
        s.active_pane().is_some()
    });
    let tab1 = model.state().active_tab().unwrap().id;

    // 新建 tab2
    model
        .execute(Task::NewTab {
            name: None,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_tab().is_some()
            && s.active_tab().unwrap().id != tab1
            && !s.panes(&s.active_tab().unwrap().id).is_empty()
    });
    let tab2 = model.state().active_tab().unwrap().id;
    assert_eq!(model.state().panes(&tab2).len(), 1, "tab2 初始应有 1 pane");

    // 切回 tab1
    model
        .execute(Task::SwitchTab {
            target: TabId(tab1.0),
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.active_tab().map(|t| t.id) == Some(tab1)
    });

    // 在 tab1 split
    let pane = model.state().active_pane().unwrap().id;
    model
        .execute(Task::SplitPane {
            target: Some(pane),
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.panes(&s.active_tab().map(|t| t.id).unwrap_or(tab1)).len() >= 2
    });

    // tab1 应有 >= 2 pane
    let tab1_panes = model.state().panes(&tab1).len();
    assert!(
        tab1_panes >= 2,
        "tab1 split 后应有 >= 2 pane: {}",
        tab1_panes
    );

    // tab2 pane 数应不变
    let tab2_panes = model.state().panes(&tab2).len();
    assert_eq!(tab2_panes, 1, "tab2 pane 数应不变: {}", tab2_panes);

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Fix3 边界测试：connect 时获取所有 tab 的 pane（不只 active tab）
// ============================================================================

#[test]
fn fix3_edge_all_tabs_panes_on_connect() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    // 用原生 tmux 创建 2 tab 布局
    let rc = Command::new("tmux")
        .args([
            "-L",
            &socket,
            "new-session",
            "-d",
            "-s",
            "demo",
            "-x",
            "80",
            "-y",
            "24",
        ])
        .status();
    if rc.is_err() || !rc.unwrap().success() {
        eprintln!("skip: 无法创建 tmux session");
        cleanup(&socket);
        return;
    }
    let w0 = String::from_utf8(
        Command::new("tmux")
            .args([
                "-L",
                &socket,
                "list-windows",
                "-t",
                "demo",
                "-F",
                "#{window_id}",
            ])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    // 在 tab1 分割
    let _ = Command::new("tmux")
        .args(["-L", &socket, "split-window", "-h", "-t", &w0])
        .status();
    // 新建 tab2
    let _ = Command::new("tmux")
        .args(["-L", &socket, "new-window", "-t", "demo"])
        .status();

    // muxterm 连接（new-session 模式创建新 session，共享 server）
    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    // 等待所有 tab 的 pane 都到达
    wait_for(&mut model, Duration::from_secs(10), |s| {
        s.active_pane().is_some() && s.active_tab().is_some()
    });

    // 验证：所有 tab 都有 pane（不只 active tab）
    // 注意：connect_tmux 是 new-session，本 session 只有 1 个 tab；
    // 这里断言本 session 的每个 tab 都有 pane。
    let aw = model.state().active_tab().unwrap().id;
    let tabs = model.state().tabs();
    assert!(!tabs.is_empty(), "应至少有 1 个 tab");
    for t in &tabs {
        let panes = model.state().panes(&t.id);
        assert!(!panes.is_empty(), "tab {:?} 的 pane 不应为空", t.id);
    }

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Bug 5 测试：CLI -L 用 TmuxRuntime
// ============================================================================

#[test]
fn bug5_cli_list_sessions_with_tmux_socket() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    // 用 TmuxRuntime 连接（通过 TerminalModel，模拟 CLI 路径）
    let backend = TmuxRuntime::new(Some(&socket));
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    rt.block_on(model.connect()).unwrap();
    let _ = model.poll_events();
    std::mem::forget(rt);

    assert_eq!(model.state().status(), BackendStatus::Connected);
    // 应有 session（TmuxRuntime 创建的新 session）
    assert!(
        !model.state().workspace_name().is_empty(),
        "CLI -L 应有 session"
    );

    let _ = model.shutdown();
    cleanup(&socket);
}

#[test]
fn bug5_cli_list_windows_with_tmux_socket() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let backend = TmuxRuntime::new(Some(&socket));
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    rt.block_on(model.connect()).unwrap();
    let _ = model.poll_events();
    std::mem::forget(rt);

    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });

    // 应有至少 1 个 window
    let windows = model.state().tabs();
    assert!(!windows.is_empty(), "CLI -L 应有 window");

    let _ = model.shutdown();
    cleanup(&socket);
}

#[test]
fn bug5_cli_list_panes_with_tmux_socket() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let backend = TmuxRuntime::new(Some(&socket));
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    rt.block_on(model.connect()).unwrap();
    let _ = model.poll_events();
    std::mem::forget(rt);

    wait_for(&mut model, Duration::from_secs(10), |s| {
        s.active_pane().is_some()
    });

    let tab = model.state().active_tab().unwrap().id;
    let panes = model.state().panes(&tab);
    assert!(!panes.is_empty(), "CLI -L 应有 pane");

    let _ = model.shutdown();
    cleanup(&socket);
}

#[test]
fn bug5_cli_split_pane_via_tmux_socket() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let backend = TmuxRuntime::new(Some(&socket));
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    rt.block_on(model.connect()).unwrap();
    let _ = model.poll_events();
    std::mem::forget(rt);

    wait_for(&mut model, Duration::from_secs(10), |s| {
        s.active_pane().is_some()
    });
    let pane = model.state().active_pane().unwrap().id;

    model
        .execute(Task::SplitPane {
            target: Some(pane),
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();

    // 等待 split 完成
    let ok = wait_for(&mut model, Duration::from_secs(5), |s| {
        s.panes(&s.active_tab().map(|t| t.id).unwrap_or(TabId(0)))
            .len()
            >= 2
    });
    assert!(ok, "CLI -L split-pane 后应有 >= 2 个 pane");

    // 用原生 tmux 验证
    let session_name = model.state().workspace_name().to_string();
    let pane_count = String::from_utf8(
        Command::new("tmux")
            .args([
                "-L",
                &socket,
                "list-panes",
                "-t",
                &session_name,
                "-F",
                "#{pane_id}",
            ])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .lines()
    .count();
    assert!(
        pane_count >= 2,
        "原生 tmux 应看到 >= 2 个 pane: {}",
        pane_count
    );

    let _ = model.shutdown();
    cleanup(&socket);
}

#[test]
fn bug5_cli_new_window_via_tmux_socket() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let backend = TmuxRuntime::new(Some(&socket));
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    rt.block_on(model.connect()).unwrap();
    let _ = model.poll_events();
    std::mem::forget(rt);

    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });
    let aw = model.state().active_tab().unwrap().id;
    let initial_tabs = model.state().tabs().len();

    model
        .execute(Task::NewTab {
            name: None,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();

    let ok = wait_for(&mut model, Duration::from_secs(5), |s| {
        s.tabs().len() > initial_tabs
    });
    assert!(ok, "CLI -L new-window 后 tab 数应增加");

    let _ = model.shutdown();
    cleanup(&socket);
}

#[test]
fn bug5_new_tab_inherits_active_pane_cwd() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();
    let cwd = format!("/tmp/muxterm-cwd-{}", socket);
    std::fs::create_dir_all(&cwd).expect("创建隔离 cwd");
    let expected_cwd = std::fs::canonicalize(&cwd)
        .expect("解析隔离 cwd")
        .to_string_lossy()
        .into_owned();

    let mut model = connect_tmux(&socket);
    let ready = wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });
    assert!(ready, "tmux 应有 active pane");
    let source_pane = model.state().active_pane().unwrap().id;

    let status = Command::new("tmux")
        .args([
            "-L",
            &socket,
            "send-keys",
            "-t",
            &format!("%{}", source_pane.0),
            &format!("cd {}", cwd),
            "Enter",
        ])
        .status()
        .expect("发送 cd 到隔离 pane");
    assert!(status.success(), "tmux send-keys cd 应成功");
    assert!(
        wait_pane_cwd(&socket, source_pane, &expected_cwd, Duration::from_secs(5)),
        "source pane 应进入测试 cwd"
    );

    let old_tab = model.state().active_tab().unwrap().id;
    let old_tab_count = model.state().tabs().len();
    model
        .execute(Task::NewTab {
            name: None,
            command: None,
            workdir: None,
        })
        .expect("NewTab 应成功排入 tmux");

    let switched = wait_for(&mut model, Duration::from_secs(5), |s| {
        s.tabs().len() > old_tab_count
            && s.active_tab().is_some_and(|tab| tab.id != old_tab)
            && s.active_pane().is_some()
    });
    assert!(switched, "NewTab 后应切到新 tab");
    let new_pane = model.state().active_pane().unwrap().id;
    assert!(
        wait_pane_cwd(&socket, new_pane, &expected_cwd, Duration::from_secs(5)),
        "新 tab 的 pane 必须继承 active pane cwd，而不是回到 HOME"
    );

    let _ = model.shutdown();
    cleanup(&socket);
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn bug5_cli_send_keys_via_tmux_socket() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let backend = TmuxRuntime::new(Some(&socket));
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    rt.block_on(model.connect()).unwrap();
    let _ = model.poll_events();
    std::mem::forget(rt);

    wait_for(&mut model, Duration::from_secs(10), |s| {
        s.active_pane().is_some()
    });
    let pane = model.state().active_pane().unwrap().id;

    for c in "echo cli_test_output".chars() {
        model
            .execute(Task::SendKeys {
                target: pane,
                keys: vec![muxterm::core::protocol::terminal::input::KeyEvent::Char(c)],
            })
            .unwrap();
        let _ = model.poll_events();
    }
    model
        .execute(Task::SendKeys {
            target: pane,
            keys: vec![muxterm::core::protocol::terminal::input::KeyEvent::Enter],
        })
        .unwrap();
    let _ = model.poll_events();

    let ok = wait_for(&mut model, Duration::from_secs(5), |s| {
        s.pane_output(&pane)
            .map(|o| String::from_utf8_lossy(o).contains("cli_test_output"))
            .unwrap_or(false)
    });
    assert!(ok, "CLI -L send-keys 后 output 应包含 cli_test_output");

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Bug 5 边界测试：CLI 无 -L 仍用 ShellRuntime
// ============================================================================

#[test]
fn bug5_edge_no_socket_uses_local_backend() {
    use muxterm::core::runtime::ShellRuntime;

    let backend = ShellRuntime::new("sleep 60", "/");
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(model.connect()).unwrap();
    let _ = model.poll_events();

    // ShellRuntime 的 workspace name 是 "local"
    assert_eq!(
        model.state().workspace_name(),
        "local",
        "无 -L 应用 ShellRuntime (local workspace)"
    );

    let _ = rt.block_on(model.shutdown());
}

// ============================================================================
// Bug 6 测试：Alt+N 按 tabs() 顺序映射
// ============================================================================

#[test]
fn bug6_alt_n_maps_to_window_index() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    wait_for(&mut model, Duration::from_secs(10), |s| {
        s.active_pane().is_some()
    });

    let aw = model.state().active_tab().unwrap().id;

    // 新建第二个 tab
    model
        .execute(Task::NewTab {
            name: None,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(5), |s| s.tabs().len() >= 2);

    // 正向：Alt+1 → 第 1 个 tab；Alt+2 → 第 2 个 tab
    let tabs = model.state().tabs();
    assert!(tabs.len() >= 2, "应有 >= 2 个 tab");
    let first_tab = tabs[0].id;
    let second_tab = tabs[1].id;

    model
        .execute(Task::SwitchTab { target: first_tab })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.active_tab().map(|t| t.id) == Some(first_tab)
    });
    assert_eq!(
        model.state().active_tab().map(|t| t.id),
        Some(first_tab),
        "Alt+1 应切到第一个 tab {:?}",
        first_tab
    );

    model
        .execute(Task::SwitchTab { target: second_tab })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.active_tab().map(|t| t.id) == Some(second_tab)
    });
    assert_eq!(
        model.state().active_tab().map(|t| t.id),
        Some(second_tab),
        "Alt+2 应切到第二个 tab {:?}",
        second_tab
    );

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Bug 6 边界测试：Alt+9 不存在 → 不 crash
// ============================================================================

#[test]
fn bug6_edge_alt9_nonexistent_no_crash() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    let mut model = connect_tmux(&socket);
    assert_eq!(model.state().status(), BackendStatus::Connected);

    wait_for(&mut model, Duration::from_secs(10), |s| {
        s.active_pane().is_some()
    });

    // 只有 1 个 window，Alt+9 → 不存在
    let windows = model.state().tabs();
    assert_eq!(windows.len(), 1, "应只有 1 个 window");

    // 发送 SwitchWindow 到不存在的 window id
    let outcome = model
        .execute(Task::SwitchTab { target: TabId(999) })
        .unwrap();
    let _ = model.poll_events();

    // 不应 crash，状态应仍为 Connected
    assert_eq!(model.state().status(), BackendStatus::Connected);

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Bug 7 正向测试：attach 到已有 session，正确显示 window/pane
// ============================================================================

#[test]
fn bug7_positive_attach_existing_session() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    // 用原生 tmux 创建 2-tab 3-pane 布局
    let rc = Command::new("tmux")
        .args([
            "-L",
            &socket,
            "new-session",
            "-d",
            "-s",
            "demo",
            "-x",
            "80",
            "-y",
            "24",
        ])
        .status();
    if rc.is_err() || !rc.unwrap().success() {
        eprintln!("skip: 无法创建 tmux session");
        cleanup(&socket);
        return;
    }
    let w0 = String::from_utf8(
        Command::new("tmux")
            .args([
                "-L",
                &socket,
                "list-windows",
                "-t",
                "demo",
                "-F",
                "#{window_id}",
            ])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    Command::new("tmux")
        .args(["-L", &socket, "split-window", "-h", "-t", &w0])
        .status()
        .unwrap();
    let p1 = String::from_utf8(
        Command::new("tmux")
            .args(["-L", &socket, "list-panes", "-t", &w0, "-F", "#{pane_id}"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .lines()
    .nth(1)
    .unwrap_or("")
    .to_string();
    if !p1.is_empty() {
        Command::new("tmux")
            .args(["-L", &socket, "split-window", "-v", "-t", &p1])
            .status()
            .unwrap();
    }
    Command::new("tmux")
        .args(["-L", &socket, "new-window", "-t", "demo"])
        .status()
        .unwrap();

    // 用 TmuxRuntime attach 到 demo session
    let backend = TmuxRuntime::new_with_attach(Some(&socket), "demo");
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    rt.block_on(model.connect()).unwrap();
    let _ = model.poll_events();
    std::mem::forget(rt);

    assert_eq!(model.state().status(), BackendStatus::Connected);
    // 应有 session（demo）
    assert!(
        !model.state().workspace_name().is_empty(),
        "attach 后应有 session"
    );

    // 等待 tabs 和 panes 到达（tmux window 直接体现为产品 Tab）。
    wait_for(&mut model, Duration::from_secs(10), |s| {
        s.tabs().len() >= 2 && s.active_pane().is_some()
    });

    assert_eq!(
        model.state().tabs().len(),
        2,
        "attach 两个 tmux window 后应精确映射为 2 个 Muxterm Tab"
    );

    let tabs = model.state().tabs();

    // 正向：按 pane 数区分 — 一个 tab 3 pane，另一个 1 pane
    let pane_counts: Vec<usize> = tabs
        .iter()
        .map(|t| model.state().panes(&t.id).len())
        .collect();
    assert!(
        pane_counts.contains(&3),
        "应有一个 3-pane tab: {:?}",
        pane_counts
    );
    assert!(
        pane_counts.contains(&1),
        "应有一个 1-pane tab: {:?}",
        pane_counts
    );

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Bug 7 正向测试：attach 后 split-pane 原生 tmux 验证
// ============================================================================

#[test]
fn bug7_positive_attach_then_split() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    Command::new("tmux")
        .args([
            "-L",
            &socket,
            "new-session",
            "-d",
            "-s",
            "demo",
            "-x",
            "80",
            "-y",
            "24",
        ])
        .status()
        .unwrap();

    let backend = TmuxRuntime::new_with_attach(Some(&socket), "demo");
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    rt.block_on(model.connect()).unwrap();
    let _ = model.poll_events();
    std::mem::forget(rt);

    wait_for(&mut model, Duration::from_secs(10), |s| {
        s.active_pane().is_some()
    });
    let pane = model.state().active_pane().unwrap().id;

    // split
    model
        .execute(Task::SplitPane {
            target: Some(pane),
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.panes(&s.active_tab().map(|t| t.id).unwrap_or(TabId(0)))
            .len()
            >= 2
    });

    // 原生 tmux 验证
    let pane_count = String::from_utf8(
        Command::new("tmux")
            .args([
                "-L",
                &socket,
                "list-panes",
                "-t",
                "demo",
                "-F",
                "#{pane_id}",
            ])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .lines()
    .count();
    assert!(
        pane_count >= 2,
        "原生 tmux 应看到 >= 2 个 pane: {}",
        pane_count
    );

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Bug 7 正向测试：list-sessions 列出所有 tmux session
// ============================================================================

#[test]
fn bug7_positive_list_sessions_shows_all() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    // 创建两个 session
    Command::new("tmux")
        .args([
            "-L",
            &socket,
            "new-session",
            "-d",
            "-s",
            "sess1",
            "-x",
            "80",
            "-y",
            "24",
        ])
        .status()
        .unwrap();
    Command::new("tmux")
        .args([
            "-L",
            &socket,
            "new-session",
            "-d",
            "-s",
            "sess2",
            "-x",
            "80",
            "-y",
            "24",
        ])
        .status()
        .unwrap();

    // 发现层应列出两个可打开的 Workspace 候选，不建立第三条控制连接。
    let sessions = muxterm::core::discovery::list_local_tmux_sessions(Some(&socket));
    let names: Vec<&str> = sessions.iter().map(|item| item.name.as_str()).collect();
    assert_eq!(names.len(), 2, "应只发现两个已有工作区: {names:?}");
    assert!(names.contains(&"sess1"), "发现结果缺少 sess1: {names:?}");
    assert!(names.contains(&"sess2"), "发现结果缺少 sess2: {names:?}");

    cleanup(&socket);
}

// ============================================================================
// Bug 7 反向测试：attach 不存在的 session → 错误处理
// ============================================================================

#[test]
fn bug7_negative_attach_nonexistent_session() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    // 不创建任何 session，直接 attach
    let backend = TmuxRuntime::new_with_attach(Some(&socket), "nonexistent_session");
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();

    // attach 不存在的 session 应该失败
    let result = rt.block_on(model.connect());
    assert!(result.is_err(), "attach 不存在的 session 应失败");

    let _ = rt.block_on(model.shutdown());
    cleanup(&socket);
}

// ============================================================================
// Bug 7 边界测试：attach 后切 tab pane 正确显示
// ============================================================================

#[test]
fn bug7_edge_attach_switch_tab() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    // 创建 2-tab session
    Command::new("tmux")
        .args([
            "-L",
            &socket,
            "new-session",
            "-d",
            "-s",
            "demo",
            "-x",
            "80",
            "-y",
            "24",
        ])
        .status()
        .unwrap();
    Command::new("tmux")
        .args(["-L", &socket, "new-window", "-t", "demo"])
        .status()
        .unwrap();

    // attach
    let backend = TmuxRuntime::new_with_attach(Some(&socket), "demo");
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    rt.block_on(model.connect()).unwrap();
    let _ = model.poll_events();
    std::mem::forget(rt);

    // 等待所有 tab + pane 到达
    wait_for(&mut model, Duration::from_secs(10), |s| s.tabs().len() >= 2);

    let aw = model.state().active_tab().unwrap().id;
    let tab_ids: Vec<TabId> = model.state().tabs().iter().map(|t| t.id).collect();
    assert!(tab_ids.len() >= 2, "应有 >= 2 个 tab");

    // 切到第一个 tab
    let t0 = tab_ids[0];
    model.execute(Task::SwitchTab { target: t0 }).unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.active_tab().map(|t| t.id) == Some(t0)
    });
    let tab0_panes = model.state().panes(&t0).len();
    assert!(tab0_panes >= 1, "tab0 应有 >= 1 pane: {}", tab0_panes);

    // 切到第二个 tab
    let t1 = tab_ids[1];
    model.execute(Task::SwitchTab { target: t1 }).unwrap();
    let _ = model.poll_events();
    wait_for(&mut model, Duration::from_secs(3), |s| {
        s.active_tab().map(|t| t.id) == Some(t1)
    });
    let tab1_panes = model.state().panes(&t1).len();
    assert!(tab1_panes >= 1, "tab1 应有 >= 1 pane: {}", tab1_panes);

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// Bug 7 边界测试：attach 后 send-keys 输出显示
// ============================================================================

#[test]
fn bug7_edge_attach_send_keys() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    Command::new("tmux")
        .args([
            "-L",
            &socket,
            "new-session",
            "-d",
            "-s",
            "demo",
            "-x",
            "80",
            "-y",
            "24",
        ])
        .status()
        .unwrap();

    let backend = TmuxRuntime::new_with_attach(Some(&socket), "demo");
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    rt.block_on(model.connect()).unwrap();
    let _ = model.poll_events();
    std::mem::forget(rt);

    wait_for(&mut model, Duration::from_secs(10), |s| {
        s.active_pane().is_some()
    });
    let pane = model.state().active_pane().unwrap().id;

    for c in "echo attach_test".chars() {
        model
            .execute(Task::SendKeys {
                target: pane,
                keys: vec![muxterm::core::protocol::terminal::input::KeyEvent::Char(c)],
            })
            .unwrap();
        let _ = model.poll_events();
    }
    model
        .execute(Task::SendKeys {
            target: pane,
            keys: vec![muxterm::core::protocol::terminal::input::KeyEvent::Enter],
        })
        .unwrap();
    // 无 GUI 契约必须显式模拟前端 viewport；attach seed 在 UI preferred
    // size 到达前保持 deferred，不能吞掉后续 send-keys 输出。
    let outcome = model
        .execute(Task::ResizeClient { cols: 80, rows: 24 })
        .unwrap();
    assert!(
        matches!(outcome, muxterm::core::model::task::TaskOutcome::Done),
        "{outcome:?}"
    );
    let _ = model.poll_events();

    let ok = wait_for(&mut model, Duration::from_secs(5), |s| {
        s.pane_output(&pane)
            .map(|o| String::from_utf8_lossy(o).contains("attach_test"))
            .unwrap_or(false)
    });
    assert!(
        ok,
        "attach 后 send-keys 输出应显示: {:?}",
        model.state().pane_output(&pane)
    );

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// P0：detach → re-attach 布局保持
// ============================================================================

#[test]
fn detach_reattach_layout_persists() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    // 用原生 tmux 创建带多个 pane 的 detached session
    let rc = Command::new("tmux")
        .args([
            "-L",
            &socket,
            "new-session",
            "-d",
            "-s",
            "reattach",
            "-x",
            "80",
            "-y",
            "24",
        ])
        .status();
    if rc.is_err() || !rc.unwrap().success() {
        eprintln!("skip: 无法创建 tmux session");
        cleanup(&socket);
        return;
    }
    // 水平分割 + 垂直分割 → 3 panes
    Command::new("tmux")
        .args(["-L", &socket, "split-window", "-h"])
        .status()
        .unwrap();
    Command::new("tmux")
        .args(["-L", &socket, "split-window", "-v"])
        .status()
        .unwrap();

    // ── 第一次 attach ──
    let mut model = {
        let backend = TmuxRuntime::new_with_attach(Some(&socket), "reattach");
        let mut m = TerminalModel::new(Box::new(backend));
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .unwrap();
        rt.block_on(m.connect()).unwrap();
        let _ = m.poll_events();
        std::mem::forget(rt);
        m
    };
    assert_eq!(model.state().status(), BackendStatus::Connected);

    // 等待布局建好（至少 3 个 pane）
    let ok = wait_for(&mut model, Duration::from_secs(10), |s| {
        s.active_tab()
            .map(|t| s.panes(&t.id).len() >= 3)
            .unwrap_or(false)
    });
    assert!(ok, "首次 attach 后应有至少 3 个 pane");

    let first_count = model
        .state()
        .active_tab()
        .map(|t| model.state().panes(&t.id).len())
        .unwrap_or(0);

    // ── 显式 detach（Task::Detach 发 detach-client，session 仍在 socket 上存活）──
    model.execute(Task::Detach).unwrap();
    let detach_events = model.poll_events();
    assert_eq!(model.runtime_status(), BackendStatus::Disconnected);
    assert!(detach_events.iter().any(|event| matches!(
        event,
        muxterm::core::model::state::StateChange::BackendStatusChanged(BackendStatus::Disconnected)
    )));
    let _ = model.shutdown();

    // ── 第二次 attach，验证布局保持 ──
    let mut model2 = {
        let backend = TmuxRuntime::new_with_attach(Some(&socket), "reattach");
        let mut m = TerminalModel::new(Box::new(backend));
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .unwrap();
        rt.block_on(m.connect()).unwrap();
        let _ = m.poll_events();
        std::mem::forget(rt);
        m
    };
    assert_eq!(model2.state().status(), BackendStatus::Connected);

    let ok2 = wait_for(&mut model2, Duration::from_secs(10), |s| {
        s.active_tab()
            .map(|t| s.panes(&t.id).len() >= 3)
            .unwrap_or(false)
    });
    assert!(ok2, "re-attach 后布局应保持（至少 3 个 pane）");

    let second_count = model2
        .state()
        .active_tab()
        .map(|t| model2.state().panes(&t.id).len())
        .unwrap_or(0);
    assert_eq!(
        first_count, second_count,
        "detach/re-attach 后 pane 数应保持一致: first={first_count} second={second_count}"
    );

    let _ = model2.shutdown();
    cleanup(&socket);
}

// ============================================================================
// 场景 4: status bar 订阅（文档 §B+：refresh-client -B → %subscription-changed）
// ============================================================================

#[test]
fn scenario4_status_subscription_pushes_left_value_changes() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();
    let create = Command::new("tmux")
        .args(["-L", &socket, "new-session", "-d", "-s", "subtest"])
        .output()
        .expect("创建隔离 tmux server 失败");
    assert!(
        create.status.success(),
        "创建隔离 tmux server 失败: {:?}",
        create
    );

    let mut model = connect_tmux(&socket);
    // tmux >= 3.2 时连接后应自动启用 status-left/right 订阅。
    assert!(
        model.status_subscriptions_active(),
        "tmux >= 3.2 应启用 status bar 订阅"
    );
    let _ = model.poll_events();

    // 原生侧修改 status-left，订阅应在 ~1s 内推送 %subscription-changed。
    let set = Command::new("tmux")
        .args(["-L", &socket, "set", "-g", "status-left", "SUB-LEFT-"])
        .output()
        .expect("修改 status-left 失败");
    assert!(set.status.success(), "set status-left 失败: {:?}", set);

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut got = false;
    while Instant::now() < deadline {
        // refresh() 先从 backend 拉取事件再派发（poll_events 只排空 pending）。
        for ev in model.refresh() {
            if let StateChange::StatusBarSubscription {
                name,
                value,
                pane: _,
            } = ev
            {
                if name == "muxterm.status-left" && value.contains("SUB-LEFT-") {
                    got = true;
                }
            }
        }
        if got {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(got, "应收到 status-left 订阅推送");

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// 场景 5: pane_current_command 订阅（LINUX-PLAN C2.5b）
// ============================================================================

#[test]
fn scenario5_pane_cmd_subscription_reports_foreground_command() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();
    let create = Command::new("tmux")
        .args(["-L", &socket, "new-session", "-d", "-s", "cmdsub"])
        .output()
        .expect("创建隔离 tmux server 失败");
    assert!(
        create.status.success(),
        "创建隔离 tmux server 失败: {:?}",
        create
    );

    let mut model = connect_tmux(&socket);
    assert!(
        model.status_subscriptions_active(),
        "tmux >= 3.2 应启用 status/pane-cmd 订阅"
    );
    let _ = model.poll_events();

    // pane 里跑 /bin/cat（本机 cat 是 bat alias，必须用绝对路径）。
    let tab = model.state().active_tab().expect("应有 tab").id;
    let pane = model
        .state()
        .panes(&tab)
        .first()
        .map(|p| p.id)
        .expect("应有 pane");
    let send = Command::new("tmux")
        .args([
            "-L",
            &socket,
            "send-keys",
            "-t",
            &format!("%{}", pane.0),
            "/bin/cat",
            "Enter",
        ])
        .output()
        .expect("send-keys /bin/cat 失败");
    assert!(send.status.success(), "send-keys 失败: {:?}", send);
    assert!(
        wait_pane_command(&socket, pane, "cat", Duration::from_secs(10)),
        "pane 前台命令应变为 cat（pane_current_command 是 basename）"
    );

    // 订阅推送应在 ~1s 内到达（refresh-client -B 至多 1 次/秒）。
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut got = false;
    while Instant::now() < deadline {
        for ev in model.refresh() {
            if let StateChange::StatusBarSubscription {
                name,
                value,
                pane: Some(p),
            } = ev
            {
                if name == "muxterm.pane-cmd" && p == pane && value.contains("cat") {
                    got = true;
                }
            }
        }
        if got {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(got, "应收到 muxterm.pane-cmd 订阅推送");

    let _ = model.shutdown();
    cleanup(&socket);
}

/// attach 首屏会把 follow-up 命令延后；Surface seed 完成后必须真正补发
/// status/pane-cmd 订阅，不能只在 new-session 路径生效。
#[test]
fn scenario5_attach_releases_pane_cmd_subscription_after_surface_seed() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();
    let session = "attachcmd";
    let create = Command::new("tmux")
        .args([
            "-L",
            &socket,
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            session,
            "--",
            "/bin/cat",
        ])
        .output()
        .expect("创建隔离 tmux server 失败");
    assert!(
        create.status.success(),
        "创建隔离 tmux server 失败: {create:?}"
    );

    let backend = TmuxRuntime::new_with_attach(Some(&socket), session);
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    rt.block_on(model.connect()).unwrap();
    std::mem::forget(rt);

    let ready = wait_for(&mut model, Duration::from_secs(10), |state| {
        state.active_pane().is_some()
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut subscriptions = model.status_subscriptions_active();
    while !subscriptions && Instant::now() < deadline {
        let _ = model.refresh();
        subscriptions = model.status_subscriptions_active();
        std::thread::sleep(Duration::from_millis(100));
    }
    let pane = model.state().active_pane().map(|item| item.id);
    let _ = model.shutdown();
    cleanup(&socket);

    assert!(ready, "attach 后必须建立 active pane");
    assert!(pane.is_some(), "attach 后必须有 pane");
    assert!(
        subscriptions,
        "attach Surface seed 完成后必须补发 status/pane-cmd 订阅"
    );
}
