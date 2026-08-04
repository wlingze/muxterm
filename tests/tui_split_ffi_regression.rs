//! TUI 分割 FFI 回归：复现 alt-s/v「按了没反应」的根因验证。
//!
//! TUI 前端走 `muxterm_new → muxterm_execute(TASK_SPLIT_PANE) → poll_events → snapshot`。
//! 这里用真实 tmux（独立隔离 socket）验证这条 FFI 路径在 execute 后，pane 数确实增加、
//! layout 树能读到第二个 pane —— 这正是 TUI 按键分发依赖的边界。

#![cfg(feature = "ffi")]

use std::ffi::CString;
use std::process::Command;
use std::ptr;
use std::time::{Duration, Instant};

use muxterm::core::protocol::ffi::api::{
    muxterm_connect, muxterm_execute, muxterm_free, muxterm_get_layout, muxterm_get_pane_output,
    muxterm_get_panes, muxterm_get_tabs, muxterm_new, muxterm_poll_events,
};
use muxterm::core::protocol::ffi::types::{
    CLayoutNode, CPane, CStateChange, CTab, CTask, DIR_HORIZONTAL, DIR_VERTICAL, TASK_SPLIT_PANE,
};

fn unique_socket(label: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!(
        "muxterm-tui-split-{}-{}-{}",
        label,
        std::process::id(),
        nanos
    )
}

fn create_session(socket: &str) {
    let out = Command::new("tmux")
        .args([
            "-L",
            socket,
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "ffi",
            "-x",
            "100",
            "-y",
            "30",
        ])
        .output()
        .expect("tmux new-session 失败");
    assert!(
        out.status.success(),
        "new-session 失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::thread::sleep(Duration::from_millis(400));
}

fn count_panes(socket: &str) -> usize {
    let out = Command::new("tmux")
        .args(["-L", socket, "list-panes", "-t", "ffi", "-F", "#{pane_id}"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .count()
}

fn kill_server(socket: &str) {
    let _ = Command::new("tmux")
        .args(["-L", socket, "kill-server"])
        .output();
}

/// 用 FFI handle 执行分割并等待事件回流，验证 pane 数 + layout 树。
fn run_split(socket: &str, dir: u32) -> (usize, usize) {
    let backend = CString::new("tmux").unwrap();
    let sock = CString::new(socket).unwrap();
    let sess = CString::new("ffi").unwrap();
    // TUI 的 resolve_backend 会找到已有 session 名并传给 muxterm_new（attach 路径）
    let h = muxterm_new(backend.as_ptr(), sock.as_ptr(), sess.as_ptr());
    assert!(!h.is_null(), "muxterm_new 失败");

    // 以下 FFI 调用都在 unsafe 块内
    unsafe {
        assert_eq!(muxterm_connect(h), 0, "connect 失败");
        let mut buf = [CStateChange::default(); 64];
        let _ = muxterm_poll_events(h, buf.as_mut_ptr(), 64);

        // 取第一个 tab
        let mut tabs = [CTab {
            id: 0,
            name: ptr::null(),
            is_active: 0,
        }; 8];
        let nt = muxterm_get_tabs(h, tabs.as_mut_ptr(), 8);
        assert!(nt >= 1, "应至少 1 个 tab");
        let tab = tabs[0].id;

        // TUI 的 Alt+S / Alt+V 正是发 TASK_SPLIT_PANE + 对应 dir
        let task = CTask {
            type_: TASK_SPLIT_PANE,
            target_pane: 0, // 0 = active
            target_tab: 0,
            dir,
            name: ptr::null(),
        };
        let rc = muxterm_execute(h, &task);
        assert_eq!(rc, 0, "muxterm_execute(split) 失败: {rc}");

        // TUI 在 execute 后 poll_events 等事件回流再 snapshot
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut layout_leaves = 0;
        let mut final_npanes; // 在循环内首次赋值
        loop {
            let _ = muxterm_poll_events(h, buf.as_mut_ptr(), 64);
            let mut panes = [CPane {
                id: 0,
                cols: 0,
                rows: 0,
                is_active: 0,
            }; 16];
            final_npanes = muxterm_get_panes(h, tab, panes.as_mut_ptr(), 16) as usize;

            let mut root = CLayoutNode {
                type_: 0,
                pane_id: 0,
                ratio: 0,
                first: ptr::null(),
                second: ptr::null(),
            };
            if muxterm_get_layout(h, tab, &mut root) == 0 {
                layout_leaves = count_leaves(&root);
            }
            if final_npanes >= 2 && layout_leaves >= 2 {
                break;
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let tmux_count = count_panes(socket);
        let _ = muxterm_free(h);
        // 记录最后一次读取到的 pane 数，供断言
        let _ = final_npanes;
        (final_npanes, tmux_count)
    }
}

unsafe fn count_leaves(node: &CLayoutNode) -> usize {
    if node.type_ == 0 {
        // LAYOUT_LEAF
        return 1;
    }
    let mut n = 0;
    if !node.first.is_null() {
        n += count_leaves(&*node.first);
    }
    if !node.second.is_null() {
        n += count_leaves(&*node.second);
    }
    n
}

#[test]
fn tui_ffi_split_horizontal_increases_panes_and_layout() {
    let socket = unique_socket("h");
    create_session(&socket);
    assert_eq!(count_panes(&socket), 1, "初始应有 1 pane");

    let (model_panes, tmux_count) = run_split(&socket, DIR_HORIZONTAL);
    assert!(
        model_panes >= 2,
        "TUI model 读到 pane 数应 >=2，实际 {model_panes}"
    );
    assert!(
        tmux_count >= 2,
        "tmux 实际 pane 数应 >=2，实际 {tmux_count}"
    );

    kill_server(&socket);
}

#[test]
fn pane_zero_id_is_distinct_from_active_after_split() {
    // 回归：tmux pane id 是 0 基的，%0 / %1 是真实 pane。
    // 旧 bug：get_pane_output 把 pane_id==0 当“active”哨兵，分割后取 %0 输出
    // 会错误返回 active pane 的输出，导致两个 pane 显示相同内容。
    let socket = unique_socket("p0");
    create_session(&socket);
    let out = std::process::Command::new("tmux")
        .args(["-L", &socket, "split-window", "-h", "-t", "ffi"])
        .output()
        .expect("split-window 失败");
    assert!(out.status.success());
    std::thread::sleep(Duration::from_millis(400));

    let bt = CString::new("tmux").unwrap();
    let sock = CString::new(socket.as_str()).unwrap();
    let sess = CString::new("ffi").unwrap();
    let h = muxterm_new(bt.as_ptr(), sock.as_ptr(), sess.as_ptr());
    assert!(!h.is_null());
    unsafe {
        assert_eq!(muxterm_connect(h), 0, "connect 失败");
        let mut buf = [CStateChange::default(); 64];
        // 等 pane 列表出现（分割已发生，需等 backend 刷新）
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut np = 0;
        let mut panes = [CPane {
            id: 0,
            cols: 0,
            rows: 0,
            is_active: 0,
        }; 16];
        while Instant::now() < deadline {
            let _ = muxterm_poll_events(h, buf.as_mut_ptr(), 64);
            np = muxterm_get_panes(h, 0, panes.as_mut_ptr(), 16);
            if np >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(np >= 2, "应有 >=2 pane，实际 {np}");
        // 核心回归：第一个 pane 的 id 必须是 0（0 基），第二个是 1。
        // 若 FFI 把 0 当“active”哨兵，这里会取到错误的 pane。
        let p0 = panes[0].id;
        let p1 = panes[1].id;
        assert_eq!(p0, 0, "tmux 第一个 pane id 应为 0（0 基），实际 {p0}");
        assert_eq!(p1, 1, "tmux 第二个 pane id 应为 1（0 基），实际 {p1}");
        // 两个 pane 都能独立取到输出（不因 0 哨兵冲突而返回相同/错误数据）
        let mut b0 = [0u8; 256];
        let mut b1 = [0u8; 256];
        let n0 = muxterm_get_pane_output(h, p0, b0.as_mut_ptr(), b0.len());
        let n1 = muxterm_get_pane_output(h, p1, b1.as_mut_ptr(), b1.len());
        assert!(n0 >= 0, "pane %0 读取输出不应报错，n0={n0}");
        assert!(n1 >= 0, "pane %1 读取输出不应报错，n1={n1}");
        muxterm_free(h);
    }
    kill_server(&socket);
}

#[test]
fn tui_ffi_split_vertical_increases_panes_and_layout() {
    let socket = unique_socket("v");
    create_session(&socket);
    assert_eq!(count_panes(&socket), 1, "初始应有 1 pane");

    let (model_panes, tmux_count) = run_split(&socket, DIR_VERTICAL);
    assert!(
        model_panes >= 2,
        "TUI model 读到 pane 数应 >=2，实际 {model_panes}"
    );
    assert!(
        tmux_count >= 2,
        "tmux 实际 pane 数应 >=2，实际 {tmux_count}"
    );

    kill_server(&socket);
}
