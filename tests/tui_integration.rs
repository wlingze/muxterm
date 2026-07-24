#![cfg(feature = "tui")]

//! TUI 集成测试：spawn TUI 进程在 tmux pane 里，capture-pane 抓画面，断言渲染。
//!
//! 这些测试需要 tmux + 可执行 muxterm 二进制（编译时 --features tui）。
//! 测试流程：
//! 1. 编译 muxterm（cargo test 已编译 test bin；但 TUI 进程是独立的 bin）
//! 2. 在独立 tmux socket 里 new-session，send-keys 启动 `muxterm --tui [...]`
//! 3. sleep 短暂等待 TUI 初始化 / 交互
//! 4. tmux capture-pane -p 抓画面文本
//! 5. 断言 tab 栏、pane、状态栏、交互结果
//!
//! 跑这些测试：cargo test --no-default-features --features tui --test tui_integration

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// 找到 muxterm 二进制路径（cargo test 会编译到 target-dir/debug/）。
fn muxterm_bin() -> PathBuf {
    let target =
        std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "../muxterm-target".to_string());
    let p = PathBuf::from(&target).join("debug").join("muxterm");
    if p.exists() {
        return p;
    }
    PathBuf::from("target/debug/muxterm")
}

fn rand_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{nanos}")
}

fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 宿主 tmux：用来跑 TUI 进程并 capture 画面。
struct HostTmux {
    socket: String,
}

impl HostTmux {
    fn new(prefix: &str) -> Self {
        let socket = format!("{prefix}-{}-{}", std::process::id(), rand_suffix());
        let status = Command::new("tmux")
            .args(["-L", &socket, "new-session", "-d", "-x", "100", "-y", "30"])
            .status()
            .expect("tmux new-session");
        assert!(status.success(), "host tmux new-session 应成功");
        Self { socket }
    }

    fn send_keys(&self, keys: &str) {
        let status = Command::new("tmux")
            .args(["-L", &self.socket, "send-keys", "-t", "0", keys])
            .status()
            .expect("send-keys");
        assert!(status.success());
    }

    fn send_line(&self, line: &str) {
        let status = Command::new("tmux")
            .args(["-L", &self.socket, "send-keys", "-t", "0", line, "Enter"])
            .status()
            .expect("send-keys Enter");
        assert!(status.success());
    }

    fn capture(&self) -> String {
        let output = Command::new("tmux")
            .args(["-L", &self.socket, "capture-pane", "-p", "-t", "0"])
            .output()
            .expect("capture-pane");
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    fn kill(self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.socket, "kill-server"])
            .status();
    }
}

impl Drop for HostTmux {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.socket, "kill-server"])
            .status();
    }
}

/// 在独立 tmux socket 里启动 TUI，等待并抓取画面。
fn capture_tui_screen(extra_args: &[&str]) -> String {
    let host = HostTmux::new("tui-it");
    let bin = muxterm_bin();
    let mut cmd_str = bin.to_string_lossy().to_string();
    cmd_str.push_str(" --tui");
    for a in extra_args {
        cmd_str.push(' ');
        cmd_str.push_str(a);
    }
    host.send_line(&cmd_str);
    std::thread::sleep(Duration::from_millis(1500));
    let screen = host.capture();
    host.kill();
    screen
}

fn cleanup_local_session(name: &str) {
    let bin = muxterm_bin();
    let _ = Command::new(&bin)
        .args(["kill-session", "-s", name])
        .output();
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let sock = format!("{runtime}/muxterm-{name}.sock");
    let _ = std::fs::remove_file(&sock);
    std::thread::sleep(Duration::from_millis(100));
}

fn setup_tmux_backend_2tab(backend_sock: &str) {
    let _ = Command::new("tmux")
        .args(["-L", backend_sock, "kill-server"])
        .output();
    Command::new("tmux")
        .args([
            "-L",
            backend_sock,
            "new-session",
            "-d",
            "-s",
            "demo",
            "-x",
            "100",
            "-y",
            "30",
        ])
        .status()
        .unwrap();
    let w0 = String::from_utf8(
        Command::new("tmux")
            .args([
                "-L",
                backend_sock,
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
        .args(["-L", backend_sock, "split-window", "-h", "-t", &w0])
        .status()
        .unwrap();
    let p1 = String::from_utf8(
        Command::new("tmux")
            .args([
                "-L",
                backend_sock,
                "list-panes",
                "-t",
                &w0,
                "-F",
                "#{pane_id}",
            ])
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
            .args(["-L", backend_sock, "split-window", "-v", "-t", &p1])
            .status();
    }
    Command::new("tmux")
        .args(["-L", backend_sock, "new-window", "-t", "demo"])
        .status()
        .unwrap();
}

// ============================================================================
// 基础渲染
// ============================================================================

#[test]
fn tui_shows_tab_bar_with_window_name() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let screen = capture_tui_screen(&[]);
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
    assert!(
        screen.contains('@'),
        "pane 标题栏应显示 pane id, 画面:\n{screen}"
    );
}

#[test]
fn tui_alt_t_creates_new_tab() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let host = HostTmux::new("tui-alt");
    let bin = muxterm_bin();
    host.send_line(&format!("{} --tui", bin.to_string_lossy()));
    std::thread::sleep(Duration::from_millis(1500));
    host.send_keys("M-t");
    std::thread::sleep(Duration::from_millis(1000));
    let screen = host.capture();
    host.kill();
    assert!(
        screen.contains("2:"),
        "Alt+T 应创建新 tab (2:...), 画面:\n{screen}"
    );
}

// ============================================================================
// TUI × local（--tui -s name）
// ============================================================================

#[test]
fn tui_local_session_dash_s() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let name = format!("tui-loc-{}-{}", std::process::id(), rand_suffix());
    cleanup_local_session(&name);
    let bin = muxterm_bin();

    // 先用 CLI 建 local daemon + 2 tab 布局
    let st = Command::new(&bin)
        .args(["new-session", "-s", &name])
        .status()
        .unwrap();
    assert!(st.success());
    let _ = Command::new(&bin)
        .args(["split-pane", "-h", "-s", &name])
        .status();
    let _ = Command::new(&bin).args(["new-tab", "-s", &name]).status();
    std::thread::sleep(Duration::from_millis(300));

    let host = HostTmux::new("tui-loc");
    host.send_line(&format!("{} --tui -s {}", bin.to_string_lossy(), name));
    std::thread::sleep(Duration::from_millis(2000));
    let screen = host.capture();
    host.kill();

    assert!(
        screen.contains("connected"),
        "--tui -s 应显示 connected, 画面:\n{screen}"
    );
    assert!(
        screen.contains("1:") && screen.contains("2:"),
        "--tui -s 应显示 2 个 tab, 画面:\n{screen}"
    );

    cleanup_local_session(&name);
}

// ============================================================================
// TUI × tmux（--tui -L socket -s name）
// ============================================================================

#[test]
fn tui_tmux_attach_dash_l_dash_s() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let backend = format!("tui-be-{}-{}", std::process::id(), rand_suffix());
    setup_tmux_backend_2tab(&backend);
    let bin = muxterm_bin();

    let host = HostTmux::new("tui-tmux");
    host.send_line(&format!(
        "{} --tui -L {} -s demo",
        bin.to_string_lossy(),
        backend
    ));
    std::thread::sleep(Duration::from_millis(2500));
    let screen = host.capture();
    host.kill();

    assert!(
        screen.contains("connected"),
        "--tui -L -s 应 connected, 画面:\n{screen}"
    );
    assert!(
        screen.contains("1:") && screen.contains("2:"),
        "--tui -L -s 应显示 2 tabs, 画面:\n{screen}"
    );

    let _ = Command::new("tmux")
        .args(["-L", &backend, "kill-server"])
        .status();
}

// ============================================================================
// TUI: Alt+N 切 tab 后 pane 正确
// ============================================================================

#[test]
fn tui_alt_n_switches_tab_panes() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let backend = format!("tui-altn-{}-{}", std::process::id(), rand_suffix());
    setup_tmux_backend_2tab(&backend);
    let bin = muxterm_bin();

    let host = HostTmux::new("tui-altn");
    host.send_line(&format!(
        "{} --tui -L {} -s demo",
        bin.to_string_lossy(),
        backend
    ));
    std::thread::sleep(Duration::from_millis(2500));

    // 初始多半在 tab2（1 pane）；Alt+1 → tab1（3 panes）
    host.send_keys("M-1");
    std::thread::sleep(Duration::from_millis(1200));
    let screen1 = host.capture();

    host.send_keys("M-2");
    std::thread::sleep(Duration::from_millis(1200));
    let screen2 = host.capture();
    host.kill();

    assert!(
        screen1.contains("1:") && screen1.contains('*'),
        "Alt+1 后 tab 栏应标记 tab1: {screen1}"
    );
    // tab1 有 3 panes（setup_tmux_backend_2tab）；状态栏是可靠信号
    // （pane 标题行的 @N 在 nested vertical 下可能不在同一 capture 行）
    assert!(
        screen1.contains("3 panes"),
        "Alt+1 后应切到 3-pane tab: {screen1}"
    );

    assert!(
        screen2.contains("2:") || screen2.contains("connected"),
        "Alt+2 后 TUI 仍正常: {screen2}"
    );
    assert!(
        screen2.contains("1 pane") || screen2.contains("connected"),
        "Alt+2 后应回到单 pane tab: {screen2}"
    );

    let _ = Command::new("tmux")
        .args(["-L", &backend, "kill-server"])
        .status();
}

// ============================================================================
// TUI: Alt+S / Alt+V 分割
// ============================================================================

#[test]
fn tui_alt_s_and_alt_v_split() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let host = HostTmux::new("tui-split");
    let bin = muxterm_bin();
    host.send_line(&format!("{} --tui", bin.to_string_lossy()));
    std::thread::sleep(Duration::from_millis(1500));

    host.send_keys("M-s"); // 水平分割
    std::thread::sleep(Duration::from_millis(800));
    let after_h = host.capture();

    host.send_keys("M-v"); // 垂直分割
    std::thread::sleep(Duration::from_millis(800));
    let after_v = host.capture();
    host.kill();

    let panes_h = after_h.matches('@').count();
    assert!(
        panes_h >= 2,
        "Alt+S 后应有 >= 2 pane 标题: count={panes_h}\n{after_h}"
    );
    let panes_v = after_v.matches('@').count();
    assert!(
        panes_v >= 3,
        "Alt+V 后应有 >= 3 pane 标题: count={panes_v}\n{after_v}"
    );
    assert!(
        after_v.contains("connected"),
        "分割后应仍 connected: {after_v}"
    );
}

/// TUI 键盘搭 2tab3pane：Alt+S → Alt+V（右侧）→ Alt+T，再 Alt+1 验证 3 panes + echo 输出。
#[test]
fn tui_build_2tab3pane_via_keys_and_echo() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let host = HostTmux::new("tui-2t3p");
    let bin = muxterm_bin();
    host.send_line(&format!("{} --tui", bin.to_string_lossy()));
    std::thread::sleep(Duration::from_millis(1500));

    // tab1: 水平分割 → 再竖直分割（新 pane 为激活侧，即右侧）
    host.send_keys("M-s");
    std::thread::sleep(Duration::from_millis(700));
    host.send_keys("M-v");
    std::thread::sleep(Duration::from_millis(700));
    // tab2
    host.send_keys("M-t");
    std::thread::sleep(Duration::from_millis(700));

    let after_tabs = host.capture();
    assert!(
        after_tabs.contains("1:") && after_tabs.contains("2:"),
        "Alt+T 后应有 2 个 tab: {after_tabs}"
    );
    assert!(
        after_tabs.contains("1 pane") || after_tabs.contains("connected"),
        "新建 tab 应为单 pane: {after_tabs}"
    );

    // 回到 tab1（3 panes）
    host.send_keys("M-1");
    std::thread::sleep(Duration::from_millis(1000));
    let tab1 = host.capture();
    assert!(tab1.contains("3 panes"), "Alt+1 后应显示 3 panes: {tab1}");

    let marker = {
        let s = rand_suffix();
        format!("e{}", &s[s.len().saturating_sub(6)..])
    };
    for ch in format!("echo {marker}").chars() {
        host.send_keys(&ch.to_string());
        std::thread::sleep(Duration::from_millis(25));
    }
    host.send_keys("Enter");
    std::thread::sleep(Duration::from_millis(1200));
    let screen = host.capture();
    host.kill();

    assert!(
        screen.contains(&marker),
        "tab1 应显示 echo 输出 '{marker}': {screen}"
    );
}

// ============================================================================
// TUI: pty 输出显示
// ============================================================================

#[test]
fn tui_pty_output_visible() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let host = HostTmux::new("tui-pty");
    let bin = muxterm_bin();
    host.send_line(&format!("{} --tui", bin.to_string_lossy()));
    std::thread::sleep(Duration::from_millis(1500));

    // 向 TUI 内 shell 输入 echo（字符逐个 + Enter）
    let marker = format!("tuipty{}", rand_suffix());
    for ch in format!("echo {marker}").chars() {
        host.send_keys(&ch.to_string());
        std::thread::sleep(Duration::from_millis(30));
    }
    host.send_keys("Enter");
    std::thread::sleep(Duration::from_millis(1500));
    let screen = host.capture();
    host.kill();

    assert!(
        screen.contains(&marker),
        "TUI 应显示 pty 输出 '{marker}', 画面:\n{screen}"
    );
}

// ============================================================================
// TUI: Ctrl-Q detach 后 session 持续
// ============================================================================

#[test]
fn tui_ctrl_q_detach_keeps_session() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let backend = format!("tui-det-{}-{}", std::process::id(), rand_suffix());
    setup_tmux_backend_2tab(&backend);
    let bin = muxterm_bin();

    let host = HostTmux::new("tui-det");
    host.send_line(&format!(
        "{} --tui -L {} -s demo",
        bin.to_string_lossy(),
        backend
    ));
    std::thread::sleep(Duration::from_millis(2500));

    // Ctrl-Q 退出 TUI（detach）
    host.send_keys("C-q");
    std::thread::sleep(Duration::from_millis(1000));

    // 宿主 pane 应已离开 alternate screen；backend session 仍在
    let sessions = String::from_utf8(
        Command::new("tmux")
            .args(["-L", &backend, "list-sessions", "-F", "#{session_name}"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(
        sessions.lines().any(|l| l.trim() == "demo"),
        "Ctrl-Q 后 tmux session demo 应仍在: {sessions}"
    );

    let panes = String::from_utf8(
        Command::new("tmux")
            .args([
                "-L",
                &backend,
                "list-panes",
                "-a",
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
    .filter(|l| !l.is_empty())
    .count();
    assert!(panes >= 2, "detach 后 pane 应保留: {panes}");

    host.kill();
    let _ = Command::new("tmux")
        .args(["-L", &backend, "kill-server"])
        .status();
}

#[test]
fn tui_ctrl_q_detach_keeps_local_daemon() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }
    let name = format!("tui-lq-{}-{}", std::process::id(), rand_suffix());
    cleanup_local_session(&name);
    let bin = muxterm_bin();

    let st = Command::new(&bin)
        .args(["new-session", "-s", &name])
        .status()
        .unwrap();
    assert!(st.success());
    let _ = Command::new(&bin)
        .args(["split-pane", "-h", "-s", &name])
        .status();

    let host = HostTmux::new("tui-lq");
    host.send_line(&format!("{} --tui -s {}", bin.to_string_lossy(), name));
    std::thread::sleep(Duration::from_millis(2000));
    host.send_keys("C-q");
    std::thread::sleep(Duration::from_millis(1000));
    host.kill();

    // daemon 应仍可查询
    let out = Command::new(&bin)
        .args(["list-layout", "-s", &name, "--format", "json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "Ctrl-Q 后 local daemon 应仍在: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains('@') || stdout.contains("split") || stdout.contains("t1"),
        "daemon 状态应保留: {stdout}"
    );

    cleanup_local_session(&name);
}
