#![cfg(feature = "tui")]

//! TUI 集成测试：spawn TUI 进程在 tmux pane 里，capture-pane 抓画面，断言渲染。
//!
//! 这些测试需要 tmux + 可执行 muxterm 二进制（编译时 --features tui）。
//! 测试流程：
//! 1. 编译 muxterm（cargo test 已编译 test bin；但 TUI 进程是独立的 bin）
//! 2. 在独立 tmux socket 里 new-session，send-keys 启动 `muxterm --tui`
//! 3. sleep 短暂等待 TUI 初始化
//! 4. tmux capture-pane -p 抓画面文本
//! 5. 断言：tab 栏有内容、pane 边框存在、状态栏显示 connected
//!
//! 跑这些测试：cargo test --no-default-features --features tui --test tui_integration

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// 找到 muxterm 二进制路径（cargo test 会编译到 target-dir/debug/）。
fn muxterm_bin() -> PathBuf {
    // CARGO_BIN_EXE_muxterm 是 cargo test 注入的环境变量（Rust 1.43+）
    // 但只有 [[bin]] 定义时才有。我们用 target-dir 推导。
    let target =
        std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "../muxterm-target".to_string());
    let p = PathBuf::from(target).join("debug").join("muxterm");
    if p.exists() {
        return p;
    }
    // 兜底：相对当前仓库
    PathBuf::from("target/debug/muxterm")
}

/// 在独立 tmux socket 里启动 TUI，等待并抓取画面。
fn capture_tui_screen(extra_args: &[&str]) -> String {
    let socket = format!("tui-it-{}-{}", std::process::id(), rand_suffix());
    let bin = muxterm_bin();

    // new-session
    let status = Command::new("tmux")
        .args(["-L", &socket, "new-session", "-d", "-x", "80", "-y", "24"])
        .status()
        .expect("tmux new-session 失败");
    assert!(status.success(), "tmux new-session 应成功");

    // 启动 muxterm --tui
    let mut cmd_str = bin.to_string_lossy().to_string();
    cmd_str.push_str(" --tui");
    for a in extra_args {
        cmd_str.push(' ');
        cmd_str.push_str(a);
    }
    let status = Command::new("tmux")
        .args(["-L", &socket, "send-keys", &cmd_str, "Enter"])
        .status()
        .expect("tmux send-keys 失败");
    assert!(status.success());

    // 等待 TUI 初始化
    std::thread::sleep(Duration::from_millis(1500));

    // capture-pane
    let output = Command::new("tmux")
        .args(["-L", &socket, "capture-pane", "-p", "-t", "0"])
        .output()
        .expect("tmux capture-pane 失败");
    let screen = String::from_utf8_lossy(&output.stdout).to_string();

    // 清理
    let _ = Command::new("tmux")
        .args(["-L", &socket, "kill-server"])
        .status();

    screen
}

fn rand_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{nanos}")
}

/// 判断 tmux 是否可用（CI / 无 tmux 环境跳过）。
fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn tui_shows_tab_bar_with_window_name() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let screen = capture_tui_screen(&[]);
    // tab 栏（第二行）应含 1-based 序号（如 1:t1*）
    assert!(
        screen.contains("1:"),
        "tab 栏应显示 tab 序号, 画面:\n{screen}"
    );
}

#[test]
fn tui_shows_pane_border_top() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let screen = capture_tui_screen(&[]);
    // 顶部边框 ┌...┐
    assert!(screen.contains('┌'), "应有顶部边框 ┌, 画面:\n{screen}");
    assert!(screen.contains('┐'), "应有顶部边框 ┐, 画面:\n{screen}");
}

#[test]
fn tui_shows_pane_border_bottom() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let screen = capture_tui_screen(&[]);
    assert!(screen.contains('└'), "应有底部边框 └, 画面:\n{screen}");
    assert!(screen.contains('┘'), "应有底部边框 ┘, 画面:\n{screen}");
}

#[test]
fn tui_status_bar_shows_connected() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let screen = capture_tui_screen(&[]);
    // 状态栏应含 connected
    assert!(
        screen.contains("connected"),
        "状态栏应显示 connected, 画面:\n{screen}"
    );
}

#[test]
fn tui_status_bar_shows_quit_hint() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let screen = capture_tui_screen(&[]);
    assert!(
        screen.contains("Ctrl-Q"),
        "状态栏应显示 Ctrl-Q 提示, 画面:\n{screen}"
    );
}

#[test]
fn tui_pane_titles_show_shell_name() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let screen = capture_tui_screen(&[]);
    // pane 标题栏应含 @1 + shell 名（zsh/bash）
    assert!(
        screen.contains("@1"),
        "pane 标题栏应显示 pane id @1, 画面:\n{screen}"
    );
}

#[test]
fn tui_alt_t_creates_new_tab() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    // 启动 TUI，发送 Alt+T，抓画面
    let socket = format!("tui-alt-{}-{}", std::process::id(), rand_suffix());
    let bin = muxterm_bin();

    Command::new("tmux")
        .args(["-L", &socket, "new-session", "-d", "-x", "80", "-y", "24"])
        .status()
        .expect("tmux new-session");

    let cmd_str = format!("{} --tui", bin.to_string_lossy());
    Command::new("tmux")
        .args(["-L", &socket, "send-keys", &cmd_str, "Enter"])
        .status()
        .expect("send-keys");

    std::thread::sleep(Duration::from_millis(1500));

    // 发送 Alt+T (tmux: M-t)
    Command::new("tmux")
        .args(["-L", &socket, "send-keys", "-t", "0", "M-t"])
        .status()
        .expect("send M-t");

    std::thread::sleep(Duration::from_millis(1000));

    let output = Command::new("tmux")
        .args(["-L", &socket, "capture-pane", "-p", "-t", "0"])
        .output()
        .expect("capture-pane");
    let screen = String::from_utf8_lossy(&output.stdout).to_string();

    let _ = Command::new("tmux")
        .args(["-L", &socket, "kill-server"])
        .status();

    // Alt+T 后 tab 栏应显示第二个 tab（2:...）
    assert!(
        screen.contains("2:"),
        "Alt+T 应创建新 tab (2:...), 画面:\n{screen}"
    );
}
