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
use std::time::Duration;

use muxterm::core::protocol::ffi::api::{
    muxterm_connect, muxterm_execute, muxterm_free, muxterm_get_layout, muxterm_get_panes,
    muxterm_get_tabs, muxterm_new, muxterm_poll_events, muxterm_shutdown,
};
use muxterm::core::protocol::ffi::types::{
    CLayoutNode, CPane, CStateChange, CTab, CTask, DIR_VERTICAL, LAYOUT_LEAF, LAYOUT_SPLIT_H,
    LAYOUT_SPLIT_V, TASK_NEW_TAB, TASK_SPLIT_PANE, TASK_SWITCH_TAB,
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
