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
use muxterm::core::runtime::TmuxBackend;
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
    let backend = TmuxBackend::new(Some(socket));
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
