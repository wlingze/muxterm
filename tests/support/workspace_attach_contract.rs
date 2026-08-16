#![allow(dead_code)]
//! 跨平台 attach 契约（W13 / 1820.log）。
//!
//! **无 GTK。** Linux / macOS / Windows 的 Surface 测试都走这套夹具：
//! 先在隔离 tmux 里铺好 2 tab / 3 pane 和 token，再让客户端 attach。
//! 空 session 立刻 echo 不算 attach 保真。

use std::time::Duration;

use super::tmux_test_support::*;

/// attach 后等待拓扑/播种的上限。
pub const ATTACH_TIMEOUT: Duration = Duration::from_secs(8);

/// 可见 pane 控件最小宽/高（像素）。0 就是白屏。
#[allow(dead_code)]
pub const MIN_PANE_PX: i32 = 40;

/// 洪水 1s 内允许的 `PaneOutput` 事件数。超过必须 pause。
/// 1820.log 单 pane 约 1000 事件/秒且 0 pause。
pub const MAX_OUTPUT_EVENTS_PER_SEC: usize = 400;

/// CUP 洪水帧数（隔离 socket，不要对着用户 server）。
pub const CUP_FLOOD_FRAMES: u32 = 400;

/// 已涂 token 的 2tab / 3pane 工作区。Drop 时 `kill-server -L`。
pub struct PaintedWorkspace {
    pub socket: String,
    pub session: String,
    /// tab 1（3 pane）的 tmux pane 数字 id（无 `%`）。
    pub tab1_panes: [u32; 3],
    /// tab 2 的唯一 pane。
    #[allow(dead_code)]
    pub tab2_pane: u32,
    pub tab1_tokens: [String; 3],
    pub tab2_token: String,
}

impl Drop for PaintedWorkspace {
    fn drop(&mut self) {
        kill_server(&self.socket);
    }
}

impl PaintedWorkspace {
    pub fn pane_target(&self, id: u32) -> String {
        format!("%{id}")
    }
}

/// 先建拓扑和画面，**不**启动 Muxterm。
pub fn build_painted_2tab_3pane(label: &str) -> PaintedWorkspace {
    assert!(tmux_available(), "需要本机 tmux");
    let socket = unique_socket(label);
    let session = format!("att-{label}");
    let suffix = rand_nanos();

    // 独立 server：第一个 pane 直接 /bin/cat。
    let output = std::process::Command::new("tmux")
        .args([
            "-L",
            &socket,
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            &session,
            "-x",
            "80",
            "-y",
            "24",
            "--",
            "/bin/cat",
        ])
        .output()
        .expect("new-session");
    assert!(
        output.status.success(),
        "new-session 失败: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    tmux_ok(
        &socket,
        &["split-window", "-t", &session, "-h", "--", "/bin/cat"],
    );
    tmux_ok(
        &socket,
        &["split-window", "-t", &session, "-v", "--", "/bin/cat"],
    );
    tmux_ok(
        &socket,
        &[
            "new-window",
            "-t",
            &session,
            "-n",
            "other",
            "--",
            "/bin/cat",
        ],
    );
    // attach 必须落在 3 pane 那一页，而不是刚创建的 other。
    tmux_ok(&socket, &["select-window", "-t", &format!("{session}:0")]);

    let tab1 = list_pane_ids(&socket, &format!("{session}:0"));
    let tab2 = list_pane_ids(&socket, &format!("{session}:1"));
    assert_eq!(tab1.len(), 3, "tab 1 应有 3 pane: {tab1:?}");
    assert_eq!(tab2.len(), 1, "tab 2 应有 1 pane: {tab2:?}");

    let tab1_tokens = [
        format!("T1P0_{suffix}"),
        format!("T1P1_{suffix}"),
        format!("T1P2_{suffix}"),
    ];
    let tab2_token = format!("T2P0_{suffix}");

    for (id, token) in tab1.iter().zip(tab1_tokens.iter()) {
        let target = format!("%{id}");
        send_keys_literal(&socket, &target, token);
        wait_capture_contains(&socket, &target, token, Duration::from_secs(3));
    }
    {
        let target = format!("%{}", tab2[0]);
        send_keys_literal(&socket, &target, &tab2_token);
        wait_capture_contains(&socket, &target, &tab2_token, Duration::from_secs(3));
    }

    PaintedWorkspace {
        socket,
        session,
        tab1_panes: [tab1[0], tab1[1], tab1[2]],
        tab2_pane: tab2[0],
        tab1_tokens,
        tab2_token,
    }
}

fn rand_nanos() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(1)
}

/// core `State`：2 tab、当前 tab 3 leaf、四个 token 都在某个 pane_output 里。
#[allow(dead_code)]
pub fn assert_core_painted_topology(
    state: &dyn muxterm::core::model::state::State,
    ws: &PaintedWorkspace,
) {
    let tabs = state.tabs();
    assert!(
        tabs.len() >= 2,
        "attach 后应有 ≥2 个 tab，实际 {}: {:?}",
        tabs.len(),
        tabs.iter().map(|t| t.id.0).collect::<Vec<_>>()
    );

    let active = tabs
        .iter()
        .find(|t| t.active)
        .or_else(|| tabs.first())
        .expect("应有 tab");
    let leaves = state
        .layout(&active.id)
        .map(|l| l.tree.leaves())
        .unwrap_or_default();
    assert_eq!(
        leaves.len(),
        3,
        "当前 tab 布局应有 3 leaf，实际 {leaves:?}（1820 错布局）"
    );

    let mut blob = String::new();
    for tab in &tabs {
        for pane in state.panes(&tab.id) {
            if let Some(bytes) = state.pane_output(&pane.id) {
                blob.push_str(&String::from_utf8_lossy(bytes));
            }
        }
    }
    for token in ws.tab1_tokens.iter().chain(std::iter::once(&ws.tab2_token)) {
        assert!(
            blob.contains(token),
            "core pane_output 应含播种 token {token}（attach 白屏 = 快照没进缓冲）。blob 长度 {}",
            blob.len()
        );
    }
}

/// 数一批事件里的 PaneOutput。
#[allow(dead_code)]
pub fn count_pane_output_events(events: &[muxterm::core::model::state::StateChange]) -> usize {
    events
        .iter()
        .filter(|e| {
            matches!(
                e,
                muxterm::core::model::state::StateChange::PaneOutput { .. }
            )
        })
        .count()
}
