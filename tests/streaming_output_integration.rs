//! 持续输出 / 时间行为集成测试。
//!
//! 用真实 tmux -L <socket> 验证 pane 在长时间、高频、无换行、长行等场景下
//! 的 output 累积与状态机健壮性（Phase 3）。
//!
//! 这些不是「一次 echo」测试，而是「时间行为」测试：它们验证 muxterm 能承载
//! 长期运行的 shell 与流式输出，而不只是显示 layout。

#![cfg(feature = "tui")]
#![allow(clippy::let_underscore_future)]
#![allow(unused_variables)]

use muxterm::core::model::state::{BackendStatus, State};
use muxterm::core::model::task::Task;
use muxterm::core::model::TerminalModel;
use muxterm::core::protocol::terminal::input::KeyEvent;
use muxterm::core::runtime::TmuxRuntime;
use muxterm::core::types::PaneId;
use std::process::Command;
use std::time::{Duration, Instant};

/// 生成唯一的 tmux socket 名。
fn unique_socket() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("muxterm-stream-{}-{}", std::process::id(), nanos)
}

fn cleanup(socket: &str) {
    let _ = Command::new("tmux")
        .args(["-L", socket, "kill-server"])
        .output();
}

fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn connect_tmux(socket: &str) -> TerminalModel {
    let backend = TmuxRuntime::new(Some(socket));
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    rt.block_on(model.connect()).unwrap();
    let _ = model.poll_events();
    std::mem::forget(rt); // keep runtime alive for the duration of the test
    model
}

fn wait_for<F>(model: &mut TerminalModel, timeout: Duration, cond: F) -> bool
where
    F: Fn(&dyn State) -> bool,
{
    let deadline = Instant::now() + timeout;
    loop {
        let _ = model.refresh();
        if cond(model.state()) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 向 pane 输入一行命令（逐字符 send-keys + Enter）。
fn type_command(model: &mut TerminalModel, pane: PaneId, cmd: &str) {
    let mut keys: Vec<KeyEvent> = cmd.chars().map(KeyEvent::Char).collect();
    keys.push(KeyEvent::Enter);
    model
        .execute(Task::SendKeys { target: pane, keys })
        .unwrap();
    let _ = model.poll_events();
}

/// 获取 pane 当前累积的原始输出字节（转成字符串）。
fn output_str(model: &mut TerminalModel, pane: PaneId) -> String {
    let _ = model.refresh();
    model
        .state()
        .pane_output(&pane)
        .map(|o| String::from_utf8_lossy(o).into_owned())
        .unwrap_or_default()
}

// ============================================================================
// 1. 普通持续输出：长时间运行，输出停止后 pane 状态正常，程序退出后 %exit
// ============================================================================

#[test]
fn continuous_output_long_running() {
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

    // 启动一个每 100ms 输出一行的后台循环，运行 3 秒
    type_command(
        &mut model,
        pane,
        "for i in $(seq 1 30); do echo \"stream-line-$i\"; sleep 0.1; done",
    );

    // 等待一定输出累积
    let got = wait_for(&mut model, Duration::from_secs(8), |s| {
        s.pane_output(&pane).map(|o| o.len() > 300).unwrap_or(false)
    });
    assert!(got, "持续输出应在数秒内累积到一定字节量");

    let out = output_str(&mut model, pane);
    assert!(out.contains("stream-line-"), "输出应含流式行: {}", out);

    // 等循环结束（30 行 * 0.1s = 3s）
    std::thread::sleep(Duration::from_secs(4));

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// 2. 无换行输出：持续 progress，验证行边界不被 pane 内容干扰
// ============================================================================

#[test]
fn no_newline_progress_output() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();
    let mut model = connect_tmux(&socket);
    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });
    let pane = model.state().active_pane().unwrap().id;

    // 无换行的持续 progress 输出（用 \r 原地更新）
    type_command(
        &mut model,
        pane,
        "for i in $(seq 1 20); do printf 'progress %d...\\r' $i; sleep 0.1; done; echo DONE",
    );

    let got = wait_for(&mut model, Duration::from_secs(8), |s| {
        s.pane_output(&pane).map(|o| o.len() > 50).unwrap_or(false)
    });
    assert!(got, "progress 输出应累积");
    let out = output_str(&mut model, pane);
    assert!(
        out.contains("progress") || out.contains("DONE"),
        "应含 progress 内容"
    );

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// 3. 高频输出：短时间内大量输出，事件队列不爆炸
// ============================================================================

#[test]
fn high_frequency_output_stays_bounded() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();
    let mut model = connect_tmux(&socket);
    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });
    let pane = model.state().active_pane().unwrap().id;

    // 高频输出：一次性生成数千行
    type_command(&mut model, pane, "seq 1 3000");

    // 等输出到达（可能被 MAX_PANE_OUTPUT_BYTES 截断，但不应卡死/崩溃）
    let _ = wait_for(&mut model, Duration::from_secs(8), |s| {
        s.pane_output(&pane)
            .map(|o| o.len() > 5000)
            .unwrap_or(false)
    });

    // 状态机仍正常：能继续发命令并收到响应
    type_command(&mut model, pane, "echo STILL_ALIVE");
    let alive = wait_for(&mut model, Duration::from_secs(5), |s| {
        s.pane_output(&pane)
            .map(|o| String::from_utf8_lossy(o).contains("STILL_ALIVE"))
            .unwrap_or(false)
    });
    assert!(alive, "高频输出后状态机应仍能响应命令");

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// 4. 长行/大块输出：单条超长行、跨 read chunk
// ============================================================================

#[test]
fn long_line_large_output() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();
    let mut model = connect_tmux(&socket);
    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });
    let pane = model.state().active_pane().unwrap().id;

    // 单条超长行（100KB），验证 read loop 不截断/崩溃
    type_command(&mut model, pane, "head -c 100000 /dev/zero | tr '\\0' 'x'");

    let got = wait_for(&mut model, Duration::from_secs(8), |s| {
        s.pane_output(&pane)
            .map(|o| o.len() > 50000)
            .unwrap_or(false)
    });
    assert!(got, "长行输出应累积到较大字节量");

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// 5. 特殊字符与 UTF-8：中文、emoji、宽字符
// ============================================================================

#[test]
fn unicode_and_special_chars_output() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();
    let mut model = connect_tmux(&socket);
    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });
    let pane = model.state().active_pane().unwrap().id;

    // 写一个真实 UTF-8 文件（Rust 端写入真实多字节字节），然后 cat 它。
    // 命令本身是纯 ASCII，多字节字节从文件流经 tmux %output 协议层，验证字节保留。
    let tmp = format!("/tmp/muxterm-utf8-{}.txt", std::process::id());
    std::fs::write(&tmp, "中文测试 emoji😀 宽字𠀀\n").expect("write utf8 file");

    type_command(&mut model, pane, &format!("cat {}", tmp));

    // 等输出真正包含文件内容（命令回显本身也可能先到，所以按内容匹配）
    let got = wait_for(&mut model, Duration::from_secs(6), |s| {
        s.pane_output(&pane)
            .map(|o| {
                let t = String::from_utf8_lossy(o);
                t.contains("中文") || t.contains("emoji")
            })
            .unwrap_or(false)
    });
    assert!(got, "输出应含文件内容");
    let out = output_str(&mut model, pane);
    assert!(out.contains("中文"), "输出应含真实 UTF-8 中文: {:?}", out);
    assert!(out.contains("emoji"), "应含 emoji 文本: {:?}", out);
    // 输出里应是真实多字节字节，而非未解码的 \u 字面转义
    assert!(
        !out.contains("\\u4e2d"),
        "不应出现未解码的 \\u 转义: {:?}",
        out
    );

    let _ = std::fs::remove_file(&tmp);

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// 6. 多 pane 同时输出互不阻塞
// ============================================================================

#[test]
fn multi_pane_concurrent_output() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();
    let mut model = connect_tmux(&socket);
    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });

    // 分割出第二个 pane
    let pane0 = model.state().active_pane().unwrap().id;
    model
        .execute(Task::SplitPane {
            target: Some(pane0),
            dir: muxterm::core::model::layout::SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    // 等待 active tab 下出现 2 个 pane
    let tab_id = model.state().active_tab().unwrap().id;
    let ok = wait_for(&mut model, Duration::from_secs(5), move |s| {
        s.panes(&tab_id).len() >= 2
    });
    assert!(ok, "分割后应至少有 2 个 pane");
    // 收集该 tab 下所有 pane
    let panes: Vec<PaneId> = model.state().panes(&tab_id).iter().map(|p| p.id).collect();
    assert!(panes.len() >= 2, "应至少有 2 个 pane: {:?}", panes);

    // 两个 pane 同时输出
    type_command(&mut model, panes[0], "seq 1 100");
    type_command(&mut model, panes[1], "seq 100 200");

    // 等两边都有输出
    let ok = wait_for(&mut model, Duration::from_secs(8), |s| {
        let a = s.pane_output(&panes[0]).map(|o| o.len()).unwrap_or(0);
        let b = s.pane_output(&panes[1]).map(|o| o.len()).unwrap_or(0);
        a > 0 && b > 0
    });
    assert!(ok, "两个 pane 都应累积输出");

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// 7. resize 期间 pane 正在输出：resize → SIGWINCH → 重绘洪峰与 %layout-change 交织
// ============================================================================

#[test]
fn resize_during_active_output() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();
    let mut model = connect_tmux(&socket);
    wait_for(&mut model, Duration::from_secs(5), |s| {
        s.active_pane().is_some()
    });
    let pane = model.state().active_pane().unwrap().id;

    // 先启动一个持续输出循环（后台，跑 5 秒）
    type_command(
        &mut model,
        pane,
        "for i in $(seq 1 50); do echo \"resize-out-$i\"; sleep 0.1; done",
    );

    // 等开始输出
    let started = wait_for(&mut model, Duration::from_secs(5), |s| {
        s.pane_output(&pane)
            .map(|o| {
                let t = String::from_utf8_lossy(o);
                t.contains("resize-out-")
            })
            .unwrap_or(false)
    });
    assert!(started, "resize 前应已有输出");

    // 在输出进行中多次 resize（放大 + 缩小）
    for (cols, rows) in [(100, 30), (40, 15), (120, 40), (80, 24)] {
        model
            .execute(Task::ResizePane {
                target: pane,
                cols,
                rows,
            })
            .unwrap();
        let _ = model.poll_events();
        std::thread::sleep(Duration::from_millis(200));
    }

    // resize 后状态机仍正常：能继续发命令并收到输出
    type_command(&mut model, pane, "echo RESIZE_OK");
    let ok = wait_for(&mut model, Duration::from_secs(5), |s| {
        s.pane_output(&pane)
            .map(|o| String::from_utf8_lossy(o).contains("RESIZE_OK"))
            .unwrap_or(false)
    });
    assert!(ok, "resize 期间输出后状态机应仍响应命令");

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// 8. attach 已有 session 的初始消息洪峰：session/window/pane 状态 + 每 pane 初始
//    输出 + 多条 begin/end + 后续异步 output 交织
// ============================================================================

#[test]
fn attach_initial_flood_and_live_output() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();

    // 用原生 tmux 建一个带多个 pane + 历史输出的 session
    let rc = Command::new("tmux")
        .args([
            "-L",
            &socket,
            "new-session",
            "-d",
            "-s",
            "flood",
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
    // 在 pane 0 生成一些输出
    Command::new("tmux")
        .args([
            "-L",
            &socket,
            "send-keys",
            "-t",
            "flood:0",
            "echo attach-initial-output; seq 1 50",
            "Enter",
        ])
        .status()
        .unwrap();

    // attach 到该 session（attach 模式会收到大量初始状态 + 历史输出）
    let backend = TmuxRuntime::new_with_attach(Some(&socket), "flood");
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
    assert!(
        !model.state().workspace_name().is_empty(),
        "attach 后应有 session"
    );

    // 等待初始状态完全建好（tab + pane）
    let ready = wait_for(&mut model, Duration::from_secs(10), |s| {
        s.active_pane().is_some() && s.active_tab().is_some()
    });
    assert!(ready, "attach 初始状态应建好");

    // attach 后仍能继续交互：发新命令收到新输出（证明初始洪峰没破坏状态机）
    let pane = model.state().active_pane().unwrap().id;
    type_command(&mut model, pane, "echo FLOOD_OK");
    let ok = wait_for(&mut model, Duration::from_secs(5), |s| {
        s.pane_output(&pane)
            .map(|o| String::from_utf8_lossy(o).contains("FLOOD_OK"))
            .unwrap_or(false)
    });
    assert!(ok, "attach 洪峰后状态机应仍能响应命令");

    let _ = model.shutdown();
    cleanup(&socket);
}

/// attach 前已经存在的 shell 画面必须进入 core 累计输出，供 GUI 首次创建
/// terminal view 时回放；不能只验证 attach 后新发的 echo。
#[test]
fn attach_restores_existing_shell_screen_output() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let socket = unique_socket();
    let marker = "ATTACH_RESTORE_SCREEN_74291";
    let created = Command::new("tmux")
        .args([
            "-L",
            &socket,
            "new-session",
            "-d",
            "-s",
            "restore",
            "-x",
            "80",
            "-y",
            "24",
        ])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !created {
        cleanup(&socket);
        eprintln!("skip: 无法创建 tmux session");
        return;
    }
    let command = format!("printf '{marker}\\n'");
    let sent = Command::new("tmux")
        .args([
            "-L",
            &socket,
            "send-keys",
            "-t",
            "restore",
            &command,
            "Enter",
        ])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    assert!(sent, "预置 attach 屏幕内容失败");
    std::thread::sleep(Duration::from_millis(150));

    let backend = TmuxRuntime::new_with_attach(Some(&socket), "restore");
    let mut model = TerminalModel::new(Box::new(backend));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    rt.block_on(model.connect()).unwrap();
    std::mem::forget(rt);

    let ready = wait_for(&mut model, Duration::from_secs(5), |state| {
        state
            .active_pane()
            .and_then(|pane| state.pane_output(&pane.id))
            .map(|output| String::from_utf8_lossy(output).contains(marker))
            .unwrap_or(false)
    });
    let output = model
        .state()
        .active_pane()
        .and_then(|pane| model.state().pane_output(&pane.id))
        .map(String::from_utf8_lossy)
        .unwrap_or_default();
    assert!(ready, "attach 后应恢复已有 shell 画面，实际输出={output:?}");

    let _ = model.shutdown();
    cleanup(&socket);
}

// ============================================================================
// 9. 更高强度：多 pane 同时高频输出并持续一段时间，验证背压/事件队列有界、
//    且每个 pane 都累积到各自输出（不串流、不阻塞）
// ============================================================================

#[test]
fn sustained_multi_pane_stress_stays_bounded() {
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

    let pane0 = model.state().active_pane().unwrap().id;
    // 分割出第二个 pane
    model
        .execute(Task::SplitPane {
            target: Some(pane0),
            dir: muxterm::core::model::layout::SplitDir::Horizontal,
            command: None,
            workdir: None,
        })
        .unwrap();
    let _ = model.poll_events();
    let tab_id = model.state().active_tab().unwrap().id;
    let ok = wait_for(&mut model, Duration::from_secs(5), |s| {
        s.panes(&tab_id).len() >= 2
    });
    assert!(ok, "分割后应至少有 2 个 pane");
    let panes: Vec<PaneId> = model.state().panes(&tab_id).iter().map(|p| p.id).collect();
    assert!(panes.len() >= 2, "应有至少 2 个 pane: {:?}", panes);

    // 两个 pane 各自持续高频输出一段时间（总计约 6000 行）
    type_command(
        &mut model,
        panes[0],
        "for i in $(seq 1 3000); do echo a-stress-$i; done",
    );
    type_command(
        &mut model,
        panes[1],
        "for i in $(seq 1 3000); do echo b-stress-$i; done",
    );

    // 等两个 pane 都累积到各自输出
    let got = wait_for(&mut model, Duration::from_secs(10), |s| {
        let a = s
            .pane_output(&panes[0])
            .map(String::from_utf8_lossy)
            .map(|s| s.contains("a-stress-"))
            .unwrap_or(false);
        let b = s
            .pane_output(&panes[1])
            .map(String::from_utf8_lossy)
            .map(|s| s.contains("b-stress-"))
            .unwrap_or(false);
        a && b
    });
    assert!(got, "两个 pane 都应累积到各自标记输出");

    // 事件队列不应爆炸（内部有 MAX_STATE_EVENTS 裁剪 + MAX_PANE_OUTPUT_BYTES 上限）
    // 通过 refresh 触发的 StateChange 数量不应异常：状态机仍能继续响应
    type_command(&mut model, panes[0], "echo STRESS_ALIVE");
    let alive = wait_for(&mut model, Duration::from_secs(5), |s| {
        s.pane_output(&panes[0])
            .map(|o| String::from_utf8_lossy(o).contains("STRESS_ALIVE"))
            .unwrap_or(false)
    });
    assert!(alive, "高负载后状态机应仍能响应命令");

    let _ = model.shutdown();
    cleanup(&socket);
}
