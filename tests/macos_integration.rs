#![cfg(feature = "ffi")]

//! macOS 客户端同源 FFI 集成测试：2tab3pane tmux session attach 后校验布局。
//!
//! 与 `tests/tui_integration.rs` 的 `setup_tmux_backend_2tab` 同构：
//! tab1 = 3 panes（水平 split + 右侧竖直 split），tab2 = 1 pane。
//!
//! **键盘驱动的端到端 UI 测试**见
//! `src/platform/macos/MuxtermAppUITests/MuxtermAppUITests.swift`
//! （`testTwoTabThreePaneViaKeyboard` / Ctrl+D 用例），必须用 `app.typeKey`。
//!
//! ## CLI/TUI 复现笔记
//! - macOS：Cmd+D / Cmd+Shift+D 分屏；Ctrl+D 关 pane/tab/window。
//! - TUI 仍将 Ctrl+D 映射为退出应用（`is_quit`）。
//! - LocalBackend：单 pane Close → 关 window；多 tab 末 pane → 只关该 tab。
//!
//! 跑：`cargo test --no-default-features --features ffi --test macos_integration`

use std::ffi::CString;
use std::process::Command;
use std::ptr;
use std::time::{Duration, Instant};

use muxterm::core::protocol::ffi::api::{
    muxterm_connect, muxterm_execute, muxterm_free, muxterm_get_layout, muxterm_get_pane_output,
    muxterm_get_panes, muxterm_get_tabs, muxterm_new, muxterm_poll_events, muxterm_resize_client,
    muxterm_resize_pane_axis, muxterm_shutdown,
};
use muxterm::core::protocol::ffi::types::{
    CLayoutNode, CPane, CStateChange, CTab, CTask, DIR_HORIZONTAL, DIR_VERTICAL, LAYOUT_LEAF,
    LAYOUT_SPLIT_H, LAYOUT_SPLIT_V, TASK_NEW_TAB, TASK_SPLIT_PANE, TASK_SWITCH_TAB,
};

fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn rand_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{nanos}")
}

/// 与 TUI 集成测试相同：demo session，window0 三 pane，window1 单 pane。
fn setup_tmux_backend_2tab3pane(backend_sock: &str) {
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

fn count_layout_leaves(node: &CLayoutNode) -> usize {
    match node.type_ {
        LAYOUT_SPLIT_H | LAYOUT_SPLIT_V => {
            let mut n = 0;
            if !node.first.is_null() {
                n += count_layout_leaves(unsafe { &*node.first });
            }
            if !node.second.is_null() {
                n += count_layout_leaves(unsafe { &*node.second });
            }
            n
        }
        _ => 1,
    }
}

fn layout_has_split(node: &CLayoutNode) -> bool {
    matches!(node.type_, LAYOUT_SPLIT_H | LAYOUT_SPLIT_V)
}

fn tmux_pane_sizes(backend_sock: &str) -> Vec<(String, u16, u16)> {
    let output = Command::new("tmux")
        .args([
            "-L",
            backend_sock,
            "list-panes",
            "-a",
            "-F",
            "#{pane_id} #{pane_width} #{pane_height}",
        ])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            Some((
                parts.next()?.to_string(),
                parts.next()?.parse().ok()?,
                parts.next()?.parse().ok()?,
            ))
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
enum LayoutShape {
    Leaf(u32),
    Split {
        type_: u32,
        first: Box<LayoutShape>,
        second: Box<LayoutShape>,
    },
}

unsafe fn clone_layout_shape(node: &CLayoutNode) -> LayoutShape {
    match node.type_ {
        LAYOUT_SPLIT_H | LAYOUT_SPLIT_V => LayoutShape::Split {
            type_: node.type_,
            first: Box::new(clone_layout_shape(&*node.first)),
            second: Box::new(clone_layout_shape(&*node.second)),
        },
        _ => LayoutShape::Leaf(node.pane_id),
    }
}

unsafe fn tab_layout_shape(
    h: *mut muxterm::core::protocol::ffi::api::MuxtermHandle,
    tab_id: u32,
) -> (Vec<u32>, LayoutShape) {
    // FFI 约定 tab_id=0 表示 active tab；tmux 的第一个 window 也可能
    // 恰好是 id=0，所以测试访问该 tab 前必须先切过去。
    if tab_id == 0 {
        switch_tab(h, tab_id);
    }
    let query_id = if tab_id == 0 { 0 } else { tab_id };
    let mut panes = [CPane {
        id: 0,
        cols: 0,
        rows: 0,
        is_active: 0,
    }; 32];
    let n = muxterm_get_panes(h, query_id, panes.as_mut_ptr(), panes.len() as i32);
    assert!(n >= 1, "tab {tab_id} 应至少有一个 pane, n={n}");
    let pane_ids: Vec<u32> = panes[..n as usize].iter().map(|p| p.id).collect();

    let mut root = CLayoutNode {
        type_: LAYOUT_LEAF,
        pane_id: 0,
        ratio: 0,
        first: ptr::null(),
        second: ptr::null(),
    };
    assert_eq!(muxterm_get_layout(h, query_id, &mut root), 0);
    let shape = clone_layout_shape(&root);
    let mut leaves = Vec::new();
    collect_shape_leaves(&shape, &mut leaves);
    let mut expected = pane_ids.clone();
    leaves.sort_unstable();
    expected.sort_unstable();
    assert_eq!(
        leaves, expected,
        "tab {tab_id} layout 叶子必须只属于当前 tab"
    );
    (pane_ids, shape)
}

fn collect_shape_leaves(shape: &LayoutShape, out: &mut Vec<u32>) {
    match shape {
        LayoutShape::Leaf(id) => out.push(*id),
        LayoutShape::Split { first, second, .. } => {
            collect_shape_leaves(first, out);
            collect_shape_leaves(second, out);
        }
    }
}

unsafe fn wait_for_tab_count(
    h: *mut muxterm::core::protocol::ffi::api::MuxtermHandle,
    expected: i32,
) {
    let mut ev = [CStateChange::default(); 128];
    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(100));
        let _ = muxterm_poll_events(h, ev.as_mut_ptr(), ev.len() as i32);
        let mut tabs = [CTab {
            id: 0,
            name: ptr::null(),
            is_active: 0,
        }; 16];
        if muxterm_get_tabs(h, tabs.as_mut_ptr(), tabs.len() as i32) == expected {
            return;
        }
    }
    let mut tabs = [CTab {
        id: 0,
        name: ptr::null(),
        is_active: 0,
    }; 16];
    let actual = muxterm_get_tabs(h, tabs.as_mut_ptr(), tabs.len() as i32);
    panic!("等待 tab 数量失败：expected={expected}, actual={actual}");
}

unsafe fn switch_tab(h: *mut muxterm::core::protocol::ffi::api::MuxtermHandle, tab_id: u32) {
    let task = CTask {
        type_: TASK_SWITCH_TAB,
        target_pane: 0,
        target_tab: tab_id,
        dir: 0,
        name: ptr::null(),
    };
    assert_eq!(muxterm_execute(h, &task), 0);
    let mut ev = [CStateChange::default(); 64];
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        let _ = muxterm_poll_events(h, ev.as_mut_ptr(), 64);
        let mut tabs = [CTab {
            id: 0,
            name: ptr::null(),
            is_active: 0,
        }; 8];
        let n = muxterm_get_tabs(h, tabs.as_mut_ptr(), 8);
        if tabs[..n as usize]
            .iter()
            .any(|tab| tab.id == tab_id && tab.is_active != 0)
        {
            return;
        }
    }
    panic!("切换到 tab {tab_id} 超时");
}

unsafe fn active_pane_count(h: *mut muxterm::core::protocol::ffi::api::MuxtermHandle) -> i32 {
    let mut panes = [CPane {
        id: 0,
        cols: 0,
        rows: 0,
        is_active: 0,
    }; 16];
    // tab_id=0 → 当前 active tab（FFI 约定）
    muxterm_get_panes(h, 0, panes.as_mut_ptr(), 16)
}

unsafe fn active_layout_root(
    h: *mut muxterm::core::protocol::ffi::api::MuxtermHandle,
) -> CLayoutNode {
    let mut root = CLayoutNode {
        type_: LAYOUT_LEAF,
        pane_id: 0,
        ratio: 0,
        first: ptr::null(),
        second: ptr::null(),
    };
    assert_eq!(muxterm_get_layout(h, 0, &mut root), 0);
    root
}

#[test]
fn macos_ffi_attach_2tab3pane_layout() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }

    let backend = format!("mac-2t3p-{}-{}", std::process::id(), rand_suffix());
    setup_tmux_backend_2tab3pane(&backend);

    let bt = CString::new("tmux").unwrap();
    let sock = CString::new(backend.as_str()).unwrap();
    let sess = CString::new("demo").unwrap();
    let h = muxterm_new(bt.as_ptr(), sock.as_ptr(), sess.as_ptr());
    assert!(!h.is_null(), "muxterm_new 失败");

    unsafe {
        assert_eq!(muxterm_connect(h), 0, "muxterm_connect 失败");
        let mut ev = [CStateChange::default(); 64];
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(150));
            let _ = muxterm_poll_events(h, ev.as_mut_ptr(), 64);
        }

        let mut tabs = [CTab {
            id: 0,
            name: ptr::null(),
            is_active: 0,
        }; 8];
        let ntabs = muxterm_get_tabs(h, tabs.as_mut_ptr(), 8);
        assert_eq!(ntabs, 2, "应有 2 个 tab（2tab3pane）, ntabs={ntabs}");

        let tab_ids: Vec<u32> = (0..ntabs as usize).map(|i| tabs[i].id).collect();
        let mut pane_counts: Vec<(u32, i32)> = Vec::new();

        for tid in &tab_ids {
            switch_tab(h, *tid);
            let np = active_pane_count(h);
            let root = active_layout_root(h);
            let leaves = count_layout_leaves(&root);
            assert_eq!(
                leaves, np as usize,
                "tab {tid} layout leaves={leaves} 应等于 panes={np}"
            );
            if np == 3 {
                assert!(
                    layout_has_split(&root),
                    "3-pane tab 的 layout 应为 split 树"
                );
            }
            pane_counts.push((*tid, np));
        }

        let has_3 = pane_counts.iter().any(|(_, n)| *n == 3);
        let has_1 = pane_counts.iter().any(|(_, n)| *n == 1);
        assert!(
            has_3 && has_1,
            "应有 3-pane tab 与 1-pane tab: {pane_counts:?}"
        );

        // 再切回 3-pane tab 确认稳定
        let three_tab = pane_counts
            .iter()
            .find(|(_, n)| *n == 3)
            .map(|(id, _)| *id)
            .unwrap();
        switch_tab(h, three_tab);
        assert_eq!(active_pane_count(h), 3, "切换后 3-pane tab 仍应有 3 panes");

        let _ = muxterm_shutdown(h);
        muxterm_free(h);
    }

    let _ = Command::new("tmux")
        .args(["-L", &backend, "kill-server"])
        .status();
}

/// macOS FFI 回归：attach 前已经存在的 shell 画面必须可从累计输出读取，
/// 这样 SwiftTerm 首次创建 pane view 时才能恢复，而不是只有后续输入可见。
#[test]
fn macos_ffi_attach_restores_existing_shell_screen_output() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }

    let backend = format!("mac-restore-{}-{}", std::process::id(), rand_suffix());
    let marker = "MAC_ATTACH_RESTORE_74291";
    let created = Command::new("tmux")
        .args([
            "-L",
            &backend,
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
        eprintln!("skip: 无法创建 tmux session");
        let _ = Command::new("tmux")
            .args(["-L", &backend, "kill-server"])
            .status();
        return;
    }
    let command = format!("printf '{marker}\\n'");
    assert!(
        Command::new("tmux")
            .args([
                "-L",
                &backend,
                "send-keys",
                "-t",
                "restore",
                &command,
                "Enter"
            ])
            .status()
            .map(|status| status.success())
            .unwrap_or(false),
        "预置 tmux shell 画面失败"
    );
    std::thread::sleep(Duration::from_millis(150));

    let bt = CString::new("tmux").unwrap();
    let sock = CString::new(backend.as_str()).unwrap();
    let sess = CString::new("restore").unwrap();
    let h = muxterm_new(bt.as_ptr(), sock.as_ptr(), sess.as_ptr());
    assert!(!h.is_null(), "muxterm_new 失败");

    unsafe {
        assert_eq!(muxterm_connect(h), 0, "muxterm_connect 失败");
        let mut events = [CStateChange::default(); 64];
        let mut restored = false;
        for _ in 0..60 {
            let _ = muxterm_poll_events(h, events.as_mut_ptr(), events.len() as i32);
            let mut panes = [CPane {
                id: 0,
                cols: 0,
                rows: 0,
                is_active: 0,
            }; 16];
            let count = muxterm_get_panes(h, 0, panes.as_mut_ptr(), panes.len() as i32);
            for pane in panes.iter().take(count.max(0) as usize) {
                let mut output = vec![0u8; 256 * 1024];
                let n = muxterm_get_pane_output(h, pane.id, output.as_mut_ptr(), output.len());
                if n > 0 && String::from_utf8_lossy(&output[..n as usize]).contains(marker) {
                    restored = true;
                    break;
                }
            }
            if restored {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            restored,
            "attach 后 FFI 应恢复已有 shell 画面 marker={marker}"
        );
        let _ = muxterm_shutdown(h);
        muxterm_free(h);
    }

    let _ = Command::new("tmux")
        .args(["-L", &backend, "kill-server"])
        .status();
}

/// macOS FFI 性能回归：split/new-tab 不能因为事件轮询或 tmux 布局查询
/// 长时间没有反馈。阈值是用户可感知延迟的上限，不是微基准。
#[test]
fn macos_ffi_split_and_new_tab_complete_within_latency_budget() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }

    let backend = format!("mac-latency-{}-{}", std::process::id(), rand_suffix());
    let created = Command::new("tmux")
        .args([
            "-L",
            &backend,
            "new-session",
            "-d",
            "-s",
            "latency",
            "-x",
            "100",
            "-y",
            "30",
        ])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !created {
        eprintln!("skip: 无法创建 tmux session");
        return;
    }

    let bt = CString::new("tmux").unwrap();
    let sock = CString::new(backend.as_str()).unwrap();
    let sess = CString::new("latency").unwrap();
    let h = muxterm_new(bt.as_ptr(), sock.as_ptr(), sess.as_ptr());
    assert!(!h.is_null(), "muxterm_new 失败");

    unsafe {
        assert_eq!(muxterm_connect(h), 0, "muxterm_connect 失败");
        let mut events = [CStateChange::default(); 128];
        for _ in 0..50 {
            let _ = muxterm_poll_events(h, events.as_mut_ptr(), events.len() as i32);
            let mut panes = [CPane {
                id: 0,
                cols: 0,
                rows: 0,
                is_active: 0,
            }; 8];
            if muxterm_get_panes(h, 0, panes.as_mut_ptr(), 8) == 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        let split_task = CTask {
            type_: TASK_SPLIT_PANE,
            target_pane: 0,
            target_tab: 0,
            dir: DIR_HORIZONTAL,
            name: ptr::null(),
        };
        let split_started = Instant::now();
        assert_eq!(muxterm_execute(h, &split_task), 0);
        let mut split_elapsed = None;
        for _ in 0..200 {
            let _ = muxterm_poll_events(h, events.as_mut_ptr(), events.len() as i32);
            if active_pane_count(h) >= 2 {
                split_elapsed = Some(split_started.elapsed());
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let split_elapsed = split_elapsed.expect("split 应在 2 秒内反映到 FFI state");
        assert!(
            split_elapsed < Duration::from_secs(1),
            "split 状态反馈过慢: {split_elapsed:?}"
        );

        let new_tab = CTask {
            type_: TASK_NEW_TAB,
            target_pane: 0,
            target_tab: 0,
            dir: 0,
            name: ptr::null(),
        };
        let tab_started = Instant::now();
        assert_eq!(muxterm_execute(h, &new_tab), 0);
        let mut tab_elapsed = None;
        for _ in 0..200 {
            let _ = muxterm_poll_events(h, events.as_mut_ptr(), events.len() as i32);
            let mut tabs = [CTab {
                id: 0,
                name: ptr::null(),
                is_active: 0,
            }; 8];
            if muxterm_get_tabs(h, tabs.as_mut_ptr(), 8) >= 2 {
                tab_elapsed = Some(tab_started.elapsed());
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let tab_elapsed = tab_elapsed.expect("new-tab 应在 2 秒内反映到 FFI state");
        assert!(
            tab_elapsed < Duration::from_secs(1),
            "new-tab 状态反馈过慢: {tab_elapsed:?}"
        );

        let _ = muxterm_shutdown(h);
        muxterm_free(h);
    }
    let _ = Command::new("tmux")
        .args(["-L", &backend, "kill-server"])
        .status();
}

/// 回归用户报告的完整操作序列：
/// attach(首 tab 3 pane) → 当前 pane 再 split → 新建 tab → 再新建 tab →
/// 反复切换。每个时刻都检查布局树方向、叶子数量以及 pane 不跨 tab。
#[test]
fn macos_ffi_layout_stays_isolated_after_split_and_new_tabs() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }

    let backend = format!("mac-layout-seq-{}-{}", std::process::id(), rand_suffix());
    setup_tmux_backend_2tab3pane(&backend);
    let bt = CString::new("tmux").unwrap();
    let sock = CString::new(backend.as_str()).unwrap();
    let sess = CString::new("demo").unwrap();
    let h = muxterm_new(bt.as_ptr(), sock.as_ptr(), sess.as_ptr());
    assert!(!h.is_null(), "muxterm_new 失败");

    unsafe {
        assert_eq!(muxterm_connect(h), 0, "muxterm_connect 失败");
        let mut ev = [CStateChange::default(); 128];
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(100));
            let _ = muxterm_poll_events(h, ev.as_mut_ptr(), ev.len() as i32);
        }

        let mut tabs = [CTab {
            id: 0,
            name: ptr::null(),
            is_active: 0,
        }; 8];
        let ntabs = muxterm_get_tabs(h, tabs.as_mut_ptr(), tabs.len() as i32);
        assert_eq!(ntabs, 2, "attach 后应有 2 个 tab");
        let tab_ids: Vec<u32> = tabs[..ntabs as usize].iter().map(|t| t.id).collect();
        let three_tab = *tab_ids
            .iter()
            .find(|&&id| tab_layout_shape(h, id).0.len() == 3)
            .expect("应找到 3-pane tab");
        let one_tab = *tab_ids
            .iter()
            .find(|&&id| tab_layout_shape(h, id).0.len() == 1)
            .expect("应找到 1-pane tab");

        // 初始 attach 的嵌套方向必须是：左/上单 pane + 右/下再 split。
        let (_, initial_shape) = tab_layout_shape(h, three_tab);
        match initial_shape {
            LayoutShape::Split {
                type_: LAYOUT_SPLIT_H,
                second,
                ..
            } => assert!(matches!(
                *second,
                LayoutShape::Split {
                    type_: LAYOUT_SPLIT_V,
                    ..
                }
            )),
            other => panic!("attach 后 3-pane 布局方向错误: {other:?}"),
        }

        // 切到 3-pane tab，split 当前 active pane；新树必须有 4 个叶子。
        switch_tab(h, three_tab);
        let mut panes = [CPane {
            id: 0,
            cols: 0,
            rows: 0,
            is_active: 0,
        }; 16];
        let npanes = muxterm_get_panes(h, three_tab, panes.as_mut_ptr(), 16);
        assert_eq!(npanes, 3);
        let active = panes
            .iter()
            .take(npanes as usize)
            .find(|p| p.is_active != 0)
            .expect("3-pane tab 应有 active pane")
            .id;
        let split = CTask {
            type_: TASK_SPLIT_PANE,
            target_pane: active,
            target_tab: 0,
            dir: DIR_VERTICAL,
            name: ptr::null(),
        };
        assert_eq!(muxterm_execute(h, &split), 0);

        let mut split_ready = false;
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(100));
            let _ = muxterm_poll_events(h, ev.as_mut_ptr(), ev.len() as i32);
            let (ids, _) = tab_layout_shape(h, three_tab);
            if ids.len() == 4 {
                split_ready = true;
                break;
            }
        }
        assert!(split_ready, "split 后 3-pane tab 应变成 4-pane");
        let (four_ids, four_shape) = tab_layout_shape(h, three_tab);
        assert_eq!(four_ids.len(), 4);
        assert!(matches!(four_shape, LayoutShape::Split { .. }));

        // 新建 tab 后，tmux active tab 应只有 1 个 pane，不能暂时复用旧 4-pane 树。
        let new_tab = CTask {
            type_: TASK_NEW_TAB,
            target_pane: 0,
            target_tab: 0,
            dir: 0,
            name: ptr::null(),
        };
        assert_eq!(muxterm_execute(h, &new_tab), 0);
        wait_for_tab_count(h, 3);
        let mut tabs_after = [CTab {
            id: 0,
            name: ptr::null(),
            is_active: 0,
        }; 8];
        let n3 = muxterm_get_tabs(h, tabs_after.as_mut_ptr(), 8);
        let tab3 = tabs_after[..n3 as usize]
            .iter()
            .find(|t| t.id != three_tab && t.id != one_tab && t.is_active != 0)
            .map(|t| t.id)
            .expect("新建 tab 后应有 active tab");
        for _ in 0..50 {
            let mut new_panes = [CPane {
                id: 0,
                cols: 0,
                rows: 0,
                is_active: 0,
            }; 8];
            if muxterm_get_panes(h, tab3, new_panes.as_mut_ptr(), 8) >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            let _ = muxterm_poll_events(h, ev.as_mut_ptr(), ev.len() as i32);
        }
        let (tab3_panes, tab3_shape) = tab_layout_shape(h, tab3);
        assert_eq!(tab3_panes.len(), 1);
        assert_eq!(tab3_shape, LayoutShape::Leaf(tab3_panes[0]));

        // 再切换所有 tab；每一帧的 layout leaves 都必须严格属于当前 tab。
        for tab_id in [three_tab, one_tab, tab3, three_tab, one_tab, tab3] {
            switch_tab(h, tab_id);
            let (ids, shape) = tab_layout_shape(h, tab_id);
            match tab_id {
                id if id == three_tab => assert_eq!(ids.len(), 4),
                id if id == one_tab => assert_eq!(ids.len(), 1),
                id if id == tab3 => assert_eq!(ids.len(), 1),
                _ => unreachable!(),
            }
            if ids.len() == 1 {
                assert_eq!(shape, LayoutShape::Leaf(ids[0]));
            }
        }

        assert_eq!(muxterm_shutdown(h), 0);
        muxterm_free(h);
    }

    let _ = Command::new("tmux")
        .args(["-L", &backend, "kill-server"])
        .status();
}

/// 回归尺寸反馈环：client resize 后布局叶子数和 pane 尺寸应稳定，
/// 连续收到 layout-change 不能把一个 pane 越推越大、其他 pane 压成零。
#[test]
fn macos_ffi_tmux_client_resize_is_stable_and_pane_axis_resize_persists() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }

    let backend = format!("mac-resize-{}-{}", std::process::id(), rand_suffix());
    setup_tmux_backend_2tab3pane(&backend);
    let bt = CString::new("tmux").unwrap();
    let sock = CString::new(backend.as_str()).unwrap();
    let sess = CString::new("demo").unwrap();
    let h = muxterm_new(bt.as_ptr(), sock.as_ptr(), sess.as_ptr());
    assert!(!h.is_null(), "muxterm_new 失败");

    unsafe {
        assert_eq!(muxterm_connect(h), 0);
        let mut ev = [CStateChange::default(); 128];
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(80));
            let _ = muxterm_poll_events(h, ev.as_mut_ptr(), ev.len() as i32);
        }

        let before = tmux_pane_sizes(&backend);
        assert_eq!(before.len(), 4, "测试窗口应有 4 个 pane: {before:?}");
        let mut tabs = [CTab {
            id: 0,
            name: ptr::null(),
            is_active: 0,
        }; 8];
        let ntabs = muxterm_get_tabs(h, tabs.as_mut_ptr(), tabs.len() as i32);
        assert_eq!(ntabs, 2);
        let three_tab = (0..ntabs as usize)
            .map(|i| tabs[i].id)
            .find(|id| tab_layout_shape(h, *id).0.len() == 3)
            .expect("应找到 3-pane active tab");
        switch_tab(h, three_tab);
        let root_before = active_layout_root(h);
        assert_eq!(
            count_layout_leaves(&root_before),
            3,
            "active tab 应为 3 pane"
        );

        for _ in 0..3 {
            assert_eq!(muxterm_resize_client(h, 120, 36), 0);
            std::thread::sleep(Duration::from_millis(120));
            let _ = muxterm_poll_events(h, ev.as_mut_ptr(), ev.len() as i32);
        }
        let after = tmux_pane_sizes(&backend);
        assert_eq!(after.len(), before.len());
        assert!(after
            .iter()
            .all(|(_, cols, rows)| *cols >= 10 && *rows >= 5));
        assert!(after
            .iter()
            .all(|(_, cols, rows)| *cols <= 120 && *rows <= 36));
        assert_eq!(count_layout_leaves(&active_layout_root(h)), 3);

        // 第一 pane 是初始横向 split 左侧的边界 pane，单轴 resize 后应保存到 tmux layout。
        let mut panes = [CPane {
            id: 0,
            cols: 0,
            rows: 0,
            is_active: 0,
        }; 16];
        let n = muxterm_get_panes(h, three_tab, panes.as_mut_ptr(), panes.len() as i32);
        assert_eq!(n, 3);
        let target = panes[0].id;
        assert_eq!(muxterm_resize_pane_axis(h, target, DIR_HORIZONTAL, 60), 0);
        std::thread::sleep(Duration::from_millis(180));
        let _ = muxterm_poll_events(h, ev.as_mut_ptr(), ev.len() as i32);
        let persisted = tmux_pane_sizes(&backend);
        let target_size = persisted
            .iter()
            .find(|(id, _, _)| id == &format!("%{target}"))
            .expect("axis resize 的目标 pane 应仍存在");
        assert_eq!(target_size.1, 60, "横向分隔条尺寸应保存到 tmux");
        assert!(persisted
            .iter()
            .all(|(_, cols, rows)| *cols >= 10 && *rows >= 5));
        assert_eq!(count_layout_leaves(&active_layout_root(h)), 3);

        // 第二个 layout 叶子位于内层纵向 split，验证上下分隔条也能持久化。
        let vertical_target = panes[1].id;
        assert_eq!(
            muxterm_resize_pane_axis(h, vertical_target, DIR_VERTICAL, 18),
            0
        );
        std::thread::sleep(Duration::from_millis(180));
        let _ = muxterm_poll_events(h, ev.as_mut_ptr(), ev.len() as i32);
        let persisted_vertical = tmux_pane_sizes(&backend);
        let vertical_size = persisted_vertical
            .iter()
            .find(|(id, _, _)| id == &format!("%{vertical_target}"))
            .expect("vertical axis resize 的目标 pane 应仍存在");
        assert_eq!(vertical_size.2, 18, "纵向分隔条尺寸应保存到 tmux");

        assert_eq!(muxterm_shutdown(h), 0);
        muxterm_free(h);
    }

    let _ = Command::new("tmux")
        .args(["-L", &backend, "kill-server"])
        .status();
}

/// 窗口 live resize 回归：client 尺寸变化不应把 active tab、其它 tab 或
/// 嵌套分割方向混在一起。每次 resize 后切回两个 tab，验证 layout 叶子和
/// H/V 拓扑仍与 attach 时一致。
#[test]
fn macos_ffi_window_resize_preserves_tab_layouts_and_directions() {
    if !tmux_available() {
        eprintln!("skip: tmux 不可用");
        return;
    }

    let backend = format!("mac-window-resize-{}-{}", std::process::id(), rand_suffix());
    setup_tmux_backend_2tab3pane(&backend);
    let bt = CString::new("tmux").unwrap();
    let sock = CString::new(backend.as_str()).unwrap();
    let sess = CString::new("demo").unwrap();
    let h = muxterm_new(bt.as_ptr(), sock.as_ptr(), sess.as_ptr());
    assert!(!h.is_null(), "muxterm_new 失败");

    unsafe {
        assert_eq!(muxterm_connect(h), 0);
        let mut ev = [CStateChange::default(); 128];
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(80));
            let _ = muxterm_poll_events(h, ev.as_mut_ptr(), ev.len() as i32);
        }

        let mut tabs = [CTab {
            id: 0,
            name: ptr::null(),
            is_active: 0,
        }; 8];
        let ntabs = muxterm_get_tabs(h, tabs.as_mut_ptr(), tabs.len() as i32);
        assert_eq!(ntabs, 2);
        let ids: Vec<u32> = tabs[..ntabs as usize].iter().map(|tab| tab.id).collect();
        let three_tab = ids
            .iter()
            .copied()
            .find(|id| tab_layout_shape(h, *id).0.len() == 3)
            .unwrap();
        let one_tab = ids
            .iter()
            .copied()
            .find(|id| tab_layout_shape(h, *id).0.len() == 1)
            .unwrap();

        let (_, three_shape) = tab_layout_shape(h, three_tab);
        let (_, one_shape) = tab_layout_shape(h, one_tab);
        assert!(matches!(
            &three_shape,
            LayoutShape::Split {
                type_: LAYOUT_SPLIT_H,
                second,
                ..
            } if matches!(second.as_ref(), LayoutShape::Split { type_: LAYOUT_SPLIT_V, .. })
        ));
        assert!(matches!(&one_shape, LayoutShape::Leaf(_)));

        for (cols, rows) in [(120, 36), (90, 24), (140, 40), (100, 30)] {
            assert_eq!(muxterm_resize_client(h, cols, rows), 0);
            for _ in 0..8 {
                std::thread::sleep(Duration::from_millis(50));
                let _ = muxterm_poll_events(h, ev.as_mut_ptr(), ev.len() as i32);
            }

            switch_tab(h, three_tab);
            let (three_ids, shape_after_resize) = tab_layout_shape(h, three_tab);
            assert_eq!(three_ids.len(), 3);
            assert_eq!(shape_after_resize, three_shape);

            switch_tab(h, one_tab);
            let (one_ids, one_shape_after_resize) = tab_layout_shape(h, one_tab);
            assert_eq!(one_ids.len(), 1);
            assert_eq!(one_shape_after_resize, one_shape);
        }

        assert_eq!(muxterm_shutdown(h), 0);
        muxterm_free(h);
    }

    let _ = Command::new("tmux")
        .args(["-L", &backend, "kill-server"])
        .status();
}
