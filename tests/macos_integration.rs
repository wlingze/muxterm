#![cfg(feature = "ffi")]

//! macOS 客户端同源 FFI 集成测试：2tab3pane tmux session attach 后校验布局。
//!
//! 与 `tests/tui_integration.rs` 的 `setup_tmux_backend_2tab` 同构：
//! tab1 = 3 panes（水平 split + 右侧竖直 split），tab2 = 1 pane。
//!
//! 跑：`cargo test --no-default-features --features ffi --test macos_integration`
//!
//! 注意：FFI 约定 `tab_id == 0` 表示「当前 active tab」，因此查询时先 SwitchTab 再传 0。

use std::ffi::CString;
use std::process::Command;
use std::ptr;
use std::time::Duration;

use muxterm::core::ffi::api::{
    muxterm_connect, muxterm_execute, muxterm_free, muxterm_get_layout, muxterm_get_panes,
    muxterm_get_tabs, muxterm_new, muxterm_poll_events, muxterm_shutdown,
};
use muxterm::core::ffi::types::{
    CLayoutNode, CPane, CStateChange, CTab, CTask, LAYOUT_LEAF, LAYOUT_SPLIT_H, LAYOUT_SPLIT_V,
    TASK_SWITCH_TAB,
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

unsafe fn switch_tab(h: *mut muxterm::core::ffi::api::MuxtermHandle, tab_id: u32) {
    let task = CTask {
        type_: TASK_SWITCH_TAB,
        target_pane: 0,
        target_tab: tab_id,
        dir: 0,
        name: ptr::null(),
    };
    assert_eq!(muxterm_execute(h, &task), 0);
    let mut ev = [CStateChange::default(); 64];
    for _ in 0..5 {
        std::thread::sleep(Duration::from_millis(80));
        let _ = muxterm_poll_events(h, ev.as_mut_ptr(), 64);
    }
}

unsafe fn active_pane_count(h: *mut muxterm::core::ffi::api::MuxtermHandle) -> i32 {
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
    h: *mut muxterm::core::ffi::api::MuxtermHandle,
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
