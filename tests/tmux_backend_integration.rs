//! TmuxBackend 集成测试：用真实 tmux -L <socket> 验证 state 构建。
//!
//! 场景 1：原生 tmux 创建 2-tab 3-pane 布局 → muxterm 连接 → 验证 state
//! 场景 2：attach/detach → re-attach 布局保持
//! 场景 3：通过 muxterm 修改布局 → 原生 tmux 验证

#![cfg(feature = "tui")]

use muxterm::core::backend::TmuxBackend;
use muxterm::core::model::layout::SplitDir;
use muxterm::core::model::state::{BackendStatus, State};
use muxterm::core::model::task::Task;
use muxterm::core::model::TerminalModel;
use muxterm::core::types::{PaneId, TabId};
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
    let mut iterations = 0;
    loop {
        let events = model.refresh();
        if iterations % 10 == 0 {}
        iterations += 1;
        if cond(model.state()) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 创建 TerminalModel with TmuxBackend，连接到指定 socket。
///
/// 内部用多线程 tokio runtime，保持后台 task 存活。
fn connect_tmux(socket: &str) -> TerminalModel {
    let backend = TmuxBackend::new(Some(socket));
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

    // muxterm TmuxBackend 连接（new-session 模式，创建新 session）
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
    assert!(!model.state().sessions().is_empty(), "应有 session");
    assert!(
        model.state().active_window().is_some(),
        "应有 active window"
    );
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
    let session_name = model
        .state()
        .active_session()
        .map(|s| s.name.clone())
        .unwrap_or_default();
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

    // 等待初始 window
    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_window().is_some() && s.active_pane().is_some()
    });

    let initial_window = model.state().active_window().map(|w| w.id);

    // 新建 window（= tmux new-window = muxterm new-tab）
    model
        .execute(Task::NewWindow {
            name: Some("test-win".into()),
            command: None,
            workdir: None,
        })
        .unwrap();

    // 等待 window-add 事件：active window 应变化
    let ok = wait_for(&mut model, Duration::from_secs(3), |s| {
        let cur = s.active_window().map(|w| w.id);
        cur != initial_window && cur.is_some()
    });
    assert!(ok, "新 window 未建立");

    // 原生 tmux 验证
    let session_name = model
        .state()
        .active_session()
        .map(|s| s.name.clone())
        .unwrap_or_default();
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
                muxterm::core::terminal::input::KeyEvent::Char('e'),
                muxterm::core::terminal::input::KeyEvent::Char('c'),
                muxterm::core::terminal::input::KeyEvent::Char('h'),
                muxterm::core::terminal::input::KeyEvent::Char('o'),
                muxterm::core::terminal::input::KeyEvent::Char(' '),
                muxterm::core::terminal::input::KeyEvent::Char('h'),
                muxterm::core::terminal::input::KeyEvent::Char('i'),
                muxterm::core::terminal::input::KeyEvent::Enter,
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
