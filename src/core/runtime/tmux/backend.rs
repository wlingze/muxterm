//! TmuxRuntime：tmux -CC 控制模式后端。
//!
//! 封装现有 `runtime::tmux::client`（spawn tmux -CC + 事件流）和
//! `runtime::tmux::command`（强类型命令构造器），实现 `Runtime` trait。
//!
//! 设计：
//! - `connect()`：spawn tmux -CC new-session，drain 启动事件建立初始 state
//!   （session / 第一个 window / 第一个 pane）
//! - 后台 task 持续读 `TmuxEvent`，把 `Message` 转成内部 state 更新 +
//!   `StateChange` 事件入队；命令响应行（ResponseLine）暂不处理
//! - `execute(Task)`：把 Task 映射成 `TmuxCommand`，通过命令 channel 发给
//!   后台 sender task 异步 `send_command`（execute 本身是同步 fn）
//! - `take_events()`：drain 内部事件队列
//! - State 视图从内部 state 读
//!
//! 与 ShellRuntime 不同：状态变化由 tmux 推送的事件驱动，execute 只发命令，
//! 不立即改 state（tmux 会回推 LayoutChange/PaneModeChanged 等通知）。

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::core::buffer_cap::{append_capped, MAX_PANE_OUTPUT_BYTES, MAX_STATE_EVENTS};
use crate::core::model::backend::{Runtime, RuntimeCapability};
use crate::core::model::layout::{LayoutNode, SplitDir, TabLayout};
use crate::core::model::state::{BackendStatus, PaneInfo, State, StateChange, TabInfo};
use crate::core::model::task::{Task, TaskOutcome};
use crate::core::runtime::tmux::client::{
    ConnectMode, TmuxClient, TmuxClientConfig, TmuxClientHandle, TmuxEvent,
};
use crate::core::runtime::tmux::command as cmd;
use crate::core::runtime::tmux::protocol::{
    parse_layout_tree, LayoutTree, Message, NotificationKind, TmuxSessionId,
};
use crate::core::types::{PaneId, TabId};

/// 后台命令查询标记：记录发出去的命令，收到 %end 时处理响应行。
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum PendingQuery {
    /// 非查询命令的响应占位；避免 split/send-keys 的 `%end` 消耗后续查询。
    Ignore,
    /// 新建 attach pane 的 readiness probe（send Enter）。
    ReadyProbe { pane: PaneId },
    /// list-panes -t <tab> -F '...'：解析所有 pane（pane_id, tab_id, active, cols, rows）。
    ListPanes { tab: TabId },
    /// list-windows -t <session> -F '...'：解析所有 window（window_id, name, active, layout, panes）。
    ListWindows,
    /// display-message -p -t <pane> '<format>'：取单行响应。
    DisplayMessage { pane: PaneId },
    /// capture-pane -e -p -t <pane>：恢复 attach 时 tmux 已存在的可见屏幕。
    CapturePane { pane: PaneId },
    /// display-message format：查询 resync 时需要重放的 VT 状态。
    PaneResyncState { pane: PaneId },
    /// resync 的 primary/alternate capture。
    PaneResyncCapture { pane: PaneId, alternate: bool },
    /// list-sessions：列出 tmux server 上所有 session。
    ListSessions,
}

/// status bar 订阅名（文档 §B+：`refresh-client -B` 的名字）。
pub const STATUS_LEFT_SUBSCRIPTION: &str = "muxterm.status-left";
const PANE_CMD_SUBSCRIPTION: &str = "muxterm.pane-cmd";
pub const STATUS_RIGHT_SUBSCRIPTION: &str = "muxterm.status-right";

/// 事件队列在一个 pane 上积压到这个字节数时进入 snapshot/resync。
///
/// tmux control mode 的 pause 并不是可靠的 ring buffer：`control.c` 会丢掉
/// pause 期间尚未发送的 blocks，随后 continue 直接从当前尾部继续。因此
/// 不能只靠“暂停后合并剩余字节”，必须用 capture-pane + pane state 重新对齐。
const RESYNC_BACKLOG_BYTES: usize = 256 * 1024;
/// 输出长期没有被 GUI 消费也要触发 resync，即使单次 chunk 很小。
const RESYNC_BACKLOG_AGE: Duration = Duration::from_millis(250);
/// 即使 GUI 每轮都及时 drain，持续的高字节速率也会让 VTE 逐帧落后。
/// 在短窗口内超过此值时切换成 snapshot，避免只按事件队列大小判断。
const RESYNC_WINDOW_BYTES: usize = 64 * 1024;
const RESYNC_WINDOW: Duration = Duration::from_millis(250);

/// 单个 pane 的输出流控状态。
#[derive(Debug, Default)]
struct PaneFlow {
    /// 外部 `%pause` 且无法发起查询时的兼容缓冲。
    suppressed: Vec<u8>,
    /// 当前事件队列中最早一个 live output 的时间。
    queued_at: Option<Instant>,
    /// 正在进行 authoritative snapshot transaction。
    resyncing: bool,
    /// 连续 burst 流量（只在真正静默后清零；不能按每轮 poll 清零）。
    last_output_at: Option<Instant>,
    window_bytes: usize,
    overload: bool,
}

/// tmux pane 的终端状态（由 display-message format 查询）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PaneReplayState {
    cursor_x: u32,
    cursor_y: u32,
    cursor_flag: bool,
    cursor_shape: String,
    cursor_blinking: Option<bool>,
    alternate_on: bool,
    alternate_saved_x: Option<u32>,
    alternate_saved_y: Option<u32>,
    insert_flag: bool,
    wrap_flag: bool,
    keypad_flag: bool,
    keypad_cursor_flag: bool,
    origin_flag: bool,
    mouse_all_flag: bool,
    mouse_any_flag: bool,
    mouse_button_flag: bool,
    mouse_sgr_flag: bool,
    mouse_standard_flag: bool,
    mouse_utf8_flag: bool,
    bracket_paste_flag: bool,
}

/// 一次 pane snapshot/resync 事务。所有 live output 都暂存在事务里，直到
/// primary + alternate capture 完成，确保前端看到的只有一个可重放边界。
#[derive(Debug, Default)]
struct PaneResync {
    state: Option<PaneReplayState>,
    primary: Option<Vec<u8>>,
    alternate: Option<Vec<u8>>,
    live: Vec<u8>,
}

/// tmux -CC 后端。
pub struct TmuxRuntime {
    config: TmuxClientConfig,
    handle: Option<TmuxClientHandle>,
    event_rx: Option<mpsc::Receiver<TmuxEvent>>,
    /// 命令发送 channel：execute 把 TmuxCommand 字符串塞进来，
    /// 后台 sender task 异步 send_command。
    cmd_tx: Option<mpsc::UnboundedSender<String>>,
    /// 后台事件回流 task 的 join handle（用于 shutdown 时 abort）。
    _pump_handle: Option<tokio::task::JoinHandle<()>>,
    _sender_handle: Option<tokio::task::JoinHandle<()>>,
    /// sender task 的异步写错误；由前端轮询成可见状态事件。
    command_error_rx: Option<mpsc::UnboundedReceiver<String>>,
    /// SSH 读写字节计数（spawn_ssh 时从 handle 克隆）。
    traffic: Option<crate::core::transport::TrafficCounters>,

    // ── 内部 state ──────────────────────────────────────────
    /// 当前 bind 的 tmux session 名（= Workspace 名）。
    workspace_name: String,
    active_session: Option<TmuxSessionId>,
    /// list-sessions 查询到的 server 上全部 session（供池层发现）。
    known_sessions: Vec<(TmuxSessionId, String)>,
    tabs: Vec<TabInfo>,
    panes: Vec<PaneInfo>,
    layouts: HashMap<TabId, TabLayout>,
    /// pane 累积输出。
    outputs: HashMap<PaneId, Vec<u8>>,
    /// attach 初始 capture 的历史行数（W16a：`capture-pane -S -N`）。
    scrollback_lines: u32,

    status: BackendStatus,
    events: VecDeque<StateChange>,

    /// 当前命令响应累积的行（%begin..%end 之间），带 number 标识。
    response_accum: HashMap<i64, Vec<String>>,
    /// 等待响应的命令回调（number → 处理函数）。简化为存命令类型标记。
    pending_queries: VecDeque<PendingQuery>,
    /// `%begin <number>` 到达时从 pending_queries 队首取出的查询，按 number 登记。
    ///
    /// tmux 控制模式是串行的，但高输出下 `%begin/%end` 仍可能与多个在途查询
    /// 交叠。按 number 匹配能避免用简单的 FIFO `pop_front` 错配查询。
    pending_by_number: HashMap<i64, PendingQuery>,
    /// 缓存每个 tab（tmux window）的 layout 字符串（从 list-windows 响应获取），用于重建 LayoutNode。
    window_layouts: HashMap<TabId, String>,
    /// 当前处于 zoom（`resize-pane -Z` / prefix-z）的 tab。
    /// 此时 tmux 的 `window_layout` 仍是完整 split 树，GUI 必须只渲染 active pane。
    window_zoomed: HashSet<TabId>,
    /// 每个 tab 的 pane 数量（从 list-windows 响应获取），用于确认所有 pane 查询完成。
    expected_panes_per_window: HashMap<TabId, usize>,
    /// 已收到 `%window-close` 但尚未经权威 `list-windows` 确认的 tab。
    ///
    /// tmux `move-window` 会先 unlink 再 link 窗口，控制模式下可能产生
    /// `%window-add` / `%session-window-changed` / `%window-close` 组合，且
    /// close 通知偶尔会晚于 `list-windows` 响应到达。此时不能立即删 tab，
    /// 否则权威响应已确认存在的窗口会被迟到的 close 永久删掉（tab 丢失）。
    /// 收到 close 后发起一次权威查询，响应里存在的 tab 取消关闭，真正不存在
    /// 的才发 TabClosed/PaneClosed。
    pending_close_tabs: HashSet<TabId>,
    /// attach 初始快照查询中的 pane。初始 `%output` 不能先喂给前端，
    /// 否则随后 capture-pane 只能追加，已有屏幕内容会重复或缺失。
    initial_capture_pending: HashSet<PaneId>,
    /// 已完成 attach 初始快照的 pane；之后的 `%output` 才是实时增量。
    initial_capture_done: HashSet<PaneId>,
    /// 被 `%pause` 暂停输出的 pane（`%continue` 恢复；供背压/诊断）。
    paused_panes: HashSet<PaneId>,
    /// 每个 pane 的输出速率窗口（洪峰 pause / 合并）。
    flow: HashMap<PaneId, PaneFlow>,
    /// 正在进行的 authoritative pane snapshot transaction。
    resyncs: HashMap<PaneId, PaneResync>,
    /// attach 初始快照查询进行期间到达的实时 `%output` 缓冲。
    ///
    /// capture-pane 返回的是查询瞬间的完整屏幕；在「发出 capture-pane」到「收到
    /// 响应」之间的窗口里 shell 若产生输出，tmux 会继续发 `%output`，这些增量如果
    /// 直接丢弃会丢数据。这里暂存它们，快照返回后拼接到快照尾部，从而既保留完整
    /// 屏幕又不错过查询期间的实时增量。
    initial_capture_buf: HashMap<PaneId, Vec<u8>>,
    /// `%begin` 之后、capture 响应结束之前到达的输出。这个边界由 tmux
    /// control-mode response number 定义，不再用内容子串猜测重复。
    initial_capture_tail: HashMap<PaneId, Vec<u8>>,
    /// attach 新建 pane 尚未完成首个 capture 时暂存的用户输入。
    ///
    /// tmux 会先报告新 window/pane，再异步启动 pane 内的 shell。若在
    /// capture 边界建立前直接 send-keys，输入可能落在 shell 启动窗口而被
    /// 丢弃；等权威快照完成后再发，保证 attach → new pane 的输入无损。
    pending_writes: HashMap<PaneId, Vec<u8>>,
    /// capture 完成后至少跨过一个 backend poll 才允许输入。
    ///
    /// capture 的 prompt 已经可见并不代表 pane 的 shell 已经完成启动；
    /// 延后一轮把输入放到下一批 control-mode 命令，避免首个字符落在
    /// shell 初始化窗口。
    deferred_write_panes: HashSet<PaneId>,
    /// attach 初始 state 完成后新建 pane 的 shell readiness probe。
    attach_bootstrap_complete: bool,
    awaiting_pane_ready: HashSet<PaneId>,
    ready_probe_at: HashMap<PaneId, Instant>,
    ready_probe_in_flight: HashSet<PaneId>,
    ready_probe_acknowledged: HashSet<PaneId>,
    ready_probe_rounds: HashMap<PaneId, u8>,
    new_attach_panes: HashSet<PaneId>,
    /// 已经看到 capture 查询 `%begin` 的 pane。`initial_capture_buf` 中的
    /// 字节可能是请求前排队的通知，成功 capture 时不应再次重放；tail 则
    /// 明确属于响应边界之后，必须保留。
    capture_response_seen: HashSet<PaneId>,
    /// 是否支持 `refresh-client -r`（OSC 10/11 颜色上报；tmux < 3.2 不支持）。
    colour_report_supported: bool,
    colour_report_warned: bool,
    /// 是否支持 `refresh-client -B`（status bar 订阅；tmux ≥ 3.2，文档 §B+）。
    status_subscription_supported: bool,
    /// 已成功发出 status-left/right 订阅（前端据此关闭轮询定时器）。
    status_subscriptions_active: bool,
}

/// 解析 `tmux -V` 输出（如 `tmux 3.7b` / `tmux 2.9a`）。
pub fn parse_tmux_version(text: &str) -> Option<(u32, u32)> {
    let head = text.split_whitespace().nth(1)?;
    let digits: String = head
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut parts = digits.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// `refresh-client -r`（颜色上报）需要 tmux >= 3.2。
pub fn supports_colour_report(version: Option<(u32, u32)>) -> bool {
    match version {
        None => true, // 版本未知时尝试上报，失败由命令错误自然暴露
        Some((major, minor)) => major > 3 || (major == 3 && minor >= 2),
    }
}

/// `refresh-client -B`（format 订阅）需要 tmux >= 3.2（文档 §B+）。
pub fn supports_status_subscription(version: Option<(u32, u32)>) -> bool {
    matches!(version, Some((major, minor)) if major > 3 || (major == 3 && minor >= 2))
}

/// capture-pane 响应 → 终端字节流。
///
/// 按行还原可见屏幕；去掉尾部纯空白行，并且不在最后补 CRLF。否则新创建的
/// SwiftTerm 从 (0,0) 开始喂入，尾部空白行会被当成换行把光标推到 pane
/// 最底部（「新 pane 的 shell 在最下面」），而实际 tmux 光标仍在 prompt
/// 那一行。
fn capture_pane_bytes(lines: &[String]) -> Vec<u8> {
    let mut end = lines.len();
    while end > 0 && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    lines[..end].join("\r\n").into_bytes()
}

/// tmux format fields needed to restore terminal modes after capture-pane.
/// Keep this as one line so a response is unambiguous even when pane_tabs or
/// a future value is empty.
const PANE_RESYNC_FORMAT: &str = concat!(
    "#{cursor_x}|#{cursor_y}|#{cursor_flag}|#{cursor_shape}|#{cursor_blinking}|",
    "#{alternate_on}|#{alternate_saved_x}|#{alternate_saved_y}|#{insert_flag}|",
    "#{wrap_flag}|#{keypad_flag}|#{keypad_cursor_flag}|#{origin_flag}|",
    "#{mouse_all_flag}|#{mouse_any_flag}|#{mouse_button_flag}|#{mouse_sgr_flag}|",
    "#{mouse_standard_flag}|#{mouse_utf8_flag}|#{bracket_paste_flag}"
);

fn parse_bool_field(fields: &[&str], index: usize) -> bool {
    fields
        .get(index)
        .is_some_and(|v| *v == "1" || *v == "on" || *v == "true")
}

fn parse_optional_u32(fields: &[&str], index: usize) -> Option<u32> {
    fields
        .get(index)
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|value| *value != u32::MAX)
}

fn parse_pane_replay_state(line: &str) -> PaneReplayState {
    let fields: Vec<&str> = line.trim().split('|').collect();
    PaneReplayState {
        cursor_x: fields.first().and_then(|v| v.parse().ok()).unwrap_or(0),
        cursor_y: fields.get(1).and_then(|v| v.parse().ok()).unwrap_or(0),
        cursor_flag: parse_bool_field(&fields, 2),
        cursor_shape: fields.get(3).copied().unwrap_or_default().to_string(),
        cursor_blinking: fields.get(4).and_then(|v| match *v {
            "1" | "on" | "true" => Some(true),
            "0" | "off" | "false" => Some(false),
            _ => None,
        }),
        alternate_on: parse_bool_field(&fields, 5),
        alternate_saved_x: parse_optional_u32(&fields, 6),
        alternate_saved_y: parse_optional_u32(&fields, 7),
        insert_flag: parse_bool_field(&fields, 8),
        wrap_flag: parse_bool_field(&fields, 9),
        keypad_flag: parse_bool_field(&fields, 10),
        keypad_cursor_flag: parse_bool_field(&fields, 11),
        origin_flag: parse_bool_field(&fields, 12),
        mouse_all_flag: parse_bool_field(&fields, 13),
        mouse_any_flag: parse_bool_field(&fields, 14),
        mouse_button_flag: parse_bool_field(&fields, 15),
        mouse_sgr_flag: parse_bool_field(&fields, 16),
        mouse_standard_flag: parse_bool_field(&fields, 17),
        mouse_utf8_flag: parse_bool_field(&fields, 18),
        bracket_paste_flag: parse_bool_field(&fields, 19),
    }
}

fn push_csi(out: &mut Vec<u8>, body: &str) {
    out.extend_from_slice(b"\x1b[");
    out.extend_from_slice(body.as_bytes());
}

/// 把 capture-pane 内容和 tmux 的 pane state 组成一个可一次性喂给 VT 的
/// snapshot。capture-pane 只给 grid cells，不包含 cursor/mode，所以这里补
/// 上 alternate screen、cursor、mouse、wrap/origin 等状态。
fn build_pane_snapshot(
    state: Option<&PaneReplayState>,
    primary: &[u8],
    alternate: &[u8],
    live: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(primary.len() + alternate.len() + live.len() + 128);
    out.extend_from_slice(primary);
    if let Some(state) = state {
        if state.alternate_on {
            // DEC 1049 enters the saved-cursor alternate screen and clears it;
            // capture-pane -a below then paints the authoritative alternate grid.
            if let (Some(saved_x), Some(saved_y)) =
                (state.alternate_saved_x, state.alternate_saved_y)
            {
                let x = saved_x.saturating_add(1);
                let y = saved_y.saturating_add(1);
                push_csi(&mut out, &format!("{y};{x}H"));
            }
            out.extend_from_slice(b"\x1b[?1049h");
            out.extend_from_slice(alternate);
        }

        let x = state.cursor_x.saturating_add(1);
        let y = state.cursor_y.saturating_add(1);
        push_csi(&mut out, &format!("{y};{x}H"));
        push_csi(&mut out, if state.cursor_flag { "?25h" } else { "?25l" });
        if let Some(blinking) = state.cursor_blinking {
            push_csi(&mut out, if blinking { "?12h" } else { "?12l" });
        }
        if !state.cursor_shape.is_empty() {
            let shape = match state.cursor_shape.as_str() {
                "block" | "default" => "1",
                "underline" => "3",
                "bar" | "vertical" => "5",
                _ => "1",
            };
            push_csi(&mut out, &format!("{shape} q"));
        }
        push_csi(&mut out, if state.insert_flag { "4h" } else { "4l" });
        push_csi(&mut out, if state.wrap_flag { "?7h" } else { "?7l" });
        push_csi(&mut out, if state.origin_flag { "?6h" } else { "?6l" });
        out.extend_from_slice(if state.keypad_flag {
            b"\x1b="
        } else {
            b"\x1b>"
        });
        push_csi(
            &mut out,
            if state.keypad_cursor_flag {
                "?1h"
            } else {
                "?1l"
            },
        );
        if state.bracket_paste_flag {
            push_csi(&mut out, "?2004h");
        } else {
            push_csi(&mut out, "?2004l");
        }

        // Reset the mouse modes first, then replay exactly the flags tmux reports.
        for mode in [1000, 1002, 1003, 1005, 1006] {
            push_csi(&mut out, &format!("?{mode}l"));
        }
        if state.mouse_standard_flag || state.mouse_button_flag || state.mouse_any_flag {
            push_csi(
                &mut out,
                if state.mouse_any_flag {
                    "?1003h"
                } else if state.mouse_button_flag {
                    "?1002h"
                } else {
                    "?1000h"
                },
            );
        }
        if state.mouse_utf8_flag {
            push_csi(&mut out, "?1005h");
        }
        if state.mouse_sgr_flag {
            push_csi(&mut out, "?1006h");
        }
    } else if !alternate.is_empty() {
        // Older tmux may not expose the format fields. Keep the captured
        // alternate content visible rather than silently dropping it.
        out.extend_from_slice(b"\x1b[?1049h");
        out.extend_from_slice(alternate);
    }
    out.extend_from_slice(live);
    out
}

impl TmuxRuntime {
    // ── 层级映射（docs/LAYER-MAPPING.md 权威定义）──────────
    //
    // muxterm: Workspace → Tab → Pane  (3 层)
    // tmux:    session → window → pane    (3 层)
    //
    // 映射规则：
    //   tmux session  → Workspace（按名字 bind 一条 session）
    //   tmux window   → muxterm Tab      (1:1)  ← tmux window = muxterm Tab
    //   tmux pane     → muxterm Pane     (1:1)
    //
    // 因此：
    //   self.tabs 的每个 tab.id = TabId(tmux_window_index)
    //   self.panes 的每个 pane.tab = TabId(tmux_window_index)
    //   list-windows 返回 N 个 Tab（对应 tmux 的 N 个 window）
    /// 创建后端（尚未 connect）。socket 非空时隔离 tmux server（`-L`）。
    pub fn new(socket: Option<&str>) -> Self {
        let mut extra_args: Vec<String> = Vec::new();
        if let Some(s) = socket {
            let s = s.trim();
            if !s.is_empty() {
                extra_args.push("-L".into());
                extra_args.push(s.to_string());
            }
        }
        Self {
            config: TmuxClientConfig {
                mode: None,
                extra_args,
                tmux_bin: None,
                cols: Some(80),
                rows: Some(24),
                event_buffer: 0,
                ssh_alias: None,
            },
            handle: None,
            event_rx: None,
            cmd_tx: None,
            _pump_handle: None,
            _sender_handle: None,
            command_error_rx: None,
            traffic: None,
            workspace_name: String::new(),
            active_session: None,
            known_sessions: vec![],
            tabs: vec![],
            panes: vec![],
            layouts: HashMap::new(),
            outputs: HashMap::new(),
            scrollback_lines: 10_000,
            status: BackendStatus::Disconnected,
            events: VecDeque::new(),
            response_accum: HashMap::new(),
            pending_queries: VecDeque::new(),
            pending_by_number: HashMap::new(),
            window_layouts: HashMap::new(),
            window_zoomed: HashSet::new(),
            expected_panes_per_window: HashMap::new(),
            pending_close_tabs: HashSet::new(),
            initial_capture_pending: HashSet::new(),
            initial_capture_done: HashSet::new(),
            paused_panes: HashSet::new(),
            flow: HashMap::new(),
            resyncs: HashMap::new(),
            initial_capture_buf: HashMap::new(),
            initial_capture_tail: HashMap::new(),
            pending_writes: HashMap::new(),
            deferred_write_panes: HashSet::new(),
            attach_bootstrap_complete: false,
            awaiting_pane_ready: HashSet::new(),
            ready_probe_at: HashMap::new(),
            ready_probe_in_flight: HashSet::new(),
            ready_probe_acknowledged: HashSet::new(),
            ready_probe_rounds: HashMap::new(),
            new_attach_panes: HashSet::new(),
            capture_response_seen: HashSet::new(),
            colour_report_supported: true,
            colour_report_warned: false,
            status_subscription_supported: false,
            status_subscriptions_active: false,
        }
    }

    /// 创建后端并指定 attach 模式（连接已有 tmux session）。
    ///
    /// `target` 是 tmux session 名或 id（如 "demo" 或 "$0"）。
    pub fn new_with_attach(socket: Option<&str>, target: &str) -> Self {
        let mut backend = Self::new(socket);
        backend.config.mode = Some(ConnectMode::Attach {
            target: Some(target.to_string()),
        });
        backend
    }

    /// 创建远程 SSH tmux 后端并 attach 到已有 session。
    ///
    /// SSH 的读写、pty 和 tmux -CC 参数仍由 `TmuxClient::spawn_ssh` 统一处理，
    /// 这里仅把 alias 写入客户端配置，避免平台前端自行解析控制协议。
    pub fn new_with_ssh_attach(alias: &str, target: &str) -> Self {
        let mut backend = Self::new(None);
        backend.config.ssh_alias = Some(alias.to_string());
        backend.config.mode = Some(ConnectMode::Attach {
            target: Some(target.to_string()),
        });
        backend
    }

    /// 创建后端并指定 new-session 模式 + session 名。
    pub fn new_with_session_name(socket: Option<&str>, name: &str) -> Self {
        let mut backend = Self::new(socket);
        backend.config.mode = Some(ConnectMode::NewSession {
            name: Some(name.to_string()),
            start_directory: None,
        });
        backend
    }

    /// 解析 SSH attach 的 Host alias 与远端 tmux `-L`。
    ///
    /// `muxterm_new("ssh", socket=alias, session)` 没有单独 alias 参数，
    /// 此时 `socket` **就是** `~/.ssh/config` Host 名，禁止再当成远端 `-L`
    /// （否则会变成日志里的 `ssh ryzen -- tmux -L ryzen -CC attach`）。
    /// `muxterm_new_connect` 才把 `ssh_alias` 和 `-L socket` 分开。
    pub fn ssh_alias_and_tmux_socket(
        sock: Option<&str>,
        alias: Option<&str>,
    ) -> Option<(String, Option<String>)> {
        let nonempty = |s: &str| {
            let trimmed = s.trim();
            (!trimmed.is_empty()).then_some(trimmed.to_string())
        };
        let alias = alias.and_then(nonempty);
        let sock = sock.and_then(nonempty);
        match (alias, sock) {
            (Some(a), Some(s)) if a == s => Some((a, None)),
            (Some(a), s) => Some((a, s)),
            (None, Some(a)) => Some((a, None)),
            (None, None) => None,
        }
    }

    /// 通过 SSH alias 在远端启动 tmux -CC（new-session 模式）。
    ///
    /// `ssh_alias` 是 `~/.ssh/config` 里的 Host 名；`socket` 是远端 tmux 的 `-L` socket 名（可选）。
    pub fn new_ssh(ssh_alias: &str, socket: Option<&str>) -> Self {
        let mut backend = Self::new(socket);
        backend.config.ssh_alias = Some(ssh_alias.to_string());
        backend
    }

    /// 通过 SSH alias 在远端 attach 已有 session。
    pub fn new_ssh_attach(ssh_alias: &str, socket: Option<&str>, target: &str) -> Self {
        let mut backend = Self::new_ssh(ssh_alias, socket);
        backend.config.mode = Some(ConnectMode::Attach {
            target: Some(target.to_string()),
        });
        backend
    }

    /// 设置 attach 初始 capture 的历史行数（W16a）。
    pub fn set_scrollback_lines(&mut self, lines: u32) {
        self.scrollback_lines = lines.max(1);
    }

    /// 设置 control client 的初始字符网格。
    ///
    /// tmux 在 `-CC` 启动时会用这个尺寸创建窗口；如果先用默认的
    /// 80x24 启动，Codex/Cursor/htop 的首帧会按错误的列数换行，随后即使
    /// 收到 resize 也可能把输入框和底栏留在错误的行。最终尺寸仍由前端
    /// `refresh-client -C` 校准，这里只负责让首屏尽量接近真实窗口。
    pub fn set_client_size(&mut self, cols: u16, rows: u16) {
        if cols >= 2 && rows >= 1 {
            self.config.cols = Some(u32::from(cols));
            self.config.rows = Some(u32::from(rows));
        }
    }

    fn active_tab_id(&self) -> Option<TabId> {
        self.tabs.iter().find(|tab| tab.active).map(|tab| tab.id)
    }

    fn tab_is_active(&self, tab: TabId) -> bool {
        self.active_tab_id() == Some(tab)
    }

    /// 为一个 tab 的所有 pane 发起一次性 Surface seed。
    ///
    /// attach 初始阶段只抓活动 tab，避免后台 tab 的 capture 把连接建立和
    /// Cmd-Shift-P 卡在大量串行响应上。切 tab 后由同一个入口按需补抓。
    fn query_capture_tab(&mut self, tab: TabId) {
        let panes: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|pane| pane.tab == tab)
            .map(|pane| pane.id)
            .collect();
        for pane in panes {
            self.query_capture_pane(pane);
        }
    }

    fn active_tab_topology_ready(&self) -> bool {
        let Some(tab) = self.active_tab_id() else {
            return false;
        };
        let Some(expected) = self.expected_panes_per_window.get(&tab) else {
            return false;
        };
        self.panes.iter().filter(|pane| pane.tab == tab).count() >= *expected
    }

    /// 测试用：当前 connect 模式（attach / new-session）。
    #[cfg(test)]
    pub fn test_connect_mode(&self) -> Option<&ConnectMode> {
        self.config.mode.as_ref()
    }

    /// 测试用：传给 tmux 的二进制级参数（含 `-L`）。
    #[cfg(test)]
    pub fn test_extra_args(&self) -> &[String] {
        &self.config.extra_args
    }

    /// 测试用：SSH Host alias。
    #[cfg(test)]
    pub fn test_ssh_alias(&self) -> Option<&str> {
        self.config.ssh_alias.as_deref()
    }

    /// 创建新 session，并指定起始工作目录（session 名由 tmux 自动生成）。
    pub fn new_with_cwd(socket: Option<&str>, start_directory: Option<&str>) -> Self {
        let mut backend = Self::new(socket);
        backend.config.mode = Some(ConnectMode::NewSession {
            name: None,
            start_directory: start_directory.map(|s| s.to_string()),
        });
        backend
    }

    /// 把指定 tab 标记为 active，并发出 ActiveTabChanged 事件。
    fn mark_tab_active(&mut self, tab_id: TabId) {
        if !self.tabs.iter().any(|t| t.id == tab_id) {
            return;
        }
        let current_active = self.tabs.iter().find(|t| t.active).map(|t| t.id);
        for t in self.tabs.iter_mut() {
            t.active = t.id == tab_id;
        }
        let target_pane = self
            .layouts
            .get(&tab_id)
            .map(|layout| layout.active)
            .filter(|pane| {
                self.panes
                    .iter()
                    .any(|candidate| candidate.id == *pane && candidate.tab == tab_id)
            });
        let old_active_pane = self
            .panes
            .iter()
            .find(|pane| pane.active)
            .map(|pane| pane.id);
        for pane in &mut self.panes {
            pane.active = Some(pane.id) == target_pane && pane.tab == tab_id;
        }
        if current_active != Some(tab_id) {
            self.events
                .push_back(StateChange::ActiveTabChanged { tab: tab_id });
        }
        if old_active_pane != target_pane {
            if let Some(pane) = target_pane {
                self.events
                    .push_back(StateChange::ActivePaneChanged { tab: tab_id, pane });
            }
        }
    }

    /// 目标 tab 的 pane 数据为空时重新查询（兜底）。
    fn query_panes_if_empty(&mut self, tab_id: TabId) {
        if !self.owns_tab(tab_id) {
            return;
        }
        let pane_count = self.panes.iter().filter(|p| p.tab == tab_id).count();
        if pane_count == 0 {
            tracing::debug!(
                target: "muxterm::tmux",
                "切 tab 到 @{} 但 pane 为空，重新查询",
                tab_id.0
            );
            self.query_list_panes(tab_id);
        }
    }

    fn owns_tab(&self, tab_id: TabId) -> bool {
        self.tabs.iter().any(|t| t.id == tab_id)
    }

    fn should_ignore_foreign_tab(&self, tab: TabId) -> bool {
        !self.tabs.is_empty() && !self.owns_tab(tab)
    }

    fn is_attached_session(&self, session: TmuxSessionId) -> bool {
        self.active_session == Some(session)
    }

    /// 事件队列过长时丢弃最旧的 PaneOutput，避免挂起轮询时涨到数 GB。
    ///
    /// 结构性事件（ActiveTabChanged / TabClosed / PaneClosed / PaneAdded /
    /// TabAdded / WindowClosed）**绝不丢弃**：它们驱动 UI 切 tab / 关闭 /
    /// 布局重建，被裁掉会让前端永远等不到确认而卡死（例如 `%session-window-
    /// changed` 在输出洪峰下被硬裁，macOS 的切 tab 门禁就再也不会放行）。
    /// 只丢 PaneOutput；仍超限时丢可重建的 LayoutChanged（push_layout_changed
    /// 本就会合并同一 tab 的旧布局）。
    fn trim_event_queue(&mut self) {
        // 第一优先：丢弃最旧的 PaneOutput（体积大、可丢弃、不影响状态机）。
        while self.events.len() > MAX_STATE_EVENTS {
            let Some(idx) = self
                .events
                .iter()
                .position(|e| matches!(e, StateChange::PaneOutput { .. }))
            else {
                break;
            };
            self.events.remove(idx);
        }
        // 仍超限：丢弃最旧的 LayoutChanged（可重建，前端只要最新布局）。
        while self.events.len() > MAX_STATE_EVENTS {
            let Some(idx) = self
                .events
                .iter()
                .position(|e| matches!(e, StateChange::LayoutChanged { .. }))
            else {
                break;
            };
            self.events.remove(idx);
        }
        // 极端情况仍超限（几乎全是关键结构事件）：宁可让队列暂时超一点，
        // 也绝不硬裁结构性事件——否则切 tab / 关闭会永久卡死。
    }

    /// 合并同一 tab 尚未交给前端的布局事件。
    ///
    /// 窗口 resize 会让 tmux 连续发送 layout-change；前端只需要最新完整
    /// layout。保留中间快照会让 GUI 反复重建 pane 树，表现为闪烁和比例跳动。
    fn push_layout_changed(&mut self, layout: TabLayout) {
        let tab = layout.tab;
        self.events.retain(
            |event| !matches!(event, StateChange::LayoutChanged { tab: old, .. } if *old == tab),
        );
        self.events
            .push_back(StateChange::LayoutChanged { tab, layout });
    }

    /// 记录一次 pane 输出。正常情况下原始字节逐块交付；一旦事件队列的
    /// byte backlog/age 超过阈值，`maybe_start_resyncs` 会移除旧增量并以
    /// authoritative snapshot 替换，避免把半截 ESC/CUP 帧喂给前端。
    fn note_pane_output(&mut self, pane: PaneId, content: &[u8]) {
        if self.resyncs.contains_key(&pane) {
            append_capped(
                &mut self.resyncs.entry(pane).or_default().live,
                content,
                MAX_PANE_OUTPUT_BYTES,
            );
            return;
        }
        let flow = self.flow.entry(pane).or_default();
        let now = Instant::now();
        if flow
            .last_output_at
            .is_none_or(|last| now.duration_since(last) >= RESYNC_WINDOW)
        {
            flow.window_bytes = 0;
            flow.overload = false;
        }
        flow.last_output_at = Some(now);
        flow.window_bytes = flow.window_bytes.saturating_add(content.len());
        if flow.window_bytes >= RESYNC_WINDOW_BYTES {
            flow.overload = true;
        }
        if flow.resyncing {
            append_capped(&mut flow.suppressed, content, MAX_PANE_OUTPUT_BYTES);
        } else {
            self.push_pane_output(pane, content.to_vec());
        }
    }

    /// 把一段 pane 字节追加进核心缓冲并产生一个 `PaneOutput` 事件。
    fn push_pane_output(&mut self, pane: PaneId, data: Vec<u8>) {
        if data.is_empty() {
            return;
        }
        append_capped(
            self.outputs.entry(pane).or_default(),
            &data,
            MAX_PANE_OUTPUT_BYTES,
        );
        self.events
            .push_back(StateChange::PaneOutput { pane, data });
        self.trim_event_queue();
        let flow = self.flow.entry(pane).or_default();
        flow.queued_at.get_or_insert_with(Instant::now);
    }

    /// 收到 `%pause`/`%continue` 时把合并缓冲立即交付（暂停期间不丢字节）。
    fn flush_suppressed_output(&mut self, pane: PaneId) {
        let suppressed = std::mem::take(&mut self.flow.entry(pane).or_default().suppressed);
        if !suppressed.is_empty() {
            self.push_pane_output(pane, suppressed);
        }
    }

    fn pane_event_backlog_bytes(&self, pane: PaneId) -> usize {
        self.events
            .iter()
            .filter_map(|event| match event {
                StateChange::PaneOutput { pane: p, data } if *p == pane => Some(data.len()),
                _ => None,
            })
            .sum()
    }

    /// 启动一次不会丢帧的 pane resync transaction。
    fn begin_pane_resync(&mut self, pane: PaneId, reason: &'static str) {
        if self.resyncs.contains_key(&pane) {
            return;
        }
        // 老的增量已经无法与即将捕获的屏幕建立可靠边界；丢掉它们，
        // 否则 snapshot reset 后又会被旧帧覆盖。
        self.events.retain(
            |event| !matches!(event, StateChange::PaneOutput { pane: p, .. } if *p == pane),
        );
        if let Some(flow) = self.flow.get_mut(&pane) {
            flow.resyncing = true;
            flow.queued_at = None;
            flow.suppressed.clear();
            flow.window_bytes = 0;
            flow.overload = false;
        }
        self.resyncs.insert(pane, PaneResync::default());
        self.paused_panes.insert(pane);

        if self
            .dispatch_tmux_command(&cmd::refresh_client_pause(pane, true))
            .is_err()
        {
            // 单元测试/断线窗口没有 command channel；保持状态机可恢复，
            // 不把 pane 永久卡在 resyncing。
            self.resyncs.remove(&pane);
            if let Some(flow) = self.flow.get_mut(&pane) {
                flow.resyncing = false;
            }
            self.paused_panes.remove(&pane);
            return;
        }
        let query = cmd::display_message(PaneId(pane.0), PANE_RESYNC_FORMAT);
        if self.dispatch_tmux_command(&query).is_ok() {
            self.replace_last_pending(PendingQuery::PaneResyncState { pane });
            tracing::info!(
                target: "muxterm::tmux::resync",
                pane = pane.0,
                reason,
                "paused pane and requested authoritative state/capture"
            );
        } else {
            self.resyncs.remove(&pane);
            if let Some(flow) = self.flow.get_mut(&pane) {
                flow.resyncing = false;
            }
            self.paused_panes.remove(&pane);
        }
    }

    fn maybe_start_resyncs(&mut self) {
        if self.cmd_tx.is_none() {
            return;
        }
        let now = Instant::now();
        let panes: Vec<PaneId> = self
            .flow
            .iter()
            .filter(|(pane, flow)| {
                let backlog = self.pane_event_backlog_bytes(**pane);
                !flow.resyncing
                    && (flow.overload
                        || backlog >= RESYNC_BACKLOG_BYTES
                        || flow
                            .queued_at
                            .is_some_and(|at| now.duration_since(at) >= RESYNC_BACKLOG_AGE))
            })
            .map(|(pane, _)| *pane)
            .collect();
        for pane in panes {
            self.begin_pane_resync(pane, "output-backlog");
        }
    }

    fn isolated_socket_name(&self) -> Option<&str> {
        let mut it = self.config.extra_args.iter();
        while let Some(a) = it.next() {
            if a == "-L" {
                return it.next().map(String::as_str);
            }
        }
        None
    }

    fn is_attach_mode(&self) -> bool {
        matches!(self.config.mode.as_ref(), Some(ConnectMode::Attach { .. }))
    }

    /// tmux window → muxterm Tab。处理 `%window-add`；
    /// `%unlinked-window-add` 是其它 session 的窗口，不进入当前 tab 列表。
    fn add_window_tab(&mut self, tab: TabId) {
        // move-window 的 unlink→link 组合里，`%window-add` 是窗口已重新 link
        // 的明确信号；若此前 close 已挂起，立即取消，不必等下一次权威查询。
        self.pending_close_tabs.remove(&tab);
        if !self.tabs.iter().any(|t| t.id == tab) {
            self.tabs.push(TabInfo {
                id: tab,
                name: format!("t{}", tab.0),
                active: true,
            });
            for t in self.tabs.iter_mut() {
                if t.id != tab {
                    t.active = false;
                }
            }
            // 新 window 的 pane 尚未由 list-panes 返回；不能继续把旧 tab pane
            // 暴露为全局 active，否则紧接着的 split/input 会串到旧 tab。
            for pane in &mut self.panes {
                pane.active = false;
            }
            // list-panes 返回前没有任何可发布的真实 PaneId。禁止用 PaneId(0)
            // 占位：它通常属于旧 tab，会让 Workspace/GTK 把同一 pane 挂进
            // 两个 tab，造成数据串 pane 与 gtk_widget_set_parent critical。
            self.layouts.remove(&tab);
            self.events.push_back(StateChange::TabAdded { tab });
        }
        // 主动查询该 tmux window 的 pane
        self.query_list_panes(tab);
    }

    /// tmux window 关闭 → muxterm Tab 关闭。
    /// `%window-close` 与 `%unlinked-window-close` 共用。
    /// 真正关闭一个 tab：先逐 pane 发 PaneClosed，前端才能回收对应的终端视图；
    /// 只发 TabClosed 会让切 tab 后保留的视图泄漏（视图只在 PaneClosed 时移除）。
    fn remove_window_tab(&mut self, tab: TabId) {
        for p in self.panes.iter().filter(|p| p.tab == tab) {
            self.events
                .push_back(StateChange::PaneClosed { pane: p.id });
        }
        self.panes.retain(|p| p.tab != tab);
        self.layouts.remove(&tab);
        self.tabs.retain(|t| t.id != tab);
        self.events.push_back(StateChange::TabClosed { tab });
    }

    fn close_window_tab(&mut self, tab: TabId) {
        if !self.tabs.iter().any(|t| t.id == tab) {
            return;
        }
        // 同一 tab 已挂起等待权威裁决时，不重复发查询。
        if self.pending_close_tabs.contains(&tab) {
            return;
        }
        // 先挂起：move-window 的 unlink/link 会产生 add+close 通知组合，
        // close 可能晚于权威 list-windows 响应到达；立即删除会把已确认存在
        // 的窗口删掉。立即发起权威查询，由响应裁决是否真正关闭；发不出
        // 查询（如单测/未连接）时只能按通知直接关闭。
        self.pending_close_tabs.insert(tab);
        if !self.query_list_windows() {
            self.pending_close_tabs.remove(&tab);
            self.remove_window_tab(tab);
        }
    }

    /// 权威 `list-windows` 响应裁决挂起的关闭：响应里仍存在的窗口取消关闭
    /// （move-window 重新 link），确实不存在的窗口才发 TabClosed/PaneClosed。
    fn settle_pending_close_tabs(&mut self, confirmed_tabs: &HashSet<TabId>) {
        let pending: Vec<TabId> = self.pending_close_tabs.iter().copied().collect();
        for tab in pending {
            if confirmed_tabs.contains(&tab) {
                // move-window 已把窗口 link 回来：取消挂起，保留 tab。
                self.pending_close_tabs.remove(&tab);
                continue;
            }
            // 真正关闭
            self.remove_window_tab(tab);
            self.pending_close_tabs.remove(&tab);
        }
    }

    /// tmux window 重命名 → muxterm Tab 重命名。
    /// `%window-renamed` 与 `%unlinked-window-renamed` 共用。
    fn rename_window_tab(&mut self, tab: TabId, name: String) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab) {
            t.name = name.clone();
        }
        self.events.push_back(StateChange::TabRenamed { tab, name });
    }

    /// 处理一条 tmux Message，更新内部 state 并产生 StateChange。
    fn handle_message(&mut self, msg: Message) {
        match msg {
            Message::Output { pane, content, .. } => {
                if self.resyncs.contains_key(&pane) {
                    append_capped(
                        &mut self.resyncs.entry(pane).or_default().live,
                        &content,
                        MAX_PANE_OUTPUT_BYTES,
                    );
                    return;
                }
                // attach 的初始控制流可能先发一个 prompt，再由 list-panes
                // 查询完整屏幕。先暂存这段不完整输出（而不是直接丢弃），
                // capture-pane 返回后以完整快照初始化，并把暂存的实时增量
                // 拼到快照尾部；这样既保留完整屏幕又不丢查询期间的输出。
                if self.is_attach_mode() && !self.initial_capture_done.contains(&pane) {
                    // 若尚未发起 capture 查询（pending 未建立），说明此时
                    // 只是启动期提示；等 query_capture_pane 真正发出查询后再
                    // 开始缓冲，避免把启动 prompt 与屏幕内容混在一起。
                    if self.initial_capture_pending.contains(&pane) {
                        // `%begin` 是 tmux 对 capture 命令的确定性边界：边界
                        // 之前的通知可能已经被 capture-pane 包含，边界之后的
                        // 字节则一定是快照之后的 live 增量。
                        let buf = if self.capture_response_seen.contains(&pane) {
                            self.initial_capture_tail.entry(pane).or_default()
                        } else {
                            self.initial_capture_buf.entry(pane).or_default()
                        };
                        append_capped(buf, &content, MAX_PANE_OUTPUT_BYTES);
                        tracing::trace!(
                            target: "muxterm::tmux",
                            pane = pane.0,
                            len = content.len(),
                            "attach 快照查询期间暂存实时 %output"
                        );
                    } else {
                        tracing::trace!(
                            target: "muxterm::tmux",
                            pane = pane.0,
                            "attach 启动 prompt 已忽略（等待 capture 快照）"
                        );
                    }
                    return;
                }
                self.mark_pane_ready(pane);
                tracing::trace!(
                    target: "muxterm::tmux",
                    pane = pane.0,
                    len = content.len(),
                    "实时 %output 交付"
                );
                self.note_pane_output(pane, &content);
            }
            Message::LayoutChange {
                window,
                layout,
                visible_layout,
                flags,
            } => {
                // 同一 tmux server 上其它 session 的 layout-change 也会广播；
                // 不能对未 attach 的 window 发 list-panes，否则 tab/pane 会串台。
                let tab = TabId(window.0);
                if self.should_ignore_foreign_tab(tab) {
                    tracing::debug!(
                        target: "muxterm::tmux",
                        window = window.0,
                        "忽略非本 session 的 %layout-change"
                    );
                    return;
                }
                // `%layout-change` 携带完整树 + 可见树 + window_raw_flags。
                // zoom 时完整树不变，visible 塌成单叶且 flags 含 Z；必须记下
                // zoom 态，否则 rebuild_layout 仍按 split 渲染，tmux 已 zoom
                // 而 muxterm 看起来没变。
                // `%layout-change` 携带的是最新完整布局。先保存它，再查询 pane
                // 几何；list-panes 返回后 rebuild_layout 会用这棵最新树建模。
                // 旧实现只发查询、不更新 window_layouts，导致随后仍用旧树或
                // fallback 平铺树渲染，尤其在 attach 后再次 split 时会暴露。
                tracing::debug!(
                    target: "muxterm::tmux",
                    window = window.0,
                    layout = %layout.raw,
                    flags = flags.as_deref().unwrap_or(""),
                    "%layout-change 已保存并重新查询 pane"
                );
                self.window_layouts.insert(tab, layout.raw.clone());
                if window_is_zoomed(
                    flags.as_deref(),
                    &layout.raw,
                    visible_layout.as_ref().map(|v| v.raw.as_str()),
                ) {
                    self.window_zoomed.insert(tab);
                } else {
                    self.window_zoomed.remove(&tab);
                }
                if let Ok(tree) = parse_layout_tree(&layout.raw) {
                    self.expected_panes_per_window
                        .insert(tab, collect_layout_leaves(&tree).len());
                }
                self.query_list_panes(tab);
            }
            Message::WindowAdd { window } => {
                self.add_window_tab(window);
            }
            Message::WindowClose { window } => {
                self.close_window_tab(window);
            }
            Message::WindowRenamed { window, name } => {
                self.rename_window_tab(window, name);
            }
            Message::UnlinkedWindowAdd { .. } => {
                // 其它 session 新建窗口也会推 %unlinked-window-add（实测），
                // 该窗口不属于当前 attach 的 session，不能加进 tab 列表；
                // 若它随后被 link 进当前 session，tmux 会再发 %window-add。
            }
            Message::UnlinkedWindowClose { window } => {
                // 实测：kill-window 时控制客户端收到的是 %unlinked-window-close
                // 而不是 %window-close（tmux 3.4）。忽略它会导致 tab 关闭后
                // statusbar 不更新、Alt+1..4 仍能切到幽灵 tab。
                self.close_window_tab(window);
            }
            Message::UnlinkedWindowRenamed { window, name } => {
                self.rename_window_tab(window, name);
            }
            Message::SessionChanged { session, name } => {
                self.active_session = Some(session);
                if let Some(n) = name {
                    if self.workspace_name != n {
                        self.workspace_name = n.clone();
                        self.events
                            .push_back(StateChange::WorkspaceRenamed { name: n });
                    }
                }
            }
            Message::SessionRenamed { session, name } => {
                if self.active_session == Some(session) && self.workspace_name != name {
                    self.workspace_name = name.clone();
                    self.events
                        .push_back(StateChange::WorkspaceRenamed { name });
                }
            }
            Message::SessionsChanged => {
                self.events.push_back(StateChange::PoolChanged);
            }
            Message::PaneModeChanged { pane, mode } => {
                // mode 变化暂用作标题（简化）
                if let Some(p) = self.panes.iter_mut().find(|p| p.id == pane) {
                    if p.title != mode {
                        p.title = mode.clone();
                        self.events
                            .push_back(StateChange::PaneTitleChanged { pane, title: mode });
                    }
                }
            }
            Message::Exit { .. } => {
                self.status = BackendStatus::Exited;
                self.events
                    .push_back(StateChange::BackendStatusChanged(BackendStatus::Exited));
            }
            Message::WindowPaneChanged { window, pane } => {
                // tmux window 对应 muxterm tab（TabId(window.0)）
                let tab_id = TabId(window.0);
                if let Some(tl) = self.layouts.get_mut(&tab_id) {
                    tl.active = pane;
                }
                let tab_is_active = self
                    .tabs
                    .iter()
                    .any(|candidate| candidate.id == tab_id && candidate.active);
                if tab_is_active {
                    for candidate in self.panes.iter_mut() {
                        candidate.active = candidate.id == pane && candidate.tab == tab_id;
                    }
                    self.events
                        .push_back(StateChange::ActivePaneChanged { tab: tab_id, pane });
                } else {
                    for candidate in self.panes.iter_mut().filter(|p| p.tab == tab_id) {
                        candidate.active = false;
                    }
                }
            }
            Message::SessionWindowChanged { session, window } => {
                // 控制模式会收到整台 server 上其它 session 的切换通知
                // （日志里 `$4 muxterm` vs 已 attach 的 `$0 yaklang-workspace`）。
                if !self.is_attached_session(session) {
                    tracing::debug!(
                        target: "muxterm::tmux",
                        session = session.0,
                        window = window.0,
                        "忽略其它 session 的 %session-window-changed"
                    );
                    return;
                }
                // tmux session 的 active window 切换 → muxterm active tab 切换
                let tab_id = TabId(window.0);
                self.mark_tab_active(tab_id);
                // 活动 tab 首次切入时才请求 Surface seed。后台 tab 的原始
                // `%output` 继续进入索引面，但不会阻塞连接或提前创建 GUI。
                self.query_capture_tab(tab_id);
                self.query_panes_if_empty(tab_id);
            }
            Message::SubscriptionChanged { name, value, pane } => {
                // status-left/right / pane-cmd 订阅推送 → 前端直接消费（零轮询）。
                self.events
                    .push_back(StateChange::StatusBarSubscription { name, value, pane });
            }
            Message::ExtendedOutput { pane, content, .. } => {
                self.mark_pane_ready(pane);
                if self.resyncs.contains_key(&pane) {
                    append_capped(
                        &mut self.resyncs.entry(pane).or_default().live,
                        &content,
                        MAX_PANE_OUTPUT_BYTES,
                    );
                    return;
                }
                // pause-after 下的 %output 新形式：内容与 %output 一样是 pane
                // 增量字节，必须走同一条累积/交付路径，否则暂停恢复后丢输出。
                tracing::trace!(
                    target: "muxterm::tmux",
                    pane = pane.0,
                    len = content.len(),
                    "实时 %extended-output 交付"
                );
                self.note_pane_output(pane, &content);
            }
            Message::Pause { pane, .. } => {
                if let Some(pane) = pane {
                    self.paused_panes.insert(pane);
                    tracing::debug!(
                        target: "muxterm::tmux::pause",
                        pane = pane.0,
                        "tmux reported paused pane; scheduling authoritative resync"
                    );
                    // tmux 在 pause 时可以 discard pending blocks；收到通知
                    // 后必须重新 capture，不能仅等待 continue 再拼 bytes。
                    if self.cmd_tx.is_some() {
                        self.begin_pane_resync(pane, "tmux-pause");
                    } else {
                        self.flush_suppressed_output(pane);
                    }
                }
            }
            Message::Continue { pane, .. } => {
                if let Some(pane) = pane {
                    if !self.resyncs.contains_key(&pane) {
                        self.paused_panes.remove(&pane);
                        self.flush_suppressed_output(pane);
                    }
                }
            }
            Message::ResponseBoundary(_) | Message::Unknown { .. } => {
                // 命令响应边界由 pump_events 单独处理；未知消息忽略。
            }
        }
    }

    /// drain event_rx 的 TmuxEvent，更新 state。
    fn pump_events(&mut self) {
        let mut command_errors = Vec::new();
        if let Some(rx) = self.command_error_rx.as_mut() {
            while let Ok(message) = rx.try_recv() {
                command_errors.push(message);
            }
        }
        for message in command_errors {
            tracing::error!(target: "muxterm::tmux", "发送 tmux 命令失败: {message}");
            self.status = BackendStatus::Error;
            self.events
                .push_back(StateChange::BackendStatusChanged(BackendStatus::Error));
        }
        self.release_deferred_writes();
        self.poll_ready_probes();

        // 先把所有 TmuxEvent drain 到本地 vec，避免与 self 的可变借用冲突。
        let mut pending = Vec::new();
        if let Some(rx) = self.event_rx.as_mut() {
            while let Ok(ev) = rx.try_recv() {
                pending.push(ev);
            }
        }
        for ev in pending {
            match ev {
                TmuxEvent::Message(msg) => {
                    // 先处理 ResponseBoundary（begin/end 状态机），再处理其他消息。
                    if let Message::ResponseBoundary(b) = &msg {
                        match b.kind {
                            NotificationKind::Begin => {
                                self.response_accum.insert(b.number, Vec::new());
                                // tmux 串行执行命令：`%begin <n>` 到达时，队首查询即
                                // 该命令的响应槽。按 number 登记，end/error 时精确匹配，
                                // 避免高输出下 FIFO pop 错配。
                                if let Some(q) = self.pending_queries.pop_front() {
                                    self.pending_by_number.insert(b.number, q);
                                }
                                if let Some(PendingQuery::CapturePane { pane }) =
                                    self.pending_by_number.get(&b.number)
                                {
                                    self.capture_response_seen.insert(*pane);
                                }
                            }
                            NotificationKind::End => {
                                let lines =
                                    self.response_accum.remove(&b.number).unwrap_or_default();
                                self.dispatch_response(b.number, lines);
                            }
                            NotificationKind::Error => {
                                self.handle_response_error(b.number);
                            }
                        }
                    }
                    // 通知消息（WindowAdd / Output 等）先于对应的 %begin/%end 到达，
                    // 所以先 handle_message 处理通知，再在上面处理响应边界。
                    self.handle_message(msg);
                }
                TmuxEvent::ResponseLine { number, line, .. } => {
                    // 累积到对应 number 的响应缓冲（begin 后、end 前的行）

                    self.response_accum.entry(number).or_default().push(line);
                }
                TmuxEvent::Exit { .. } => {
                    self.status = BackendStatus::Exited;
                    self.events
                        .push_back(StateChange::BackendStatusChanged(BackendStatus::Exited));
                }
            }
        }
        self.maybe_start_resyncs();
    }

    /// 处理一条命令的完整响应（%begin..%end 之间的行）。
    ///
    /// 从 pending_queries 弹出最早的一个查询，按类型解析响应行。
    fn dispatch_response(&mut self, number: i64, lines: Vec<String>) {
        if let Some(query) = self.pending_by_number.remove(&number) {
            match query {
                PendingQuery::Ignore => {}
                PendingQuery::ReadyProbe { pane } => {
                    self.ready_probe_in_flight.remove(&pane);
                    self.ready_probe_acknowledged.insert(pane);
                }
                PendingQuery::ListPanes { tab } => {
                    self.handle_list_panes_response(tab, lines);
                }
                PendingQuery::ListWindows => {
                    self.handle_list_windows_response(lines);
                }
                PendingQuery::DisplayMessage { pane } => {
                    // 单行响应：用作 pane 标题
                    if let Some(line) = lines.first() {
                        let title = line.trim().to_string();
                        if let Some(p) = self.panes.iter_mut().find(|p| p.id == pane) {
                            if p.title != title {
                                p.title = title.clone();
                                self.events
                                    .push_back(StateChange::PaneTitleChanged { pane, title });
                            }
                        }
                    }
                }
                PendingQuery::PaneResyncState { pane } => {
                    if let Some(resync) = self.resyncs.get_mut(&pane) {
                        resync.state = lines.first().map(|line| parse_pane_replay_state(line));
                    }
                    let primary = cmd::capture_pane_with_history(pane, self.scrollback_lines);
                    let alternate = cmd::capture_alternate_pane(pane, self.scrollback_lines);
                    let primary_ok = self.dispatch_tmux_command(&primary).is_ok();
                    if primary_ok {
                        self.replace_last_pending(PendingQuery::PaneResyncCapture {
                            pane,
                            alternate: false,
                        });
                    }
                    let alternate_ok = self.dispatch_tmux_command(&alternate).is_ok();
                    if alternate_ok {
                        self.replace_last_pending(PendingQuery::PaneResyncCapture {
                            pane,
                            alternate: true,
                        });
                    }
                    if !primary_ok || !alternate_ok {
                        self.finish_pane_resync(pane);
                    }
                }
                PendingQuery::PaneResyncCapture { pane, alternate } => {
                    if let Some(resync) = self.resyncs.get_mut(&pane) {
                        let data = capture_pane_bytes(&lines);
                        // `capture-pane` returns the currently visible screen;
                        // `capture-pane -a` returns the inactive/saved screen
                        // when tmux is already in alternate mode.  Therefore
                        // the response's destination depends on alternate_on,
                        // not merely on which command was sent.
                        let alternate_on = resync
                            .state
                            .as_ref()
                            .is_some_and(|state| state.alternate_on);
                        if alternate != alternate_on {
                            resync.alternate = Some(data);
                        } else {
                            resync.primary = Some(data);
                        }
                    }
                    if alternate {
                        self.finish_pane_resync(pane);
                    }
                }
                PendingQuery::CapturePane { pane } => {
                    // capture-pane -p 按行返回当前可见屏幕；拼回 CRLF 后喂给
                    // terminal emulator。attach 初始阶段必须以快照替换此前
                    // 被抑制的 `%output`，不能因为已有 prompt 就跳过恢复。
                    let mut data = capture_pane_bytes(&lines);
                    if self.is_attach_mode() {
                        self.initial_capture_pending.remove(&pane);
                        self.initial_capture_done.insert(pane);
                        if self.new_attach_panes.contains(&pane)
                            && self.pending_writes.contains_key(&pane)
                        {
                            self.start_pane_ready_probe(pane);
                        } else if !self.new_attach_panes.contains(&pane) {
                            self.deferred_write_panes.insert(pane);
                        }
                        let before_response =
                            self.initial_capture_buf.remove(&pane).unwrap_or_default();
                        let after_response =
                            self.initial_capture_tail.remove(&pane).unwrap_or_default();
                        let response_seen = self.capture_response_seen.remove(&pane);
                        // 真实 control-mode 流中 `%begin` 已经给出边界：
                        // - begin 前的通知可能已被 capture-pane 包含，丢弃以免历史重复；
                        // - begin 后的字节属于快照之后的 live，必须追加。
                        // 直接调用 dispatch_response 的单元测试没有 `%begin`，
                        // 仍按旧的“查询期间暂存”语义保留 before_response。
                        if response_seen {
                            data.extend_from_slice(&after_response);
                        } else {
                            data.extend_from_slice(&before_response);
                            data.extend_from_slice(&after_response);
                        }
                        let snapshot = if data.len() > MAX_PANE_OUTPUT_BYTES {
                            data[data.len() - MAX_PANE_OUTPUT_BYTES..].to_vec()
                        } else {
                            data
                        };
                        self.outputs.insert(pane, snapshot.clone());
                        if !snapshot.is_empty() {
                            // capture-pane 是权威替换，不是 live 增量。前端必须
                            // reset VT 后再应用，否则 attach/Cursor 会把 seed
                            // 当成命令输出重放，造成首屏和历史双写。
                            self.events.push_back(StateChange::PaneSnapshot {
                                pane,
                                data: snapshot,
                            });
                            self.trim_event_queue();
                        }
                    } else if !data.is_empty()
                        && self
                            .outputs
                            .get(&pane)
                            .is_none_or(|output| output.is_empty())
                    {
                        append_capped(
                            self.outputs.entry(pane).or_default(),
                            &data,
                            MAX_PANE_OUTPUT_BYTES,
                        );
                        self.events
                            .push_back(StateChange::PaneOutput { pane, data });
                        self.trim_event_queue();
                    }
                }
                PendingQuery::ListSessions => {
                    // list-sessions 默认格式: "demo: 1 windows (created ...)"
                    let mut changed = false;
                    for line in &lines {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        let name = line.split(':').next().unwrap_or("").trim();
                        if name.is_empty() {
                            continue;
                        }
                        if !self.known_sessions.iter().any(|(_, n)| n == name) {
                            let sid = TmuxSessionId(self.known_sessions.len() as u32);
                            self.known_sessions.push((sid, name.to_string()));
                            changed = true;
                        }
                    }
                    if changed {
                        self.events.push_back(StateChange::PoolChanged);
                    }
                }
            }
        }
    }

    /// 完成 snapshot transaction，并把唯一一条替换事件放入队列。
    fn finish_pane_resync(&mut self, pane: PaneId) {
        let Some(resync) = self.resyncs.remove(&pane) else {
            return;
        };
        let primary = resync.primary.unwrap_or_default();
        let alternate = resync.alternate.unwrap_or_default();
        let mut snapshot =
            build_pane_snapshot(resync.state.as_ref(), &primary, &alternate, &resync.live);
        if snapshot.len() > MAX_PANE_OUTPUT_BYTES {
            snapshot = snapshot[snapshot.len() - MAX_PANE_OUTPUT_BYTES..].to_vec();
        }
        if let Some(flow) = self.flow.get_mut(&pane) {
            flow.resyncing = false;
            flow.queued_at = None;
            flow.window_bytes = 0;
            flow.overload = false;
        }
        self.paused_panes.remove(&pane);
        // An empty screen is still authoritative: emit a zero-byte snapshot so
        // every frontend clears its previous VT instead of retaining stale text.
        self.outputs.insert(pane, snapshot.clone());
        self.events.retain(
            |event| !matches!(event, StateChange::PaneOutput { pane: p, .. } if *p == pane),
        );
        self.events.push_back(StateChange::PaneSnapshot {
            pane,
            data: snapshot,
        });
        self.trim_event_queue();
        // snapshot 入队后再 continue；其后的 tmux 输出会形成下一批增量。
        let _ = self.dispatch_tmux_command(&cmd::refresh_client_pause(pane, false));
    }

    /// 处理一条命令响应的 `%error` 边界。
    ///
    /// 出错时移除按 number 登记的查询，并确保 attach 的 capture 失败不会永久
    /// 抑制该 pane 的实时输出（否则会黑屏）。capture 期间已经收到的字节不能
    /// 丢掉：它们是唯一可能包含真实 live 输出的 fallback seed。
    fn handle_response_error(&mut self, number: i64) {
        let _err_lines = self.response_accum.remove(&number).unwrap_or_default();
        if let Some(q) = self.pending_by_number.remove(&number) {
            match q {
                PendingQuery::CapturePane { pane } => {
                    self.initial_capture_pending.remove(&pane);
                    self.initial_capture_done.insert(pane);
                    if self.new_attach_panes.contains(&pane)
                        && self.pending_writes.contains_key(&pane)
                    {
                        self.start_pane_ready_probe(pane);
                    } else if !self.new_attach_panes.contains(&pane) {
                        self.deferred_write_panes.insert(pane);
                    }
                    let before_response =
                        self.initial_capture_buf.remove(&pane).unwrap_or_default();
                    let after_response =
                        self.initial_capture_tail.remove(&pane).unwrap_or_default();
                    let response_seen = self.capture_response_seen.remove(&pane);
                    // 与成功响应保持相同的边界语义：%begin 之前的通知可能已经
                    // 被 capture-pane 包含，只有响应边界之后的字节才是可靠的
                    // live fallback。没有看到 %begin 的单元测试/老 tmux 路径仍
                    // 保留 before_response，避免 capture 失败后黑屏。
                    let fallback = if response_seen {
                        after_response
                    } else {
                        let mut combined = before_response;
                        combined.extend_from_slice(&after_response);
                        combined
                    };
                    if !fallback.is_empty() {
                        tracing::warn!(
                            target: "muxterm::tmux",
                            pane = pane.0,
                            bytes = fallback.len(),
                            "capture 失败，交付暂存 live fallback"
                        );
                        self.push_pane_output(pane, fallback);
                    }
                    tracing::warn!(
                        target: "muxterm::tmux",
                        "tmux 命令 {number} 的 pane @{} 屏幕恢复失败",
                        pane.0
                    );
                }
                PendingQuery::PaneResyncState { pane }
                | PendingQuery::PaneResyncCapture { pane, .. } => {
                    tracing::warn!(
                        target: "muxterm::tmux::resync",
                        pane = pane.0,
                        number,
                        "pane snapshot query failed; releasing resync"
                    );
                    self.finish_pane_resync(pane);
                }
                PendingQuery::ReadyProbe { pane } => {
                    self.ready_probe_in_flight.remove(&pane);
                    self.ready_probe_acknowledged.remove(&pane);
                    self.ready_probe_at.insert(pane, Instant::now());
                }
                other => {
                    tracing::warn!(
                        target: "muxterm::tmux",
                        "tmux 命令 {number} 出错（丢弃查询 {other:?}）",
                    );
                }
            }
        }
    }

    /// 解析 `list-panes -a -t <session> -F '...'` 的响应。
    ///
    /// 每行格式：`%N,@M,<active>,<cols>x<rows>,<x>,<y>`（逗号分隔）
    /// 解析 `list-panes -t @N` 的响应。
    ///
    /// 默认格式："1: [70x30] [history ...] %0 (active)"
    /// 参数 tab 是这些 pane 所属的 tmux window（= muxterm tab）。
    fn handle_list_panes_response(&mut self, tab: TabId, lines: Vec<String>) {
        tracing::debug!(target: "muxterm::tmux", "list-panes 响应 tab=@{}: {} 行", tab.0, lines.len());
        let tab_id = tab;
        let mut new_panes: Vec<PaneInfo> = Vec::new();
        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let pane = match extract_pane_id_from_default(line) {
                Some(p) => p,
                None => continue,
            };
            let (cols, rows) = extract_size_from_default(line);
            let active = line.contains("(active)");
            new_panes.push(PaneInfo {
                id: pane,
                tab: tab_id,
                active,
                title: String::new(),
                cols,
                rows,
            });
        }
        if let Some(expected) = self
            .expected_panes_per_window
            .get(&tab)
            .copied()
            .filter(|count| *count > 0)
        {
            if new_panes.len() != expected {
                tracing::debug!(
                    target: "muxterm::tmux",
                    "忽略 tab=@{} 的不完整 pane 快照: got={}, expected={}",
                    tab.0,
                    new_panes.len(),
                    expected
                );
                self.query_list_panes(tab);
                return;
            }
        }
        let tab_is_active = self
            .tabs
            .iter()
            .any(|candidate| candidate.id == tab_id && candidate.active);
        let authoritative_active = new_panes
            .iter()
            .find(|pane| pane.active)
            .map(|pane| pane.id);
        let old_global_active = self
            .panes
            .iter()
            .find(|pane| pane.active)
            .map(|pane| pane.id);
        if tab_is_active {
            for pane in &mut self.panes {
                pane.active = false;
            }
        }
        let mut changed = false;
        for np in &new_panes {
            let globally_active = tab_is_active && np.active;
            if let Some(existing) = self.panes.iter_mut().find(|p| p.id == np.id) {
                if existing.cols != np.cols
                    || existing.rows != np.rows
                    || existing.active != globally_active
                {
                    existing.cols = np.cols;
                    existing.rows = np.rows;
                    existing.active = globally_active;
                    self.events.push_back(StateChange::PaneResized {
                        pane: np.id,
                        cols: np.cols,
                        rows: np.rows,
                    });
                }
            } else {
                let mut pane = np.clone();
                pane.active = globally_active;
                if self.attach_bootstrap_complete && self.is_attach_mode() {
                    self.new_attach_panes.insert(np.id);
                }
                self.panes.push(pane);
                self.events.push_back(StateChange::PaneAdded {
                    pane: np.id,
                    tab: tab_id,
                });
                changed = true;
            }
        }
        let valid_ids: Vec<PaneId> = new_panes.iter().map(|p| p.id).collect();
        let to_remove: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|p| p.tab == tab_id && !valid_ids.contains(&p.id))
            .map(|p| p.id)
            .collect();
        for pid in to_remove {
            self.panes.retain(|p| p.id != pid);
            self.pending_writes.remove(&pid);
            self.deferred_write_panes.remove(&pid);
            self.awaiting_pane_ready.remove(&pid);
            self.ready_probe_at.remove(&pid);
            self.ready_probe_in_flight.remove(&pid);
            self.ready_probe_acknowledged.remove(&pid);
            self.ready_probe_rounds.remove(&pid);
            self.new_attach_panes.remove(&pid);
            self.events.push_back(StateChange::PaneClosed { pane: pid });
            changed = true;
        }
        if changed || !new_panes.is_empty() {
            self.rebuild_layout(tab_id, &new_panes);
        }
        if tab_is_active && old_global_active != authoritative_active {
            if let Some(pane) = authoritative_active {
                self.events
                    .push_back(StateChange::ActivePaneChanged { tab: tab_id, pane });
            }
        }
        // attach 的控制模式不一定会把当前屏幕历史作为 %output 推送。只对
        // 活动 tab 发起首屏 capture；其它 tab 在第一次切入时由
        // `SessionWindowChanged` 触发，避免多窗口 attach 阶段串行抓屏。
        if self.tab_is_active(tab_id) {
            for pane in new_panes {
                self.query_capture_pane(pane.id);
            }
        }
    }

    /// 解析 `list-windows -t <session> -F '#{window_id},#{window_name},#{window_active},#{window_layout},#{window_panes},#{window_zoomed_flag}'` 的响应。
    fn handle_list_windows_response(&mut self, lines: Vec<String>) {
        // tmux list-windows 返回所有 tmux window → 每个创建/更新一个 muxterm Tab
        let mut order = HashMap::new();
        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // window_layout 本身含逗号（如 `d67e,80x24,0,0{...}`），不能用 splitn(5)
            let Some((tab, name, active, layout_str, panes_count, zoomed)) =
                parse_list_windows_line(line)
            else {
                tracing::warn!(target: "muxterm::tmux", "list-windows 行解析失败: {line}");
                continue;
            };
            order.insert(tab, order.len());
            self.window_layouts.insert(tab, layout_str);
            if zoomed {
                self.window_zoomed.insert(tab);
            } else {
                self.window_zoomed.remove(&tab);
            }
            self.expected_panes_per_window.insert(tab, panes_count);

            // tmux window → muxterm Tab
            if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab) {
                t.name = name.clone();
                t.active = active;
            } else {
                self.tabs.push(TabInfo {
                    id: tab,
                    name: name.clone(),
                    active,
                });
                self.events.push_back(StateChange::TabAdded { tab });
            }

            // 主动查询该 tmux window 的 panes
            self.query_list_panes(tab);
        }
        // TabId 是稳定的 @window_id；用户拖动 tab 只会改变 tmux index，
        // 因此必须按 list-windows 的返回顺序重排，不能保留旧 Vec 顺序。
        self.tabs
            .sort_by_key(|tab| order.get(&tab.id).copied().unwrap_or(usize::MAX));
        // 权威列表已到：裁决 move-window 等临时 unlink 产生的挂起 close。
        let confirmed_tabs: HashSet<TabId> = order.keys().copied().collect();
        self.settle_pending_close_tabs(&confirmed_tabs);
    }

    /// 发送 list-panes 查询（异步，通过 cmd_tx）。
    fn query_list_panes(&mut self, tab: TabId) {
        // 用 list-panes -t @N 查询单个 window 的 pane（默认格式不含 window_id）。
        if self.pending_queries.iter().any(
            |query| matches!(query, PendingQuery::ListPanes { tab: pending } if *pending == tab),
        ) {
            return;
        }
        let line = format!("list-panes -t @{}\n", tab.0);
        if self.dispatch_command(line).is_ok() {
            self.replace_last_pending(PendingQuery::ListPanes { tab });
        }
    }

    /// 查询 pane 当前可见屏幕，用于 attach 初始渲染恢复。
    fn query_capture_pane(&mut self, pane: PaneId) {
        if !self.is_attach_mode() {
            return;
        }
        if self.pending_queries.iter().any(
            |query| matches!(query, PendingQuery::CapturePane { pane: pending } if *pending == pane),
        ) || self.initial_capture_done.contains(&pane)
        {
            return;
        }
        // W16a：attach 播种必须带 scrollback（`-S -N`），否则滚出可见区的
        // 历史搜不到、滚不到。N = 配置的 scrollback 上限（默认 10000）。
        let line = cmd::capture_pane_with_history(pane, self.scrollback_lines).to_line();
        if self.dispatch_command(line).is_ok() {
            self.initial_capture_buf.remove(&pane);
            self.initial_capture_tail.remove(&pane);
            self.capture_response_seen.remove(&pane);
            self.initial_capture_pending.insert(pane);
            self.replace_last_pending(PendingQuery::CapturePane { pane });
        }
    }

    /// 发送 attach 首次快照边界前暂存的用户输入。
    fn dispatch_pending_write(&mut self, pane: PaneId) {
        let Some(data) = self.pending_writes.remove(&pane) else {
            return;
        };
        if data.is_empty() {
            return;
        }
        let command = cmd::send_keys_bytes(pane, &data);
        if self.dispatch_tmux_command(&command).is_err() {
            tracing::warn!(
                target = "muxterm::tmux",
                pane = pane.0,
                bytes = data.len(),
                "发送 attach 暂存输入失败"
            );
        }
    }

    /// 释放上一轮 capture 完成后暂缓的输入。
    fn release_deferred_writes(&mut self) {
        let panes = self.deferred_write_panes.drain().collect::<Vec<_>>();
        for pane in panes {
            self.dispatch_pending_write(pane);
        }
    }

    /// 某个新 attach pane 首次收到用户输入时才启动 readiness probe，
    /// 不在空闲 pane 上额外制造回车或改变 attach 后的屏幕。
    fn start_pane_ready_probe(&mut self, pane: PaneId) {
        if !self.new_attach_panes.remove(&pane) {
            return;
        }
        self.awaiting_pane_ready.insert(pane);
        self.ready_probe_rounds.insert(pane, 0);
        self.ready_probe_at.insert(pane, Instant::now());
    }

    /// 新建 attach pane 的 shell 可能在首个 capture prompt 之后仍处于
    /// 初始化窗口。用 Enter 作为无害 probe；只有收到该 pane 的下一段
    /// `%output` 才释放用户输入，若 probe 被启动阶段吞掉则下轮重试。
    fn poll_ready_probes(&mut self) {
        let now = Instant::now();
        let panes = self
            .awaiting_pane_ready
            .iter()
            .copied()
            .filter(|pane| self.ready_probe_at.get(pane).is_some_and(|at| *at <= now))
            .collect::<Vec<_>>();
        for pane in panes {
            if self.ready_probe_in_flight.contains(&pane) {
                continue;
            }
            self.ready_probe_acknowledged.remove(&pane);
            let command = cmd::send_keys(pane, &[cmd::Key::enter()]);
            if self.dispatch_tmux_command(&command).is_ok() {
                self.replace_last_pending(PendingQuery::ReadyProbe { pane });
                self.ready_probe_in_flight.insert(pane);
                self.ready_probe_at
                    .insert(pane, now + Duration::from_millis(100));
            } else {
                // 命令通道瞬时断开时不要把 pane 永久卡在 awaiting 状态；
                // 下一轮 poll 仍可在 Runtime 重连后重试 probe。
                self.ready_probe_at
                    .insert(pane, now + Duration::from_millis(100));
            }
        }
    }

    /// 收到 readiness probe 或其它首段实时输出后释放暂存输入。
    fn mark_pane_ready(&mut self, pane: PaneId) {
        if !self.ready_probe_acknowledged.remove(&pane) {
            return;
        }
        self.ready_probe_in_flight.remove(&pane);
        let rounds = self.ready_probe_rounds.entry(pane).or_default();
        *rounds = rounds.saturating_add(1);
        if *rounds < 2 {
            self.ready_probe_at
                .insert(pane, Instant::now() + Duration::from_millis(25));
            return;
        }
        self.ready_probe_rounds.remove(&pane);
        if !self.awaiting_pane_ready.remove(&pane) {
            return;
        }
        self.ready_probe_at.remove(&pane);
        self.dispatch_pending_write(pane);
    }

    /// 发送 list-sessions 查询（列出 tmux server 上所有 session）。
    fn query_list_sessions(&mut self) {
        let line = "list-sessions\n".to_string();
        if self.dispatch_command(line).is_ok() {
            self.replace_last_pending(PendingQuery::ListSessions);
        }
    }

    /// 探测 tmux 是否支持 `refresh-client -r`（颜色上报）：不支持时静默跳过，
    /// 避免老 tmux 每上报一次就打一条 `unknown flag -r` 错误。
    fn detect_colour_report_support(&mut self) {
        let socket = self
            .config
            .extra_args
            .iter()
            .position(|a| a == "-L")
            .and_then(|i| self.config.extra_args.get(i + 1))
            .cloned();
        let cfg = super::status::StatusQueryConfig {
            socket,
            ssh_alias: self.config.ssh_alias.clone(),
            session: String::new(),
        };
        if let Ok(version_out) = super::status::run_tmux(&cfg, &["-V"]) {
            let version = parse_tmux_version(&version_out);
            self.colour_report_supported = supports_colour_report(version);
            self.status_subscription_supported = supports_status_subscription(version);
        }
    }

    /// 订阅 status-left/right（文档 §B+：`refresh-client -B`，零轮询）。
    ///
    /// tmux ≥ 3.2 时值变化推 `%subscription-changed`（至多 1 次/秒），
    /// 前端据此更新原生条；老版本回退到前端轮询定时器。
    fn setup_status_subscriptions(&mut self) {
        self.status_subscriptions_active = false;
        if !self.status_subscription_supported {
            return;
        }
        let left = crate::core::runtime::tmux::command::refresh_client_subscribe(
            STATUS_LEFT_SUBSCRIPTION,
            "",
            "#{T:status-left}",
        );
        let right = crate::core::runtime::tmux::command::refresh_client_subscribe(
            STATUS_RIGHT_SUBSCRIPTION,
            "",
            "#{T:status-right}",
        );
        // pane 前台命令订阅：Working 粗判来源（LINUX-PLAN §9 C2.5b）。
        let pane_cmd = crate::core::runtime::tmux::command::refresh_client_subscribe(
            PANE_CMD_SUBSCRIPTION,
            "%*",
            "#{pane_current_command}",
        );
        // 必须走 dispatch_command：它会给每条命令登记一个 Ignore 响应槽，
        // 保持与 %begin/%end 的 FIFO 对齐；直接 tx.send 会吃掉后续真实查询
        // 的槽位，导致 list-windows/list-panes 响应错配（集成测试回归）。
        if self.dispatch_command(left.to_line()).is_ok()
            && self.dispatch_command(right.to_line()).is_ok()
            && self.dispatch_command(pane_cmd.to_line()).is_ok()
        {
            self.status_subscriptions_active = true;
            tracing::info!(
                target: "muxterm::tmux",
                "status bar 订阅已启用（refresh-client -B）"
            );
        }
    }

    /// 发送 list-windows 查询。
    /// list-windows 的 session 目标：active_session 已落地用 `$N`；
    /// 尚未收到 %session-changed 时用 attach 目标名（禁止默认 `$0`——
    /// 2026-08-15 dogfood 里 yaklang-workspace 是 `$4`，默认 `$0` 查错 session）。
    fn list_windows_session_target(&self) -> String {
        if let Some(sid) = self.active_session {
            return sid.as_str();
        }
        if let Some(ConnectMode::Attach { target: Some(name) }) = &self.config.mode {
            return name.clone();
        }
        // 非 attach 模式（new-session）在 SessionChanged 前没有可靠 id；
        // 返回空让调用方跳过查询，等事件落地后再查。
        String::new()
    }

    /// 发送 list-windows 查询。
    fn query_list_windows(&mut self) -> bool {
        let sess = self.list_windows_session_target();
        if sess.is_empty() {
            return false;
        }
        let line = format!(
            "list-windows -t {} -F \"#{{window_id}},#{{window_name}},#{{window_active}},#{{window_layout}},#{{window_panes}},#{{window_zoomed_flag}}\"\n",
            sess
        );

        if self.dispatch_command(line).is_ok() {
            self.replace_last_pending(PendingQuery::ListWindows);
            return true;
        }
        false
    }

    /// 用 parse_layout_tree 重建 LayoutNode 树。
    ///
    /// 需要 list-windows 的 window_layout 字符串。这里通过几何匹配把
    /// LayoutTree 叶子映射到 pane id（位置匹配）。
    fn rebuild_layout(&mut self, tab_id: TabId, panes: &[PaneInfo]) {
        if panes.is_empty() {
            return;
        }
        let active = panes
            .iter()
            .find(|p| p.active)
            .map(|p| p.id)
            .unwrap_or(panes[0].id);
        // zoom：pane 快照仍有全部 pane，但 GUI 只显示当前 pane（tmux prefix-z）。
        if self.window_zoomed.contains(&tab_id) {
            let layout = TabLayout {
                tab: tab_id,
                tree: LayoutNode::leaf(active),
                active,
            };
            self.layouts.insert(tab_id, layout.clone());
            self.push_layout_changed(layout);
            return;
        }
        if panes.len() == 1 {
            let tree = LayoutNode::leaf(panes[0].id);
            let layout = TabLayout {
                tab: tab_id,
                tree,
                active,
            };
            self.layouts.insert(tab_id, layout.clone());
            self.push_layout_changed(layout);
            return;
        }
        let layout_str = match self.window_layouts.get(&tab_id) {
            Some(s) => s.clone(),
            None => {
                self.build_fallback_layout(tab_id, panes, active);
                return;
            }
        };
        let tree = match parse_layout_tree(&layout_str) {
            Ok(lt) => lt,
            Err(e) => {
                tracing::warn!(target: "muxterm::tmux", "layout tree 解析失败: {e}");
                self.build_fallback_layout(tab_id, panes, active);
                return;
            }
        };
        let layout_node = match layout_tree_to_node(&tree, panes) {
            Some(n) => n,
            None => {
                self.build_fallback_layout(tab_id, panes, active);
                return;
            }
        };
        let layout = TabLayout {
            tab: tab_id,
            tree: layout_node,
            active,
        };
        self.layouts.insert(tab_id, layout.clone());
        self.push_layout_changed(layout);
    }

    /// 朴素兜底布局：按顺序水平排列 pane。
    fn build_fallback_layout(&mut self, tab_id: TabId, panes: &[PaneInfo], active: PaneId) {
        let mut sorted: Vec<PaneInfo> = panes.to_vec();
        sorted.sort_by_key(|p| (p.cols, p.id.0));
        let mut tree = LayoutNode::leaf(sorted[0].id);
        for p in &sorted[1..] {
            tree.split_at(sorted[0].id, p.id, SplitDir::Horizontal);
        }
        let layout = TabLayout {
            tab: tab_id,
            tree,
            active,
        };
        self.layouts.insert(tab_id, layout.clone());
        self.push_layout_changed(layout);
    }

    /// 把一个命令异步发送给 tmux（通过 channel）。
    /// execute 是同步 fn，命令发送走后台 task。
    fn replace_last_pending(&mut self, query: PendingQuery) {
        if let Some(last) = self.pending_queries.back_mut() {
            *last = query;
        }
    }

    fn dispatch_command(&mut self, line: String) -> std::io::Result<()> {
        let Some(tx) = self.cmd_tx.as_ref() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "tmux 命令通道未建立",
            ));
        };
        // UnboundedSender 只会在 sender task 已退出时失败，不会在快速键入/粘贴
        // 时返回 WouldBlock 丢掉 shell 输入；实际写入仍由后台 task 串行化。
        tx.send(line).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                format!("命令通道已关闭: {e}"),
            )
        })?;
        // 所有 control-mode 命令都按 FIFO 占一个响应槽；查询调用方会
        // 立即把最后一个占位替换成具体 PendingQuery。
        self.pending_queries.push_back(PendingQuery::Ignore);
        Ok(())
    }

    /// 便捷：发送一个 TmuxCommand。
    fn dispatch_tmux_command(&mut self, command: &cmd::TmuxCommand) -> std::io::Result<()> {
        self.dispatch_command(command.to_line())
    }
}

impl State for TmuxRuntime {
    fn workspace_name(&self) -> &str {
        &self.workspace_name
    }

    fn workspace_runtime(&self) -> &str {
        "tmux"
    }

    fn active_tab(&self) -> Option<&TabInfo> {
        self.tabs.iter().find(|t| t.active)
    }

    fn active_pane(&self) -> Option<&PaneInfo> {
        self.panes.iter().find(|p| p.active)
    }

    fn tabs(&self) -> Vec<&TabInfo> {
        self.tabs.iter().collect()
    }

    fn tab(&self, tab: &TabId) -> Option<&TabInfo> {
        self.tabs.iter().find(|t| &t.id == tab)
    }

    fn layout(&self, tab: &TabId) -> Option<&TabLayout> {
        self.layouts.get(tab)
    }

    fn panes(&self, tab: &TabId) -> Vec<&PaneInfo> {
        self.panes.iter().filter(|p| &p.tab == tab).collect()
    }

    fn pane(&self, pane: &PaneId) -> Option<&PaneInfo> {
        self.panes.iter().find(|p| &p.id == pane)
    }

    fn pane_output(&self, pane: &PaneId) -> Option<&[u8]> {
        self.outputs.get(pane).map(|v| v.as_slice())
    }

    fn status(&self) -> BackendStatus {
        self.status
    }
}

#[async_trait]
impl Runtime for TmuxRuntime {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn support(&self) -> &'static [RuntimeCapability] {
        &[
            RuntimeCapability::PersistDetach,
            RuntimeCapability::Discover,
            RuntimeCapability::MultiTab,
            RuntimeCapability::SplitPane,
            RuntimeCapability::SharedClientResize,
        ]
    }

    fn status_subscriptions_active(&self) -> bool {
        self.status_subscriptions_active
    }
    fn traffic_bytes(&self) -> (u64, u64) {
        self.traffic
            .as_ref()
            .map(|t| t.snapshot())
            .unwrap_or((0, 0))
    }
    async fn connect(&mut self) -> Result<()> {
        if self.status == BackendStatus::Connected {
            return Ok(());
        }
        self.status = BackendStatus::Connecting;
        self.events
            .push_back(StateChange::BackendStatusChanged(BackendStatus::Connecting));

        let config = self.config.clone();
        let (handle, rx) = TmuxClient::spawn(config)
            .await
            .context("spawn tmux -CC 失败")?;
        self.traffic = handle.traffic.clone();

        // 命令发送 channel + 后台 sender task（持有 handle）。
        // execute 同步 dispatch 命令到 cmd_tx；sender task 异步 send_command。
        // shutdown 时 drop cmd_tx 让 sender task 结束；handle 在 sender task 里，
        // shutdown 用 detach + 让 tmux 退出（kill 由 tmux 自然退出完成）。
        // 命令（尤其是逐字输入）必须按 FIFO 无损排队。bounded + try_send
        // 会在快速键入/粘贴时返回 WouldBlock，直接丢掉 shell 输入。
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<String>();
        let (command_error_tx, command_error_rx) = mpsc::unbounded_channel::<String>();
        let mut sender_handle = handle;
        let sender_join = tokio::spawn(async move {
            while let Some(line) = cmd_rx.recv().await {
                if let Err(error) = sender_handle.send_raw(&line).await {
                    let _ = command_error_tx.send(error.to_string());
                    break;
                }
            }
            // sender 结束后 detach + kill
            let _ = sender_handle.kill().await;
        });

        self.event_rx = Some(rx);
        self.cmd_tx = Some(cmd_tx);
        self.command_error_rx = Some(command_error_rx);
        self._sender_handle = Some(sender_join);
        self.handle = None; // handle 已 move 进 sender task

        // 等待 tmux 启动事件建立初始 state
        // new-session 模式：等 SessionChanged + WindowAdd
        // attach 模式：等 SessionChanged（window 不通过通知到达，需主动查询）
        let is_attach = matches!(self.config.mode, Some(ConnectMode::Attach { .. }));
        // CI 在 PersistDetach 之后立刻重新 attach：旧 control client 回收与
        // 新 `tmux -CC` 启动可能超过 5s；矩阵两格还共用同一个 `-L` socket。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            self.pump_events();
            if is_attach {
                // attach 模式只需 session 事件
                if self.active_session.is_some() {
                    break;
                }
            } else {
                // new-session 模式需 session + window
                if self.active_session.is_some() && !self.tabs.is_empty() {
                    break;
                }
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            // 短暂睡眠：仅 yield_now 忙等会饿死读循环，且拉长真实等待
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        if self.active_session.is_none() {
            self.status = BackendStatus::Error;
            self.events
                .push_back(StateChange::BackendStatusChanged(BackendStatus::Error));
            anyhow::bail!(
                "tmux 启动后未收到 session 事件 (mode={}, socket={})",
                if is_attach { "attach" } else { "new-session" },
                self.isolated_socket_name().unwrap_or("default"),
            );
        }

        // 主动查询所有 window + pane，建立完整初始 state（attach 已有 session 必需）
        self.query_list_windows();
        // 等待 list-windows 响应到达（最多 3 秒），拿到所有 window 列表后再等 pane 查询
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            self.pump_events();
            // 等 list-windows 响应到达：tabs 非空且 expected_panes_per_window 非空
            if !self.tabs.is_empty() && !self.expected_panes_per_window.is_empty() {
                break;
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        // 现在所有 window 的 pane 查询已发出（handle_list_windows_response 对每个
        // window 调了 query_list_panes）。连接只需要活动 tab 的拓扑即可交给
        // 前端；其它 tab 的 pane 列表继续在后台响应，不能让一个慢 pane 把
        // Connect/Cmd-Shift-P 卡住数秒。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            self.pump_events();
            if self.active_tab_topology_ready() {
                break;
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        // attach 的 capture 是异步 Surface seed：连接状态不再等待所有 pane
        // 的历史返回。活动 tab 的查询已经排队，前端在收到 PaneSnapshot 后
        // 播种；其它 tab 首次激活时再按需查询。
        if is_attach {
            tracing::info!(
                target: "muxterm::tmux::seed",
                active_tab = self.active_tab_id().map(|tab| tab.0),
                pending = self.initial_capture_pending.len(),
                "attach 首屏 capture 已异步排队"
            );
        }
        self.attach_bootstrap_complete = is_attach;

        // 查询所有 session（用于 list-sessions 列出 server 上所有 session）
        self.query_list_sessions();
        self.detect_colour_report_support();
        self.setup_status_subscriptions();

        self.status = BackendStatus::Connected;
        self.events
            .push_back(StateChange::BackendStatusChanged(BackendStatus::Connected));
        Ok(())
    }

    fn execute(&mut self, task: &Task) -> Result<TaskOutcome> {
        if self.cmd_tx.is_none() || self.status != BackendStatus::Connected {
            return Ok(TaskOutcome::Rejected {
                reason: "tmux 未连接".into(),
            });
        }
        let outcome = match task {
            Task::SplitPane {
                target,
                dir,
                command,
                workdir,
            } => {
                let target =
                    target.unwrap_or_else(|| self.active_pane().map(|p| p.id).unwrap_or(PaneId(0)));
                if self.pane(&target).is_none() {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                }
                let direction = match dir {
                    SplitDir::Horizontal => cmd::SplitDirection::Horizontal,
                    SplitDir::Vertical => cmd::SplitDirection::Vertical,
                };
                let name = command.as_ref().and_then(|c| c.first()).map(|s| s.as_str());
                match workdir {
                    Some(dir) => {
                        // 显式指定目录：直接 split -c。
                        let c = cmd::split_window(target, direction, name, Some(dir));
                        tracing::debug!(
                            target: "muxterm::tmux",
                            pane = target.0,
                            command = %c.as_str(),
                            "dispatch split pane"
                        );
                        if self.dispatch_tmux_command(&c).is_err() {
                            return Ok(TaskOutcome::Rejected {
                                reason: "发送命令失败".into(),
                            });
                        }
                    }
                    None => {
                        // tmux 会按精确 target pane 展开此 format；一条命令同时锁定
                        // pane 与 cwd，避免异步 display-message 回来时焦点/映射已变化。
                        let c = cmd::split_window(
                            target,
                            direction,
                            name,
                            Some("#{pane_current_path}"),
                        );
                        tracing::debug!(
                            target: "muxterm::tmux",
                            pane = target.0,
                            command = %c.as_str(),
                            "dispatch split pane"
                        );
                        if self.dispatch_tmux_command(&c).is_err() {
                            return Ok(TaskOutcome::Rejected {
                                reason: "发送命令失败".into(),
                            });
                        }
                    }
                }
                TaskOutcome::Done
            }
            Task::ClosePane { target } => {
                let c = cmd::kill_pane(*target);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::SwitchPane { target } => {
                let c = cmd::select_pane(*target);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::NextPane | Task::PrevPane => {
                // 不能拼 `select-pane -t @N -N/-P`：PaneId Display 是 @N（window
                // 语法），且 tmux 3.7 的 -N 不存在、 -P 是设 pane 样式。相对
                // 当前 window 循环用 :.+ / :.-。
                let c = if matches!(task, Task::NextPane) {
                    cmd::select_pane_next()
                } else {
                    cmd::select_pane_prev()
                };
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::RenameWorkspace { name } => {
                let Some(sess) = self.active_session else {
                    return Ok(TaskOutcome::Rejected {
                        reason: "tmux 未连接".into(),
                    });
                };
                let c = cmd::rename_session(sess, name);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::SendKeys { target, keys } => {
                let tmux_keys: Vec<cmd::Key> = keys.iter().map(key_event_to_tmux_key).collect();
                let c = cmd::send_keys(*target, &tmux_keys);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::WriteRaw { target, data } => {
                if self.is_attach_mode()
                    && (!self.initial_capture_done.contains(target)
                        || self.deferred_write_panes.contains(target)
                        || self.awaiting_pane_ready.contains(target)
                        || self.new_attach_panes.contains(target))
                {
                    self.pending_writes
                        .entry(*target)
                        .or_default()
                        .extend_from_slice(data);
                    if self.initial_capture_done.contains(target)
                        && self.new_attach_panes.contains(target)
                    {
                        self.start_pane_ready_probe(*target);
                    }
                    return Ok(TaskOutcome::Done);
                }
                let c = cmd::send_keys_bytes(*target, data);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::ReportPaneColours { target, fg, bg } => {
                if !self.colour_report_supported {
                    if !self.colour_report_warned {
                        self.colour_report_warned = true;
                        tracing::debug!(
                            target = "muxterm::tmux",
                            "tmux 不支持 refresh-client -r，跳过颜色上报"
                        );
                    }
                    return Ok(TaskOutcome::Done);
                }
                // tmux 用这两个颜色代答 pane 的 OSC 10/11 查询；必须分两次
                // 上报（一次 fg、一次 bg），tmux 每次只解析一条 OSC。
                let fg_cmd = cmd::refresh_client_colour(*target, 10, *fg);
                let bg_cmd = cmd::refresh_client_colour(*target, 11, *bg);
                if self.dispatch_tmux_command(&fg_cmd).is_err()
                    || self.dispatch_tmux_command(&bg_cmd).is_err()
                {
                    return Ok(TaskOutcome::Rejected {
                        reason: "上报 pane 颜色失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::ResizePane { target, cols, rows } => {
                let c = cmd::resize_pane(*target, Some(*cols as u32), Some(*rows as u32));
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::ResizeClient { cols, rows } => {
                let c = cmd::refresh_client_size(*cols as u32, *rows as u32);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送 client resize 命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::ResizePaneAxis { target, dir, size } => {
                let c = match dir {
                    SplitDir::Horizontal => cmd::resize_pane(*target, Some(*size as u32), None),
                    SplitDir::Vertical => cmd::resize_pane(*target, None, Some(*size as u32)),
                };
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送 pane 轴向 resize 命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::ResizePaneStep { target, dir, delta } => {
                let flag = match dir {
                    SplitDir::Horizontal => 'W',
                    SplitDir::Vertical => 'H',
                };
                let sign = if *delta >= 0 { 'U' } else { 'D' };
                let amount = delta.unsigned_abs();
                let c = cmd::TmuxCommand::from_raw(format!(
                    "resize-pane -t {} -{}{} {}",
                    target, flag, sign, amount
                ));
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::TogglePaneFullscreen { target } => {
                let c = cmd::zoom_pane(*target);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }
            Task::MoveTab {
                from,
                target,
                before,
            } => {
                if from == target {
                    return Ok(TaskOutcome::Done);
                }
                let c = cmd::move_window(*from, *target, *before);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                // move-window 可能只推 unlink 通知；紧随 mutation 的权威查询
                // 恢复被临时移除的 tab，并同步新的 index 顺序。
                self.query_list_windows();
                TaskOutcome::Done
            }
            Task::BreakPane { target } => {
                let c = cmd::break_pane(*target);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                // break-pane 会同时改变 window 与 pane 归属；按命令队列顺序
                // 查询，响应里再为每个 tab 查询 panes。
                self.query_list_windows();
                TaskOutcome::Done
            }
            Task::RefreshTabs => {
                // 外部 tmux 变更后强制重查 window/pane，同步 GUI 标签。
                self.query_list_windows();
                self.query_list_sessions();
                TaskOutcome::Done
            }
            Task::NewTab { name, .. } => {
                // tmux 的 tab = tmux window，新建 tab = 新建 tmux window
                let Some(sess) = self.active_session else {
                    return Ok(TaskOutcome::Rejected {
                        reason: "tmux 未连接".into(),
                    });
                };
                let c = cmd::new_window(sess, name.as_deref());
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }

            Task::CloseTab { target } => {
                // tmux tab = tmux window，关闭 tab = kill-window
                let c = cmd::kill_window(*target);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }

            Task::SwitchTab { target } => {
                let c = cmd::select_window(*target);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                // 乐观更新 active tab：tmux 在输出洪峰下可能延迟回
                // %session-window-changed，前端等太久会以为切 tab 不生效。
                // 真正的通知到达后 mark_tab_active 幂等，不会重复切换。
                self.mark_tab_active(*target);
                self.query_panes_if_empty(*target);
                TaskOutcome::Done
            }

            Task::RenameTab { target, name } => {
                let c = cmd::rename_window(*target, name);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                TaskOutcome::Done
            }

            Task::Detach => {
                // 显式 detach 只关闭当前 control client，不杀 tmux server/session。
                let Some(sess) = self.active_session else {
                    return Ok(TaskOutcome::Rejected {
                        reason: "tmux 未连接".into(),
                    });
                };
                let c = cmd::detach_client(sess);
                if self.dispatch_tmux_command(&c).is_err() {
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送 detach-client 失败".into(),
                    });
                }
                // 关闭发送 channel：sender 会先写完已排队的 detach-client，
                // 然后只回收 `tmux -CC` control client，不触碰 session。
                self.cmd_tx.take();
                self.status = BackendStatus::Disconnected;
                self.events.push_back(StateChange::BackendStatusChanged(
                    BackendStatus::Disconnected,
                ));
                TaskOutcome::Done
            }

            Task::Shutdown => {
                // 生命周期清理仍使用独立的 shutdown 状态；正常的 tmux
                // shutdown 也先 detach control client，再回收本地进程句柄。
                let Some(sess) = self.active_session else {
                    return Ok(TaskOutcome::Rejected {
                        reason: "tmux 未连接".into(),
                    });
                };
                let c = cmd::detach_client(sess);
                let _ = self.dispatch_tmux_command(&c);
                self.status = BackendStatus::Exited;
                self.events
                    .push_back(StateChange::BackendStatusChanged(BackendStatus::Exited));
                TaskOutcome::Done
            }
        };
        Ok(outcome)
    }

    fn take_events(&mut self) -> Vec<StateChange> {
        self.pump_events();
        let events: Vec<StateChange> = self.events.drain(..).collect();
        if self.events.is_empty() {
            for flow in self.flow.values_mut() {
                flow.queued_at = None;
            }
        }
        events
    }

    async fn shutdown(&mut self) -> Result<()> {
        // 已经由显式 Task::Detach 关闭 channel 时，不再重复发送命令。
        if self.cmd_tx.is_some() {
            self.execute(&Task::Shutdown)?;
        }
        // 关闭命令通道，sender task 收到 None 后会 kill tmux 子进程并退出
        self.cmd_tx.take();
        // 等待 sender task 结束；pty 写卡死时 abort，避免测试/CI 无限挂起。
        // 超时不得 `kill-server`：矩阵的 primary/alternate 共用隔离 socket，
        // PersistDetach 之后还要 attach 回同一 session。残留 `tmux -CC` 由
        // sender 的 kill() 或测试 fixture Drop 回收。
        if let Some(mut h) = self._sender_handle.take() {
            tokio::select! {
                r = &mut h => {
                    let _ = r;
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(8)) => {
                    tracing::warn!(target = "muxterm::tmux_backend", "shutdown: sender task 超时，abort");
                    h.abort();
                }
            }
        }
        // 丢掉事件接收端，让读线程/读 task 在 send 失败后退出，停止无界积压
        self.event_rx.take();
        self.outputs.clear();
        self.events.clear();
        self.status = BackendStatus::Exited;
        self.events
            .push_back(StateChange::BackendStatusChanged(BackendStatus::Exited));
        Ok(())
    }
}

/// 解析 list-windows -F 单行。
///
/// 格式：`@N,name,active,LAYOUT,panes`
/// LAYOUT 含逗号，因此前三个字段用 `split_once`，最后一个用 `rsplit_once`。
fn parse_list_windows_line(line: &str) -> Option<(TabId, String, bool, String, usize, bool)> {
    let (id_str, rest) = line.split_once(',')?;
    let (name, rest) = rest.split_once(',')?;
    let (active_str, rest) = rest.split_once(',')?;
    let (layout_and_panes, zoomed_str) = rest.rsplit_once(',')?;
    let (layout_str, panes_str) = layout_and_panes.rsplit_once(',')?;
    let tab = TabId::parse(id_str).ok()?;
    let active = active_str == "1";
    let panes_count = panes_str.parse().ok()?;
    let zoomed = zoomed_str == "1";
    Some((
        tab,
        name.to_string(),
        active,
        layout_str.to_string(),
        panes_count,
        zoomed,
    ))
}

/// tmux zoom：`window_raw_flags` 含 `Z`，或 visible 树是单叶而完整树是 split。
fn window_is_zoomed(flags: Option<&str>, layout_raw: &str, visible_raw: Option<&str>) -> bool {
    if flags.is_some_and(|f| f.contains('Z')) {
        return true;
    }
    let Some(visible_raw) = visible_raw else {
        return false;
    };
    let Ok(full) = parse_layout_tree(layout_raw) else {
        return false;
    };
    let Ok(visible) = parse_layout_tree(visible_raw) else {
        return false;
    };
    collect_layout_leaves(&full).len() > 1 && collect_layout_leaves(&visible).len() == 1
}

/// 把 LayoutTree（几何拓扑）转成 LayoutNode（pane id 树），按几何位置匹配。
fn layout_tree_to_node(tree: &LayoutTree, panes: &[PaneInfo]) -> Option<LayoutNode> {
    let leaves = collect_layout_leaves(tree);
    if leaves.len() != panes.len() {
        return None;
    }
    // 优先用 layout 叶子的 flags（tmux pane index）映射 PaneId
    let pane_by_idx: HashMap<u32, PaneId> = panes.iter().map(|p| (p.id.0, p.id)).collect();
    let mut mapping = HashMap::new();
    let mapped_by_flags = leaves.iter().all(|leaf| {
        if let Some(&pid) = pane_by_idx.get(&leaf.flags) {
            mapping.insert((leaf.x, leaf.y), pid);
            true
        } else {
            false
        }
    });
    if !mapped_by_flags {
        mapping.clear();
        for (leaf, pane) in leaves.iter().zip(panes.iter()) {
            mapping.insert((leaf.x, leaf.y), pane.id);
        }
    }
    layout_tree_to_node_inner(tree, &mapping)
}

fn collect_layout_leaves(tree: &LayoutTree) -> Vec<&LayoutTree> {
    match &tree.children {
        None => vec![tree],
        Some((a, b)) => {
            let mut v = collect_layout_leaves(a);
            v.extend(collect_layout_leaves(b));
            v
        }
    }
}

fn layout_tree_to_node_inner(
    tree: &LayoutTree,
    mapping: &HashMap<(u32, u32), PaneId>,
) -> Option<LayoutNode> {
    match &tree.children {
        None => mapping
            .get(&(tree.x, tree.y))
            .map(|&pid| LayoutNode::leaf(pid)),
        Some((a, b)) => {
            let first = layout_tree_to_node_inner(a, mapping)?;
            let second = layout_tree_to_node_inner(b, mapping)?;
            Some(LayoutNode::Split {
                dir: tree.dir,
                ratio: layout_split_ratio(tree, a, b),
                first: Box::new(first),
                second: Box::new(second),
            })
        }
    }
}

/// 从 tmux 子节点几何计算 first 的布局比例（0..=1000）。
///
/// tmux 的 layout 几何包含分隔线两侧的 pane 尺寸，因此用两个子节点在
/// 当前分割轴上的尺寸计算比例即可得到稳定的近似值；不能固定写成 500，
/// 否则 attach 后的非对称布局会被 GUI 重新均分。
fn layout_split_ratio(tree: &LayoutTree, first: &LayoutTree, second: &LayoutTree) -> u16 {
    let (first_size, second_size) = match tree.dir {
        SplitDir::Horizontal => (first.cols, second.cols),
        SplitDir::Vertical => (first.rows, second.rows),
    };
    let total = first_size.saturating_add(second_size);
    if total == 0 {
        return 500;
    }
    ((first_size.saturating_mul(1000) / total).clamp(50, 950)) as u16
}

/// 从默认格式的 list-panes 行提取 pane id。
///
/// 格式：`0:1.1: [80x24] [history ...] %0 (active)`
/// 提取 `%0` 部分 → PaneId(0)
fn extract_pane_id_from_default(line: &str) -> Option<PaneId> {
    // 找 %N token（pane id）
    for token in line.split_whitespace() {
        if token.starts_with('%') && token.len() > 1 {
            let num = &token[1..];
            // 去除尾部非数字字符（如 "%0(active)" 或 "%0")
            let digits: String = num.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse::<u32>() {
                return Some(PaneId(n));
            }
        }
    }
    None
}

/// 从默认格式的 list-panes 行提取尺寸。
///
/// 格式：`... [80x24] ...` → (80, 24)
fn extract_size_from_default(line: &str) -> (u16, u16) {
    // 找 [WxH] 模式
    if let Some(start) = line.find('[') {
        if let Some(end) = line[start..].find(']') {
            let inside = &line[start + 1..start + end];
            if let Some((w, h)) = inside.split_once('x') {
                return (w.parse().unwrap_or(80), h.parse().unwrap_or(24));
            }
        }
    }
    (80, 24)
}

/// 把抽象 KeyEvent 转成 tmux Key。
fn key_event_to_tmux_key(ev: &crate::core::protocol::terminal::input::KeyEvent) -> cmd::Key {
    use crate::core::protocol::terminal::input::{ArrowDir, KeyEvent};
    match ev {
        KeyEvent::Char(c) => cmd::Key::Literal(c.to_string()),
        KeyEvent::Enter => cmd::Key::enter(),
        KeyEvent::Tab => cmd::Key::tab(),
        KeyEvent::Backspace => cmd::Key::bspace(),
        KeyEvent::Escape => cmd::Key::escape(),
        KeyEvent::Ctrl(c) => cmd::Key::ctrl(*c),
        KeyEvent::Alt(c) => cmd::Key::Literal(format!("\x1b{}", c)),
        KeyEvent::Function(n) => match n {
            1 => cmd::Key::Special("F1"),
            2 => cmd::Key::Special("F2"),
            3 => cmd::Key::Special("F3"),
            4 => cmd::Key::Special("F4"),
            5 => cmd::Key::Special("F5"),
            6 => cmd::Key::Special("F6"),
            7 => cmd::Key::Special("F7"),
            8 => cmd::Key::Special("F8"),
            9 => cmd::Key::Special("F9"),
            10 => cmd::Key::Special("F10"),
            11 => cmd::Key::Special("F11"),
            12 => cmd::Key::Special("F12"),
            _ => cmd::Key::Literal(String::new()),
        },
        KeyEvent::Arrow(d) => match d {
            ArrowDir::Up => cmd::Key::up(),
            ArrowDir::Down => cmd::Key::down(),
            ArrowDir::Left => cmd::Key::left(),
            ArrowDir::Right => cmd::Key::right(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn muxterm_new_ssh_socket_is_alias_not_remote_dash_l() {
        let (alias, socket) = TmuxRuntime::ssh_alias_and_tmux_socket(Some("ryzen"), None).unwrap();
        assert_eq!(alias, "ryzen");
        assert_eq!(
            socket, None,
            "muxterm_new 的 socket=ryzen 不得变成 tmux -L ryzen"
        );
        let rt = TmuxRuntime::new_ssh_attach(&alias, socket.as_deref(), "yaklang-workspace");
        assert_eq!(rt.test_ssh_alias(), Some("ryzen"));
        assert!(
            !rt.test_extra_args()
                .windows(2)
                .any(|w| w == ["-L", "ryzen"]),
            "远端命令必须是 `tmux -CC attach -t yaklang-workspace`，不能带 -L ryzen: {:?}",
            rt.test_extra_args()
        );
    }

    #[test]
    fn muxterm_new_connect_keeps_isolated_remote_socket() {
        let (alias, socket) = TmuxRuntime::ssh_alias_and_tmux_socket(
            Some("muxterm-test-remote-feat"),
            Some("test-feat-ssh"),
        )
        .unwrap();
        assert_eq!(alias, "test-feat-ssh");
        assert_eq!(socket.as_deref(), Some("muxterm-test-remote-feat"));
        let rt = TmuxRuntime::new_ssh_attach(&alias, socket.as_deref(), "featssh");
        assert_eq!(rt.test_extra_args(), ["-L", "muxterm-test-remote-feat"]);
        assert!(!rt.test_extra_args().iter().any(|a| a == "test-feat-ssh"));
    }

    #[test]
    fn identical_alias_and_socket_is_not_dash_l() {
        let (alias, socket) =
            TmuxRuntime::ssh_alias_and_tmux_socket(Some("ryzen"), Some("ryzen")).unwrap();
        assert_eq!(alias, "ryzen");
        assert_eq!(
            socket, None,
            "alias 被同时塞进 socket 时仍不得生成 -L ryzen（macOS CoreBridge.init 旧路径）"
        );
    }

    #[test]
    fn parse_list_windows_line_keeps_full_layout_with_commas() {
        let line =
            "@1,zsh,1,d67e,80x24,0,0{40x24,0,0,0,39x24,41,0[39x12,41,0,1,39x11,41,13,2]},3,0";
        let (wid, name, active, layout, panes, zoomed) = parse_list_windows_line(line).unwrap();
        assert_eq!(wid, TabId(1));
        assert_eq!(name, "zsh");
        assert!(active);
        assert_eq!(
            layout,
            "d67e,80x24,0,0{40x24,0,0,0,39x24,41,0[39x12,41,0,1,39x11,41,13,2]}"
        );
        assert_eq!(panes, 3);
        assert!(!zoomed);
        // 完整 layout 应能解析出嵌套 vertical
        let tree = parse_layout_tree(&layout).unwrap();
        assert_eq!(tree.dir, crate::core::model::layout::SplitDir::Horizontal);
        let right = tree.children.as_ref().unwrap().1.as_ref();
        assert_eq!(right.dir, crate::core::model::layout::SplitDir::Vertical);
    }

    #[test]
    fn parse_list_windows_line_reads_zoomed_flag() {
        let line = "@2,codex,1,bbcd,80x24,0,0,1,1,1";
        let (_, name, _, layout, panes, zoomed) = parse_list_windows_line(line).unwrap();
        assert_eq!(name, "codex");
        assert_eq!(layout, "bbcd,80x24,0,0,1");
        assert_eq!(panes, 1);
        assert!(zoomed);
    }

    #[test]
    fn parse_list_windows_line_rejects_short() {
        assert!(parse_list_windows_line("@1,name").is_none());
        assert!(parse_list_windows_line("").is_none());
    }

    #[test]
    fn command_queue_accepts_high_frequency_input_without_would_block() {
        let mut backend = TmuxRuntime::new(None);
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        backend.cmd_tx = Some(tx);
        backend.status = BackendStatus::Connected;

        let burst = 4_096;
        for _ in 0..burst {
            let outcome = backend
                .execute(&Task::WriteRaw {
                    target: PaneId(1),
                    data: b"x".to_vec(),
                })
                .unwrap();
            assert_eq!(outcome, TaskOutcome::Done);
        }

        let mut received = 0;
        while rx.try_recv().is_ok() {
            received += 1;
        }
        assert_eq!(received, burst, "高频输入不能因队列满而丢失");
    }

    #[test]
    fn attach_new_pane_write_waits_for_two_probe_rounds() {
        let pane = PaneId(7);
        let mut backend = TmuxRuntime::new_with_attach(None, "existing");
        backend.status = BackendStatus::Connected;
        backend.initial_capture_done.insert(pane);
        backend.new_attach_panes.insert(pane);
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        backend.cmd_tx = Some(tx);

        let outcome = backend
            .execute(&Task::WriteRaw {
                target: pane,
                data: b"printf ready\r".to_vec(),
            })
            .unwrap();
        assert_eq!(outcome, TaskOutcome::Done);
        assert!(rx.try_recv().is_err(), "首个输入必须先留在 pending_writes");

        backend.poll_ready_probes();
        let probe = rx.try_recv().expect("应先发 readiness probe");
        assert!(probe.contains("send-keys -t %7"));
        assert!(probe.contains("Enter"));

        backend.ready_probe_acknowledged.insert(pane);
        backend.mark_pane_ready(pane);
        assert!(
            rx.try_recv().is_err(),
            "单个 probe round 不足以释放 attach 输入"
        );
        backend.ready_probe_acknowledged.insert(pane);
        backend.mark_pane_ready(pane);
        let write = rx.try_recv().expect("第二个 probe round 后应发送用户输入");
        assert!(write.contains("send-keys -t %7 -H"));
        assert!(write.contains("70 72 69 6e 74 66"));
    }

    #[test]
    fn attach_write_waits_for_capture_completion_before_dispatch() {
        let pane = PaneId(12);
        let mut backend = TmuxRuntime::new_with_attach(None, "existing");
        backend.status = BackendStatus::Connected;
        backend.initial_capture_pending.insert(pane);
        backend
            .pending_by_number
            .insert(41, PendingQuery::CapturePane { pane });
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        backend.cmd_tx = Some(tx);

        backend
            .execute(&Task::WriteRaw {
                target: pane,
                data: b"echo capture-ready\r".to_vec(),
            })
            .unwrap();
        assert!(backend.pending_writes.contains_key(&pane));
        assert!(rx.try_recv().is_err(), "capture 未完成前不得发送输入");

        backend.dispatch_response(41, vec!["prompt".into()]);
        assert!(backend.initial_capture_done.contains(&pane));
        assert!(backend.deferred_write_panes.contains(&pane));
        assert!(
            rx.try_recv().is_err(),
            "capture response 所在 poll 不能和快照同批发送输入"
        );

        backend.release_deferred_writes();
        let write = rx.try_recv().expect("下一轮才应发送暂存输入");
        assert!(write.contains("send-keys -t %12 -H"));
        assert!(write.contains("65 63 68 6f"));
    }

    #[test]
    fn attach_new_pane_requires_probe_ack_before_counting_output() {
        let pane = PaneId(13);
        let mut backend = TmuxRuntime::new_with_attach(None, "existing");
        backend.status = BackendStatus::Connected;
        backend.initial_capture_done.insert(pane);
        backend.new_attach_panes.insert(pane);
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        backend.cmd_tx = Some(tx);

        backend
            .execute(&Task::WriteRaw {
                target: pane,
                data: b"echo probe-ack\r".to_vec(),
            })
            .unwrap();
        backend.poll_ready_probes();
        let _first_probe = rx.try_recv().expect("应先发送第一轮 probe");

        // pane output 先到、probe response 尚未确认时，不能误判 shell ready。
        backend.handle_message(Message::Output {
            pane,
            content: b"early".to_vec(),
            raw_content: "early".into(),
        });
        assert!(rx.try_recv().is_err(), "未收到 probe ack 时不能释放输入");

        backend
            .pending_by_number
            .insert(51, PendingQuery::ReadyProbe { pane });
        backend.dispatch_response(51, Vec::new());
        backend.handle_message(Message::Output {
            pane,
            content: b"round-1".to_vec(),
            raw_content: "round-1".into(),
        });
        assert!(
            rx.try_recv().is_err(),
            "只有一轮 probe output 仍不能释放输入"
        );

        backend
            .pending_by_number
            .insert(52, PendingQuery::ReadyProbe { pane });
        backend.dispatch_response(52, Vec::new());
        backend.handle_message(Message::Output {
            pane,
            content: b"round-2".to_vec(),
            raw_content: "round-2".into(),
        });
        let write = rx.try_recv().expect("第二轮 ack/output 后应发送输入");
        assert!(write.contains("send-keys -t %13 -H"));
    }

    #[test]
    fn attach_new_pane_probe_error_is_retried() {
        let pane = PaneId(14);
        let mut backend = TmuxRuntime::new_with_attach(None, "existing");
        backend.status = BackendStatus::Connected;
        backend.initial_capture_done.insert(pane);
        backend.new_attach_panes.insert(pane);
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        backend.cmd_tx = Some(tx);

        backend
            .execute(&Task::WriteRaw {
                target: pane,
                data: b"echo retry\r".to_vec(),
            })
            .unwrap();
        backend.poll_ready_probes();
        let _probe = rx.try_recv().expect("应发送 probe");
        backend
            .pending_by_number
            .insert(61, PendingQuery::ReadyProbe { pane });
        backend.handle_response_error(61);
        assert!(!backend.ready_probe_in_flight.contains(&pane));
        assert!(
            backend
                .ready_probe_at
                .get(&pane)
                .is_some_and(|at| *at <= Instant::now()),
            "probe error 后必须安排下一次重试"
        );
        backend.poll_ready_probes();
        assert!(
            rx.try_recv().is_ok(),
            "probe error 后下一轮必须重新发送 probe"
        );
    }

    #[test]
    fn closing_attach_pane_clears_pending_input_and_probe_state() {
        let pane = PaneId(15);
        let mut backend = TmuxRuntime::new_with_attach(None, "existing");
        backend.status = BackendStatus::Connected;
        backend.attach_bootstrap_complete = true;
        backend.panes.push(PaneInfo {
            id: pane,
            tab: TabId(3),
            active: true,
            title: String::new(),
            cols: 80,
            rows: 24,
        });
        backend.pending_writes.insert(pane, b"stale".to_vec());
        backend.deferred_write_panes.insert(pane);
        backend.awaiting_pane_ready.insert(pane);
        backend.ready_probe_at.insert(pane, Instant::now());
        backend.ready_probe_in_flight.insert(pane);
        backend.ready_probe_acknowledged.insert(pane);
        backend.ready_probe_rounds.insert(pane, 1);
        backend.new_attach_panes.insert(pane);

        backend.handle_list_panes_response(
            TabId(3),
            vec!["0: [80x24] [history 0/2000, 0 bytes] %16 (active)".into()],
        );

        assert!(!backend.pending_writes.contains_key(&pane));
        assert!(!backend.deferred_write_panes.contains(&pane));
        assert!(!backend.awaiting_pane_ready.contains(&pane));
        assert!(!backend.ready_probe_at.contains_key(&pane));
        assert!(!backend.ready_probe_in_flight.contains(&pane));
        assert!(!backend.ready_probe_acknowledged.contains(&pane));
        assert!(!backend.ready_probe_rounds.contains_key(&pane));
        assert!(!backend.new_attach_panes.contains(&pane));
    }

    #[test]
    fn unlinked_window_close_closes_tab() {
        let mut b = TmuxRuntime::new(None);
        // 先加两个 tab
        b.handle_message(Message::WindowAdd { window: TabId(0) });
        b.handle_message(Message::WindowAdd { window: TabId(1) });
        assert!(b.tabs.iter().any(|t| t.id == TabId(1)));

        // 实测：kill-window 时控制客户端收到 %unlinked-window-close
        b.handle_message(Message::UnlinkedWindowClose { window: TabId(1) });

        assert!(!b.tabs.iter().any(|t| t.id == TabId(1)), "tab1 应被关闭");
        assert!(b.tabs.iter().any(|t| t.id == TabId(0)), "tab0 应保留");
        assert!(b
            .events
            .iter()
            .any(|e| matches!(e, StateChange::TabClosed { tab } if *tab == TabId(1))));
    }

    #[test]
    fn window_add_waits_for_real_panes_instead_of_publishing_fake_layout() {
        let mut backend = TmuxRuntime::new(None);
        backend.handle_message(Message::WindowAdd { window: TabId(7) });

        assert!(backend.tabs.iter().any(|tab| tab.id == TabId(7)));
        assert!(
            !backend.layouts.contains_key(&TabId(7)),
            "list-panes 返回前不得发布假的 PaneId(0) layout"
        );
        assert!(backend.panes.iter().all(|pane| pane.tab != TabId(7)));
    }

    #[test]
    fn unlinked_window_renamed_updates_tab_name() {
        let mut b = TmuxRuntime::new(None);
        b.handle_message(Message::WindowAdd { window: TabId(0) });

        b.handle_message(Message::UnlinkedWindowRenamed {
            window: TabId(0),
            name: "renamed-tab".into(),
        });

        let tab = b.tabs.iter().find(|t| t.id == TabId(0)).unwrap();
        assert_eq!(tab.name, "renamed-tab");
        assert!(b.events.iter().any(|e| matches!(
            e,
            StateChange::TabRenamed { tab, name } if *tab == TabId(0) && name == "renamed-tab"
        )));
    }

    #[test]
    fn subscription_changed_forwards_to_state_change() {
        let mut b = TmuxRuntime::new(None);
        b.handle_message(Message::SubscriptionChanged {
            name: STATUS_LEFT_SUBSCRIPTION.into(),
            value: "#[fg=red]11:50:23 ".into(),
            pane: None,
        });
        assert!(b.events.iter().any(|event| matches!(
            event,
            StateChange::StatusBarSubscription { name, value, pane: None }
                if name == STATUS_LEFT_SUBSCRIPTION && value == "#[fg=red]11:50:23 "
        )));
    }

    #[test]
    fn supports_status_subscription_requires_tmux_3_2() {
        assert!(supports_status_subscription(Some((3, 2))));
        assert!(supports_status_subscription(Some((3, 7))));
        assert!(supports_status_subscription(Some((4, 0))));
        assert!(!supports_status_subscription(Some((3, 1))));
        assert!(!supports_status_subscription(Some((2, 9))));
        assert!(!supports_status_subscription(None));
    }

    #[test]
    fn setup_status_subscriptions_sends_three_subscriptions_when_supported() {
        let mut backend = TmuxRuntime::new(None);
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        backend.cmd_tx = Some(tx);
        backend.status_subscription_supported = true;

        backend.setup_status_subscriptions();

        assert!(backend.status_subscriptions_active);
        let mut lines = Vec::new();
        while let Ok(line) = rx.try_recv() {
            lines.push(line);
        }
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().any(|l| l.contains("muxterm.status-left")));
        assert!(lines.iter().any(|l| l.contains("muxterm.status-right")));
        assert!(lines.iter().any(|l| l.contains("muxterm.pane-cmd")));
        assert!(lines.iter().all(|l| l.starts_with("refresh-client -B \"")));
    }

    #[test]
    fn setup_status_subscriptions_skips_unsupported_tmux() {
        let mut backend = TmuxRuntime::new(None);
        let (tx, rx) = mpsc::unbounded_channel::<String>();
        backend.cmd_tx = Some(tx);
        backend.status_subscription_supported = false;

        backend.setup_status_subscriptions();

        assert!(!backend.status_subscriptions_active);
        assert!(rx.is_empty());
    }

    #[test]
    fn asynchronous_command_error_becomes_backend_status_instead_of_panic() {
        let mut backend = TmuxRuntime::new(None);
        let (tx, rx) = mpsc::unbounded_channel::<String>();
        backend.command_error_rx = Some(rx);
        tx.send("pty 已关闭".into()).unwrap();

        backend.pump_events();

        assert_eq!(backend.status, BackendStatus::Error);
        assert!(backend.events.iter().any(|event| matches!(
            event,
            StateChange::BackendStatusChanged(BackendStatus::Error)
        )));
    }

    #[test]
    fn layout_change_rebuilds_from_latest_nested_tmux_tree() {
        let mut b = TmuxRuntime::new(None);
        let window = TabId(0);
        let latest = "1268,140x30,0,0{70x30,0,0,0,69x30,71,0[69x15,71,0,1,69x14,71,16,2]}";

        b.handle_message(Message::LayoutChange {
            window,
            layout: crate::core::runtime::tmux::protocol::LayoutChange::parse(latest).unwrap(),
            visible_layout: None,
            flags: None,
        });

        // 没有命令通道的单元测试里 query_list_panes 会被跳过，但最新 raw
        // 仍必须保留下来，随后 list-panes 响应才能按新树建模。
        assert_eq!(
            b.window_layouts.get(&window).map(String::as_str),
            Some(latest)
        );

        b.handle_list_panes_response(
            window,
            vec![
                "0: [70x30] %0 (active)".into(),
                "1: [69x15] %1".into(),
                "2: [69x14] %2".into(),
            ],
        );

        let tree = &b.layouts[&TabId(0)].tree;
        let LayoutNode::Split {
            dir: SplitDir::Horizontal,
            ratio: root_ratio,
            first,
            second,
        } = tree
        else {
            panic!("根节点应为左右 split: {tree:?}");
        };
        assert!((500..=510).contains(root_ratio));
        assert!(matches!(first.as_ref(), LayoutNode::Leaf(PaneId(0))));
        let LayoutNode::Split {
            dir: SplitDir::Vertical,
            ratio: nested_ratio,
            first: nested_first,
            second: nested_second,
        } = second.as_ref()
        else {
            panic!("右子树应为上下 split: {second:?}");
        };
        assert!((510..=525).contains(nested_ratio));
        assert!(matches!(nested_first.as_ref(), LayoutNode::Leaf(PaneId(1))));
        assert!(matches!(
            nested_second.as_ref(),
            LayoutNode::Leaf(PaneId(2))
        ));
    }

    #[test]
    fn layout_change_zoom_collapses_to_active_pane() {
        let mut b = TmuxRuntime::new(None);
        let window = TabId(0);
        let full = "aabd,80x24,0,0{40x24,0,0,1,39x24,41,0,2}";
        let visible = "bbcd,80x24,0,0,1";
        b.handle_message(Message::LayoutChange {
            window,
            layout: crate::core::runtime::tmux::protocol::LayoutChange::parse(full).unwrap(),
            visible_layout: Some(
                crate::core::runtime::tmux::protocol::LayoutChange::parse(visible).unwrap(),
            ),
            flags: Some("*Z".into()),
        });
        b.handle_list_panes_response(
            window,
            vec!["1: [40x24] %1 (active)".into(), "2: [39x24] %2".into()],
        );
        let tree = &b.layouts[&TabId(0)].tree;
        assert!(
            matches!(tree, LayoutNode::Leaf(PaneId(1))),
            "zoom 后应只渲染 active pane，实际 {tree:?}"
        );

        // 取消 zoom：flags 不再含 Z，恢复完整 split。
        b.handle_message(Message::LayoutChange {
            window,
            layout: crate::core::runtime::tmux::protocol::LayoutChange::parse(full).unwrap(),
            visible_layout: Some(
                crate::core::runtime::tmux::protocol::LayoutChange::parse(full).unwrap(),
            ),
            flags: Some("*".into()),
        });
        b.handle_list_panes_response(
            window,
            vec!["1: [40x24] %1 (active)".into(), "2: [39x24] %2".into()],
        );
        let tree = &b.layouts[&TabId(0)].tree;
        assert!(
            matches!(tree, LayoutNode::Split { .. }),
            "unzoom 后应恢复 split，实际 {tree:?}"
        );
    }

    #[test]
    fn incomplete_pane_snapshot_does_not_collapse_layout() {
        let mut b = TmuxRuntime::new(None);
        let window = TabId(0);
        let layout = "1268,140x30,0,0{70x30,0,0,0,69x30,71,0[69x15,71,0,1,69x14,71,16,2]}";
        b.handle_message(Message::LayoutChange {
            window,
            layout: crate::core::runtime::tmux::protocol::LayoutChange::parse(layout).unwrap(),
            visible_layout: None,
            flags: None,
        });

        b.handle_list_panes_response(
            window,
            vec!["0: [70x30] %0 (active)".into(), "1: [69x15] %1".into()],
        );
        assert!(!b.layouts.contains_key(&TabId(0)));
        assert!(b.panes.is_empty());
    }

    #[test]
    fn pending_layout_events_are_coalesced_per_tab() {
        let mut b = TmuxRuntime::new(None);
        let window = TabId(0);
        let layout = "1268,140x30,0,0{70x30,0,0,0,69x30,71,0[69x15,71,0,1,69x14,71,16,2]}";
        let message = || Message::LayoutChange {
            window,
            layout: crate::core::runtime::tmux::protocol::LayoutChange::parse(layout).unwrap(),
            visible_layout: None,
            flags: None,
        };
        b.handle_message(message());
        b.handle_list_panes_response(
            window,
            vec![
                "0: [70x30] %0 (active)".into(),
                "1: [69x15] %1".into(),
                "2: [69x14] %2".into(),
            ],
        );
        b.handle_message(message());
        b.handle_list_panes_response(
            window,
            vec![
                "0: [70x30] %0 (active)".into(),
                "1: [69x15] %1".into(),
                "2: [69x14] %2".into(),
            ],
        );
        let count = b
            .events
            .iter()
            .filter(
                |event| matches!(event, StateChange::LayoutChanged { tab, .. } if *tab == TabId(0)),
            )
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn attach_seed_capture_must_request_scrollback_history() {
        let src = include_str!("backend.rs");
        let call = concat!("cmd", "::", "capture_pane_with_history");
        assert!(
            src.contains(call),
            "query_capture_pane 必须调用 cmd::capture_pane_with_history"
        );
        let old = concat!("format!(", r#""capture-pane -e -p -t %{}"#);
        assert!(
            !src.contains(old),
            "禁止 attach 播种仍 format 可见屏-only 的 capture-pane（缺 -S）"
        );
    }

    #[test]
    fn attach_capture_is_limited_to_active_tab_until_switch() {
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        b.cmd_tx = Some(tx);
        b.tabs = vec![
            TabInfo {
                id: TabId(1),
                name: "active".into(),
                active: true,
            },
            TabInfo {
                id: TabId(2),
                name: "background".into(),
                active: false,
            },
        ];
        b.handle_list_panes_response(TabId(1), vec!["0: [80x24] %0 (active)".into()]);
        b.handle_list_panes_response(TabId(2), vec!["1: [80x24] %1 (active)".into()]);

        assert!(b.initial_capture_pending.contains(&PaneId(0)));
        assert!(!b.initial_capture_pending.contains(&PaneId(1)));

        b.mark_tab_active(TabId(2));
        b.query_capture_tab(TabId(2));
        assert!(b.initial_capture_pending.contains(&PaneId(1)));
    }

    #[test]
    fn client_size_overrides_default_tmux_grid() {
        let mut b = TmuxRuntime::new(None);
        assert_eq!(b.config.cols, Some(80));
        assert_eq!(b.config.rows, Some(24));
        b.set_client_size(137, 41);
        assert_eq!(b.config.cols, Some(137));
        assert_eq!(b.config.rows, Some(41));
    }

    #[test]
    fn capture_pane_response_restores_existing_screen_without_double_feed() {
        let mut b = TmuxRuntime::new(None);
        let pane = PaneId(7);
        b.pending_by_number
            .insert(1, PendingQuery::CapturePane { pane });

        b.dispatch_response(1, vec!["\u{1b}[32mrestored shell".into(), "prompt$".into()]);
        assert_eq!(
            b.outputs.get(&pane).unwrap(),
            b"\x1b[32mrestored shell\r\nprompt$"
        );
        assert!(b.events.iter().any(|event| matches!(
            event,
            StateChange::PaneOutput { pane: event_pane, data }
                if *event_pane == pane && data.starts_with(b"\x1b[32mrestored")
        )));

        // 若 tmux 已经主动推送了 %output，capture 快照不能再次追加。
        b.pending_by_number
            .insert(2, PendingQuery::CapturePane { pane });
        b.dispatch_response(2, vec!["duplicate".into()]);
        assert_eq!(
            b.outputs.get(&pane).unwrap(),
            b"\x1b[32mrestored shell\r\nprompt$"
        );
    }

    #[test]
    fn capture_response_boundary_drops_prequeued_bytes_but_keeps_repeated_live_tail() {
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        let pane = PaneId(8);
        b.initial_capture_pending.insert(pane);
        // 请求响应开始前已经排队的通知可能与 snapshot 完全相同，不能
        // 再用内容子串猜测；它们由 response boundary 明确归类为 stale。
        b.initial_capture_buf.insert(pane, b"same\r\n".to_vec());
        // begin 之后的合法 live 输出即使恰好重复 snapshot，也必须保留。
        b.initial_capture_tail.insert(pane, b"same\r\n".to_vec());
        b.capture_response_seen.insert(pane);
        b.pending_by_number
            .insert(1, PendingQuery::CapturePane { pane });

        b.dispatch_response(1, vec!["same".into()]);

        assert_eq!(
            b.outputs.get(&pane).unwrap(),
            b"samesame\r\n",
            "边界之后的重复文本也是合法 live，不能按子串误删"
        );
    }

    #[test]
    fn attach_initial_output_waits_for_full_capture_snapshot() {
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        let pane = PaneId(3);

        // attach 初始流里的 prompt 不是完整屏幕，不能先暴露给 GUI。
        b.handle_message(Message::Output {
            pane,
            content: b"prompt$ ".to_vec(),
            raw_content: "prompt$ ".into(),
        });
        assert!(!b.outputs.contains_key(&pane));
        assert!(b.events.is_empty());

        b.pending_by_number
            .insert(1, PendingQuery::CapturePane { pane });
        b.dispatch_response(1, vec!["old command".into(), "prompt$ ".into()]);
        assert_eq!(b.outputs.get(&pane).unwrap(), b"old command\r\nprompt$ ");

        // 快照完成后，后续输出恢复为普通增量。
        b.handle_message(Message::Output {
            pane,
            content: b"live\r\n".to_vec(),
            raw_content: "live\\r\\n".into(),
        });
        assert!(b.outputs.get(&pane).unwrap().ends_with(b"live\r\n"));
    }

    #[test]
    fn capture_pane_strips_trailing_blank_rows_so_cursor_stays_at_prompt() {
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        let pane = PaneId(9);
        b.pending_by_number
            .insert(1, PendingQuery::CapturePane { pane });

        // tmux 屏幕：prompt 在第 0..2 行，下方全是空白行；若把空白行也
        // 喂给新终端，光标会被推到最底部。
        b.dispatch_response(
            1,
            vec![
                "~/Developer/muxterm".into(),
                "feature/quickconnect".into(),
                "❯".into(),
                "".into(),
                "".into(),
                " ".into(),
            ],
        );
        assert_eq!(
            b.outputs.get(&pane).unwrap(),
            b"~/Developer/muxterm\r\nfeature/quickconnect\r\n\xE2\x9D\xAF"
        );
    }

    #[test]
    fn command_response_placeholder_does_not_consume_capture_query() {
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        let pane = PaneId(4);
        b.pending_by_number.insert(1, PendingQuery::Ignore);
        b.pending_by_number
            .insert(2, PendingQuery::CapturePane { pane });

        // split/send-keys 等普通命令的响应先到，不能把 capture 查询错配掉。
        b.dispatch_response(1, vec!["ignored".into()]);
        b.dispatch_response(2, vec!["restored".into()]);

        assert_eq!(b.outputs.get(&pane).unwrap(), b"restored");
    }

    #[test]
    fn attach_live_output_during_capture_is_appended_after_snapshot() {
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        let pane = PaneId(5);

        // 发起 capture 查询后，查询期间到达的实时输出先暂存，不直接暴露。
        b.initial_capture_pending.insert(pane);
        b.handle_message(Message::Output {
            pane,
            content: b"live-during-capture\r\n".to_vec(),
            raw_content: "live-during-capture\\r\\n".into(),
        });
        assert!(!b.outputs.contains_key(&pane));
        assert!(b.events.is_empty());

        // capture 快照返回：完整屏幕 + 查询期间的实时增量拼接。
        b.pending_by_number
            .insert(1, PendingQuery::CapturePane { pane });
        // 快照不补尾随 CRLF：查询期间到达的实时输出是光标位置的延续，
        // 直接拼接（若程序自己换行，其字节流里会自带 CRLF）。
        b.dispatch_response(1, vec!["screen line".into()]);
        assert_eq!(
            b.outputs.get(&pane).unwrap(),
            b"screen linelive-during-capture\r\n"
        );

        // 之后 %output 恢复为普通增量。
        b.handle_message(Message::Output {
            pane,
            content: b"after-capture\r\n".to_vec(),
            raw_content: "after-capture\\r\\n".into(),
        });
        assert!(b
            .outputs
            .get(&pane)
            .unwrap()
            .ends_with(b"after-capture\r\n"));
    }

    /// F3：capture 完成前 live 不得进 VTE（事件层）——快照前无 PaneOutput 事件；
    /// 快照返回后单事件 = 快照 + catch-up；capture 每 pane 只发一次。
    #[test]
    fn surface_seed_drops_output_until_capture() {
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        let pane = PaneId(11);

        // 发起 capture 后、快照返回前：live 只暂存，不产生 PaneOutput 事件。
        b.initial_capture_pending.insert(pane);
        b.handle_message(Message::Output {
            pane,
            content: b"PRE_SEED_TOKEN".to_vec(),
            raw_content: "PRE_SEED_TOKEN".into(),
        });
        assert!(!b.outputs.contains_key(&pane));
        assert!(b.events.is_empty(), "capture 前 live 不得交付");

        // 快照返回：单事件 = 快照 + catch-up。
        b.pending_by_number
            .insert(1, PendingQuery::CapturePane { pane });
        b.dispatch_response(1, vec!["SNAPSHOT_TOKEN".into()]);
        let events: Vec<&StateChange> = b
            .events
            .iter()
            .filter(|e| matches!(e, StateChange::PaneSnapshot { pane: p, .. } if *p == pane))
            .collect();
        assert_eq!(events.len(), 1, "快照+catch-up 应合并为一次替换事件");
        let StateChange::PaneSnapshot { data, .. } = events[0] else {
            unreachable!()
        };
        assert!(data.starts_with(b"SNAPSHOT_TOKEN"));
        assert!(data.ends_with(b"PRE_SEED_TOKEN"));

        // capture 每 pane 只发一次：已 done 的 pane 不再重复查询。
        b.query_capture_pane(pane);
        let captures = b
            .pending_queries
            .iter()
            .filter(|q| matches!(q, PendingQuery::CapturePane { pane: p } if *p == pane))
            .count();
        assert_eq!(captures, 0, "已 done 的 pane 不应重复 capture");
    }

    #[test]
    fn attach_capture_failure_recovers_live_output_without_black_screen() {
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        let pane = PaneId(6);

        b.initial_capture_pending.insert(pane);
        b.initial_capture_buf
            .insert(pane, b"live-during-failed-capture\r\n".to_vec());
        // %error 而不是 %end：capture 失败。
        b.pending_by_number
            .insert(1, PendingQuery::CapturePane { pane });
        b.handle_response_error(1);

        // 失败后不能永久抑制 pane 输出：后续实时输出必须照常渲染。
        assert!(b.initial_capture_done.contains(&pane));
        assert!(b
            .outputs
            .get(&pane)
            .is_some_and(|data| data.starts_with(b"live-during-failed-capture\r\n")));
        b.handle_message(Message::Output {
            pane,
            content: b"live-after-error\r\n".to_vec(),
            raw_content: "live-after-error\\r\\n".into(),
        });
        assert!(b
            .outputs
            .get(&pane)
            .unwrap()
            .ends_with(b"live-after-error\r\n"));
        assert!(b.events.iter().any(|event| matches!(
            event,
            StateChange::PaneOutput { pane: ep, data }
                if *ep == pane && data.ends_with(b"live-after-error\r\n")
        )));
    }

    #[test]
    fn response_number_matching_does_not_misassign_interleaved_queries() {
        // 高输出下多个 %begin/%end 交叠时，必须按 number 精确匹配，而不是 FIFO。
        let mut b = TmuxRuntime::new(None);
        let p1 = PaneId(10);
        let p2 = PaneId(11);

        // begin 1（CapturePane p1）、begin 2（CapturePane p2）
        b.pending_by_number
            .insert(1, PendingQuery::CapturePane { pane: p1 });
        b.pending_by_number
            .insert(2, PendingQuery::CapturePane { pane: p2 });

        // 响应乱序返回：end 2 先到，end 1 后到。
        b.dispatch_response(2, vec!["second screen".into()]);
        b.dispatch_response(1, vec!["first screen".into()]);

        assert_eq!(b.outputs.get(&p2).unwrap(), b"second screen");
        assert_eq!(b.outputs.get(&p1).unwrap(), b"first screen");
    }

    #[test]
    fn parse_tmux_version_handles_beta_suffix() {
        assert_eq!(parse_tmux_version("tmux 3.7b"), Some((3, 7)));
        assert_eq!(parse_tmux_version("tmux 2.9a"), Some((2, 9)));
        assert_eq!(parse_tmux_version("garbage"), None);
    }

    #[test]
    fn pane_replay_snapshot_restores_modes_and_alternate_screen() {
        let state = parse_pane_replay_state("3|4|0|underline|1|1|7|8|1|0|1|1|1|0|1|1|1|0|1|1");
        let bytes = build_pane_snapshot(
            Some(&state),
            b"primary\r\n",
            b"cursor frame",
            b"\x1b[2Jlive",
        );
        assert!(bytes.starts_with(b"primary\r\n\x1b[9;8H\x1b[?1049hcursor frame"));
        assert!(bytes.windows(b"\x1b[5;4H".len()).any(|w| w == b"\x1b[5;4H"));
        assert!(bytes.windows(b"\x1b[3 q".len()).any(|w| w == b"\x1b[3 q"));
        assert!(bytes.windows(b"\x1b[?25l".len()).any(|w| w == b"\x1b[?25l"));
        assert!(bytes.ends_with(b"\x1b[2Jlive"));
    }

    #[test]
    fn pane_replay_snapshot_keeps_primary_when_alternate_is_empty() {
        let state = parse_pane_replay_state("0|0|1|||0|||||||||||||||");
        let bytes = build_pane_snapshot(Some(&state), b"prompt$ ", b"", b"next");
        assert!(bytes.starts_with(b"prompt$ "));
        assert!(bytes.windows(b"\x1b[1;1H".len()).any(|w| w == b"\x1b[1;1H"));
        assert!(bytes.windows(b"\x1b[?25h".len()).any(|w| w == b"\x1b[?25h"));
        assert!(bytes
            .windows(b"\x1b[?2004l".len())
            .any(|w| w == b"\x1b[?2004l"));
        assert!(bytes.ends_with(b"next"));
    }

    #[test]
    fn pane_resync_emits_one_snapshot_and_replays_catch_up_bytes() {
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new(None);
        b.cmd_tx = Some(tx);
        let pane = PaneId(21);
        b.events.push_back(StateChange::PaneOutput {
            pane,
            data: b"stale-frame".to_vec(),
        });
        b.begin_pane_resync(pane, "test");
        b.handle_message(Message::Output {
            pane,
            content: b"live-after-pause".to_vec(),
            raw_content: String::new(),
        });
        b.pending_by_number
            .insert(1, PendingQuery::PaneResyncState { pane });
        b.dispatch_response(
            1,
            vec!["2|1|1|block|0|1|0|0|0|1|0|0|0|0|0|0|0|0|0|1".into()],
        );
        b.pending_by_number.insert(
            2,
            PendingQuery::PaneResyncCapture {
                pane,
                alternate: false,
            },
        );
        b.dispatch_response(2, vec!["primary".into()]);
        b.pending_by_number.insert(
            3,
            PendingQuery::PaneResyncCapture {
                pane,
                alternate: true,
            },
        );
        b.dispatch_response(3, vec!["alternate".into()]);

        assert!(!b.resyncs.contains_key(&pane));
        assert!(b.events.iter().all(|event| {
            !matches!(event, StateChange::PaneOutput { pane: p, .. } if *p == pane)
        }));
        let snapshots: Vec<&[u8]> = b
            .events
            .iter()
            .filter_map(|event| match event {
                StateChange::PaneSnapshot { pane: p, data } if *p == pane => Some(data.as_slice()),
                _ => None,
            })
            .collect();
        assert_eq!(snapshots.len(), 1);
        assert!(snapshots[0]
            .windows(b"primary".len())
            .any(|w| w == b"primary"));
        assert!(snapshots[0]
            .windows(b"alternate".len())
            .any(|w| w == b"alternate"));
        assert!(snapshots[0]
            .windows(b"live-after-pause".len())
            .any(|w| w == b"live-after-pause"));
        let primary_capture_at = snapshots[0]
            .windows(b"alternate".len())
            .position(|w| w == b"alternate")
            .expect("primary capture should be present");
        let alternate_capture_at = snapshots[0]
            .windows(b"primary".len())
            .position(|w| w == b"primary")
            .expect("alternate capture should be present");
        assert!(
            primary_capture_at < alternate_capture_at,
            "primary must be seeded before alternate"
        );
    }

    #[test]
    fn pane_resync_emits_empty_snapshot_to_clear_blank_screen() {
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new(None);
        b.cmd_tx = Some(tx);
        let pane = PaneId(22);
        b.outputs.insert(pane, b"stale text".to_vec());
        b.resyncs.insert(pane, PaneResync::default());

        b.finish_pane_resync(pane);

        assert!(b.outputs.get(&pane).is_some_and(Vec::is_empty));
        assert!(b.events.iter().any(|event| {
            matches!(
                event,
                StateChange::PaneSnapshot { pane: id, data }
                    if *id == pane && data.is_empty()
            )
        }));
    }

    #[test]
    fn colour_report_requires_tmux_3_2() {
        assert!(!supports_colour_report(Some((3, 1))));
        assert!(supports_colour_report(Some((3, 2))));
        assert!(supports_colour_report(Some((4, 0))));
        assert!(supports_colour_report(None));
    }

    fn unique_socket() -> String {
        format!("muxterm-tb-{}-{}", std::process::id(), rand_suffix())
    }

    fn rand_suffix() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        format!("{nanos}")
    }

    fn cleanup(socket: &str) {
        let _ = std::process::Command::new("tmux")
            .args(["-L", socket, "kill-server"])
            .output();
    }

    /// 整个用例上限，防止 connect/shutdown/pty 写卡住拖死 CI（曾 15min 挂起）。
    const TMUX_TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn connect_establishes_session_and_window() {
        let socket = unique_socket();
        let run = async {
            let mut b = TmuxRuntime::new(Some(&socket));
            b.connect().await.unwrap_or_else(|e| {
                eprintln!("skip: tmux 不可用: {e}");
            });
            if b.status() != BackendStatus::Connected {
                return;
            }
            assert_eq!(b.status(), BackendStatus::Connected);
            let events = b.take_events();
            assert!(events.iter().any(|e| matches!(
                e,
                StateChange::BackendStatusChanged(BackendStatus::Connected)
            )));
            assert!(b.active_session.is_some());
            assert!(!b.tabs.is_empty());
            let _ = b.shutdown().await;
        };
        let timed = tokio::time::timeout(TMUX_TEST_TIMEOUT, run).await;
        cleanup(&socket);
        if timed.is_err() {
            panic!("connect_establishes_session_and_window 超时");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn new_window_via_tmux() {
        let socket = unique_socket();
        let run = async {
            let mut b = TmuxRuntime::new(Some(&socket));
            if b.connect().await.is_err() {
                return;
            }
            let _ = b.take_events();
            let initial_tabs = b.tabs.len();
            b.execute(&Task::NewTab {
                name: Some("test-win".into()),
                command: None,
                workdir: None,
            })
            .unwrap();
            let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(3);
            loop {
                let _ = b.take_events();
                if b.tabs.len() > initial_tabs {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
            assert!(
                b.tabs.len() > initial_tabs,
                "新 tab（tmux window）未建立: tabs={}",
                b.tabs.len()
            );
            assert_eq!(b.tabs.len(), initial_tabs + 1, "NewTab 应新增 1 个 tab");
            let _ = b.shutdown().await;
        };
        let timed = tokio::time::timeout(TMUX_TEST_TIMEOUT, run).await;
        cleanup(&socket);
        if timed.is_err() {
            panic!("new_window_via_tmux 超时");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn send_keys_does_not_error() {
        let socket = unique_socket();
        let run = async {
            let mut b = TmuxRuntime::new(Some(&socket));
            if b.connect().await.is_err() {
                return;
            }
            let _ = b.take_events();
            let pane = b.active_pane().map(|p| p.id).unwrap_or(PaneId(0));
            let outcome = b
                .execute(&Task::SendKeys {
                    target: pane,
                    keys: vec![crate::core::protocol::terminal::input::KeyEvent::Char('x')],
                })
                .unwrap();
            assert_eq!(outcome, TaskOutcome::Done);
            let _ = b.shutdown().await;
        };
        let timed = tokio::time::timeout(TMUX_TEST_TIMEOUT, run).await;
        cleanup(&socket);
        if timed.is_err() {
            panic!("send_keys_does_not_error 超时（tmux socket/shutdown 挂起）");
        }
    }

    /// 回归：`%output` 事件拼接后必须与 pane 的真实字节流完全一致。
    /// cursor agent 的「擦除 + 上移 + 原地重绘」帧之间若被协议/解析层插入
    /// 多余换行，SwiftTerm/TerminalState 会把每帧画到下一行，造成输入框和
    /// 状态区逐帧堆叠。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pane_output_events_preserve_exact_frame_bytes() {
        let socket = unique_socket();
        let run = async {
            let mut b = TmuxRuntime::new(Some(&socket));
            if b.connect().await.is_err() {
                return;
            }
            let _ = b.take_events();
            let pane = b.active_pane().map(|p| p.id).unwrap_or(PaneId(0));

            // 在 pane 里用 python 输出两帧：与 cursor agent 相同的
            // 「擦除 + 上移 + 重绘」模式，帧间仅 sleep（无任何换行）。
            let script = "python3 -c 'import sys,time; f=\"\\x1b[2K\\x1b[1A\\x1b[2K\\x1b[1A\\x1b[GSTATUS-A\\r\\nTIP\\r\\n\\r\\nBOX\\r\\n\\r\\nFOOTER-A\\r\\n\"; sys.stdout.write(f); time.sleep(0.3); sys.stdout.write(f.replace(\"STATUS-A\",\"STATUS-B\").replace(\"FOOTER-A\",\"FOOTER-B\"))'\n";
            let _ = std::process::Command::new("tmux")
                .args([
                    "-L",
                    &socket,
                    "send-keys",
                    "-t",
                    &format!("%{}", pane.0),
                    script,
                ])
                .status();

            let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(6);
            let mut collected: Vec<u8> = Vec::new();
            loop {
                for ev in b.take_events() {
                    if let StateChange::PaneOutput { data, .. } = ev {
                        collected.extend_from_slice(&data);
                    }
                }
                if collected.windows(9).any(|w| w == b"FOOTER-B") {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            }

            assert!(
                collected
                    .windows(b"FOOTER-B".len())
                    .any(|w| w == b"FOOTER-B"),
                "第二帧必须到达: {:?}",
                String::from_utf8_lossy(&collected)
            );
            // 关键：两帧之间不得出现「CRLF + 空行」——即 FOOTER-A 之后到下一帧
            // ESC 之间不能有连续两个 LF（本地 shell 的 ONLCR 可能把 \n 变 \r\n，
            // 但协议/解析层绝不能额外插入换行）。
            let marker = collected
                .windows(b"FOOTER-A".len())
                .position(|w| w == b"FOOTER-A")
                .expect("第一帧必须到达");
            let after = &collected[marker + b"FOOTER-A".len()..];
            let next_esc = after.iter().position(|&b| b == 0x1b).unwrap_or(after.len());
            let between = &after[..next_esc];
            let lf_count = between.iter().filter(|&&b| b == b'\n').count();
            assert!(
                lf_count <= 1,
                "帧间不得出现多余换行（实际 {lf_count} 个 LF）: {:?}",
                String::from_utf8_lossy(between)
            );
            let _ = b.shutdown().await;
        };
        let timed = tokio::time::timeout(TMUX_TEST_TIMEOUT, run).await;
        cleanup(&socket);
        if timed.is_err() {
            panic!("pane_output_events_preserve_exact_frame_bytes 超时");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn split_pane_dispatched() {
        let socket = unique_socket();
        let run = async {
            let mut b = TmuxRuntime::new(Some(&socket));
            if b.connect().await.is_err() {
                return;
            }
            let _ = b.take_events();
            let pane = b.active_pane().map(|p| p.id).unwrap_or(PaneId(0));
            let outcome = b
                .execute(&Task::SplitPane {
                    target: Some(pane),
                    dir: SplitDir::Horizontal,
                    command: None,
                    workdir: None,
                })
                .unwrap();
            assert_eq!(outcome, TaskOutcome::Done);
            let _ = b.shutdown().await;
        };
        let timed = tokio::time::timeout(TMUX_TEST_TIMEOUT, run).await;
        cleanup(&socket);
        if timed.is_err() {
            panic!("split_pane_dispatched 超时");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn toggle_pane_fullscreen_dispatches_zoom() {
        let socket = unique_socket();
        let run = async {
            let mut b = TmuxRuntime::new(Some(&socket));
            if b.connect().await.is_err() {
                return;
            }
            let _ = b.take_events();
            let pane = b.active_pane().map(|p| p.id).unwrap_or(PaneId(0));
            let outcome = b
                .execute(&Task::TogglePaneFullscreen { target: pane })
                .unwrap();
            assert_eq!(outcome, TaskOutcome::Done);
            let _ = b.shutdown().await;
        };
        let timed = tokio::time::timeout(TMUX_TEST_TIMEOUT, run).await;
        cleanup(&socket);
        assert!(timed.is_ok(), "toggle fullscreen 应派发成功");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn execute_before_connect_rejected() {
        let mut b = TmuxRuntime::new(Some("muxterm-nosuch-socket-xyz"));
        let outcome = b
            .execute(&Task::SendKeys {
                target: PaneId(1),
                keys: vec![],
            })
            .unwrap();
        assert!(matches!(outcome, TaskOutcome::Rejected { .. }));
    }

    /// 回归：大量 %output 不得把 outputs/events 撑到数 GB（曾观测挂起时 ~20GB）。
    #[test]
    fn pane_output_accumulation_is_capped() {
        use crate::core::buffer_cap::MAX_PANE_OUTPUT_BYTES;
        use crate::core::runtime::tmux::protocol::Message;

        let mut b = TmuxRuntime::new(None);
        let pane = PaneId(42);
        let chunk = vec![b'x'; 64 * 1024];
        // 灌入远超上限的数据
        for _ in 0..80 {
            b.handle_message(Message::Output {
                pane,
                content: chunk.clone(),
                raw_content: String::new(),
            });
        }
        let stored = b.outputs.get(&pane).map(|v| v.len()).unwrap_or(0);
        assert!(
            stored <= MAX_PANE_OUTPUT_BYTES,
            "outputs 应有界，实际 {stored} > {MAX_PANE_OUTPUT_BYTES}"
        );
        assert!(
            b.events.len() <= crate::core::buffer_cap::MAX_STATE_EVENTS,
            "events 应有界，实际 {}",
            b.events.len()
        );
    }

    /// 回归：输出洪峰下结构性事件（ActiveTabChanged 等）绝不能被 trim 丢弃，
    /// 否则前端永远等不到切 tab 确认而卡死。
    #[test]
    fn output_flood_does_not_drop_structural_events() {
        use crate::core::buffer_cap::MAX_PANE_OUTPUT_BYTES;
        use crate::core::model::state::StateChange;
        use crate::core::runtime::tmux::protocol::Message;

        let mut b = TmuxRuntime::new(None);
        let pane = PaneId(1);
        // 先放一个 ActiveTabChanged（切 tab 的确认事件）
        b.events.push_back(StateChange::ActiveTabChanged {
            tab: crate::core::types::TabId(14),
        });
        // 灌入远超上限的 PaneOutput
        let chunk = vec![b'x'; 64 * 1024];
        for _ in 0..200 {
            b.handle_message(Message::Output {
                pane,
                content: chunk.clone(),
                raw_content: String::new(),
            });
        }
        // ActiveTabChanged 必须仍在队列里（前端靠它放行切 tab）
        assert!(
            b.events.iter().any(|e| matches!(
                e,
                StateChange::ActiveTabChanged { tab: t, .. } if t.0 == 14
            )),
            "输出洪峰后 ActiveTabChanged 不得被丢弃"
        );
        let _ = MAX_PANE_OUTPUT_BYTES; // 引用以保持编译
    }

    /// 流控：%pause / %continue 被安全忽略，不阻塞后续 %output 累积与状态机。
    #[test]
    fn flow_control_pause_continue_tracks_pane() {
        use crate::core::runtime::tmux::protocol::Message;

        let mut b = TmuxRuntime::new(None);
        let pane = PaneId(7);

        // 在 %output 之间穿插 %pause / %continue，验证不破坏输出累积，
        // 且暂停状态被跟踪（%pause %N → paused；%continue %N → 恢复）。
        b.handle_message(Message::Output {
            pane,
            content: b"a".to_vec(),
            raw_content: String::new(),
        });
        b.handle_message(Message::Pause {
            pane: Some(pane),
            args: String::new(),
        });
        assert!(b.paused_panes.contains(&pane), "%pause 后应标记暂停");
        b.handle_message(Message::Output {
            pane,
            content: b"b".to_vec(),
            raw_content: String::new(),
        });
        b.handle_message(Message::Continue {
            pane: Some(pane),
            args: String::new(),
        });
        assert!(!b.paused_panes.contains(&pane), "%continue 后应恢复");
        b.handle_message(Message::Output {
            pane,
            content: b"c".to_vec(),
            raw_content: String::new(),
        });

        // 三条 output 都应累积，未被 pause/continue 截断或丢弃
        let out = b.outputs.get(&pane).cloned().unwrap_or_default();
        assert_eq!(out, b"abc", "pause/continue 不应破坏 %output 累积");
        // 事件队列里应有对应数量的 PaneOutput
        let out_events = b
            .events
            .iter()
            .filter(|e| matches!(e, crate::core::model::state::StateChange::PaneOutput { .. }))
            .count();
        assert_eq!(out_events, 3, "应有 3 个 PaneOutput 事件");
    }

    /// %window-pane-changed：切换某 window 的 active pane，应触发 ActivePaneChanged。
    #[test]
    fn window_pane_changed_updates_active_pane() {
        use crate::core::model::state::StateChange;
        use crate::core::runtime::tmux::protocol::Message;

        let mut b = TmuxRuntime::new(None);
        // 预置一个 window + 两个 pane 在同一 tab
        let win = crate::core::types::TabId(0);
        let tab = crate::core::types::TabId(0);
        b.tabs.push(crate::core::model::state::TabInfo {
            id: tab,
            name: "t0".into(),
            active: true,
        });
        b.panes.push(crate::core::model::state::PaneInfo {
            id: crate::core::types::PaneId(1),
            tab,
            cols: 40,
            rows: 24,
            active: true,
            title: "p1".into(),
        });
        b.panes.push(crate::core::model::state::PaneInfo {
            id: crate::core::types::PaneId(2),
            tab,
            cols: 40,
            rows: 24,
            active: false,
            title: "p2".into(),
        });

        b.handle_message(Message::WindowPaneChanged {
            window: win,
            pane: crate::core::types::PaneId(2),
        });

        // pane 2 应变为 active
        let p2 = b
            .panes
            .iter()
            .find(|p| p.id == crate::core::types::PaneId(2))
            .unwrap();
        assert!(p2.active, "window-pane-changed 后 pane2 应 active");
        let p1 = b
            .panes
            .iter()
            .find(|p| p.id == crate::core::types::PaneId(1))
            .unwrap();
        assert!(!p1.active, "pane1 应不再 active");
        // 应有 ActivePaneChanged 事件
        assert!(
            b.events.iter().any(|e| matches!(e, StateChange::ActivePaneChanged { pane, .. } if *pane == crate::core::types::PaneId(2))),
            "应有 ActivePaneChanged(pane2)"
        );
    }

    /// %window-close：除 TabClosed 外，还必须为每个 pane 发 PaneClosed，
    /// 前端才能回收保留的终端视图（macOS SwiftTerm 视图只在 PaneClosed 时移除）。
    #[test]
    fn window_close_emits_pane_closed_for_each_pane() {
        use crate::core::model::state::StateChange;
        use crate::core::runtime::tmux::protocol::Message;

        let mut b = TmuxRuntime::new(None);
        let win = crate::core::types::TabId(2);
        let tab = crate::core::types::TabId(2);
        b.tabs.push(crate::core::model::state::TabInfo {
            id: tab,
            name: "t2".into(),
            active: false,
        });
        for id in [5u32, 6] {
            b.panes.push(crate::core::model::state::PaneInfo {
                id: crate::core::types::PaneId(id),
                tab,
                cols: 40,
                rows: 24,
                active: false,
                title: format!("p{id}"),
            });
        }

        b.handle_message(Message::WindowClose { window: win });

        assert!(
            b.events
                .iter()
                .any(|e| matches!(e, StateChange::TabClosed { tab: t } if *t == tab)),
            "应有 TabClosed"
        );
        for id in [5u32, 6] {
            assert!(
                b.events
                    .iter()
                    .any(|e| matches!(e, StateChange::PaneClosed { pane: p } if p.0 == id)),
                "应有 PaneClosed(pane {id})"
            );
        }
        assert!(
            b.panes.iter().all(|p| p.tab != tab),
            "window 关闭后 pane 应全部移除"
        );
    }

    /// move-window 竞态回归：权威 list-windows 已确认窗口存在后，迟到的
    /// `%window-close` 不得把 tab 永久删掉；挂起关闭必须等下一次权威响应
    /// 裁决，确认仍存在时取消关闭（tmux unlink→link 的 add+close 组合）。
    #[test]
    fn late_window_close_after_authoritative_list_keeps_tab() {
        use crate::core::model::state::StateChange;
        use crate::core::runtime::tmux::protocol::{Message, TmuxSessionId};
        use crate::core::types::TabId;

        let mut b = TmuxRuntime::new(None);
        b.active_session = Some(TmuxSessionId(0));
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        b.cmd_tx = Some(tx);
        b.status = BackendStatus::Connected;

        // 权威列表先确认两个 window（@0/@1），其中 @1 是 move-window 的目标。
        b.handle_list_windows_response(vec![
            "@0,first,1,aaaa,80x24,0,0,1,0".into(),
            "@1,second,0,bbbb,80x24,0,0,1,0".into(),
        ]);
        b.panes.push(crate::core::model::state::PaneInfo {
            id: crate::core::types::PaneId(7),
            tab: TabId(1),
            cols: 40,
            rows: 24,
            active: false,
            title: "p7".into(),
        });

        // 迟到的 close：只挂起，并发出权威查询。
        b.handle_message(Message::WindowClose { window: TabId(1) });
        assert!(
            b.tabs.iter().any(|t| t.id == TabId(1)),
            "close 通知到达但权威查询未返回前不得删除 tab"
        );
        assert!(
            b.panes
                .iter()
                .any(|p| p.id == crate::core::types::PaneId(7)),
            "close 挂起期间 pane 不得提前移除"
        );
        assert!(
            b.pending_close_tabs.contains(&TabId(1)),
            "close 应挂起待裁决"
        );
        assert!(
            rx.try_recv().is_ok(),
            "close 应立即发起权威 list-windows 查询"
        );

        // 下一次权威响应仍包含 @1：move-window 已重新 link，取消关闭。
        b.handle_list_windows_response(vec![
            "@0,first,0,aaaa,80x24,0,0,1,0".into(),
            "@1,second,1,bbbb,80x24,0,0,1,0".into(),
        ]);

        assert!(
            b.tabs.iter().any(|t| t.id == TabId(1)),
            "权威确认后 tab 必须保留"
        );
        assert!(
            b.panes
                .iter()
                .any(|p| p.id == crate::core::types::PaneId(7)),
            "权威确认后 pane 必须保留"
        );
        assert!(
            !b.events
                .iter()
                .any(|e| matches!(e, StateChange::TabClosed { tab: t } if *t == TabId(1))),
            "move-window 的迟到 close 不得产生 TabClosed"
        );
        assert!(
            !b.pending_close_tabs.contains(&TabId(1)),
            "裁决后不应残留挂起关闭"
        );
    }

    /// W3：attach 假布局「tmux 2 window / 4 pane」→ 2 个 Tab、pane 挂在对应 tab，
    /// 产品层无 Window/Session 概念（编译期由 types 移除保证）。
    #[test]
    fn attach_snapshot_maps_windows_to_tabs_without_product_window() {
        let mut b = TmuxRuntime::new(None);
        b.workspace_name = "demo".into();
        b.active_session = Some(crate::core::runtime::tmux::protocol::TmuxSessionId(0));

        // list-windows 响应：2 个 tmux window（@0 1 pane，@1 3 panes）
        b.handle_list_windows_response(vec![
            "@0,main,1,bbcd,80x24,0,0,1,1,0".into(),
            "@1,code,0,d67e,80x24,0,0{40x24,0,0,0,39x24,41,0[39x12,41,0,1,39x11,41,13,2]},3,0"
                .into(),
        ]);
        assert_eq!(b.tabs.len(), 2, "2 个 tmux window → 2 个 Tab");
        assert!(b.tabs.iter().any(|t| t.id == TabId(0) && t.name == "main"));
        assert!(b.tabs.iter().any(|t| t.id == TabId(1) && t.name == "code"));

        // list-panes 响应：@0 有 1 个 pane，@1 有 3 个 pane
        b.handle_list_panes_response(
            TabId(0),
            vec!["0: [80x24] [history 0/2000, 0/2000 bytes] %0 (active)".into()],
        );
        b.handle_list_panes_response(
            TabId(1),
            vec![
                "1: [80x24] [history 0/2000, 0/2000 bytes] %1 (active)".into(),
                "2: [80x24] [history 0/2000, 0/2000 bytes] %2".into(),
                "3: [80x24] [history 0/2000, 0/2000 bytes] %3".into(),
            ],
        );
        assert_eq!(b.panes.len(), 4, "4 个 pane");
        assert!(b
            .panes
            .iter()
            .all(|p| p.tab == TabId(0) || p.tab == TabId(1)));
        assert_eq!(b.panes.iter().filter(|p| p.tab == TabId(0)).count(), 1);
        assert_eq!(b.panes.iter().filter(|p| p.tab == TabId(1)).count(), 3);
        // 产品层无 Window：State 只暴露 workspace/tab/pane
        assert_eq!(b.workspace_name(), "demo");
        assert_eq!(b.workspace_runtime(), "tmux");
    }

    #[test]
    fn list_windows_response_reorders_tabs_by_tmux_index() {
        let mut b = TmuxRuntime::new(None);
        b.handle_list_windows_response(vec![
            "@0,first,1,aaaa,80x24,0,0,1,0".into(),
            "@1,second,0,bbbb,80x24,0,0,1,0".into(),
        ]);
        assert_eq!(
            b.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
            vec![TabId(0), TabId(1),]
        );

        b.handle_list_windows_response(vec![
            "@1,second,1,bbbb,80x24,0,0,1,0".into(),
            "@0,first,0,aaaa,80x24,0,0,1,0".into(),
        ]);

        assert_eq!(
            b.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
            vec![TabId(1), TabId(0)],
            "Tab 顺序必须跟随 list-windows 返回的 tmux index 顺序"
        );
    }

    /// 回归：未指定 workdir 的 split 用同一条命令精确锁定 pane 并展开其 cwd，
    /// 避免两步查询期间焦点变化后 split 到其它 tab。
    #[test]
    fn split_inherits_target_pane_directory_atomically() {
        use crate::core::model::layout::SplitDir;
        use crate::core::model::task::Task;
        use tokio::sync::mpsc;

        let mut b = TmuxRuntime::new(None);
        // 预置 pane 所在 tab/window
        b.panes.push(crate::core::model::state::PaneInfo {
            id: crate::core::types::PaneId(3),
            tab: crate::core::types::TabId(7),
            cols: 80,
            rows: 24,
            active: true,
            title: "p3".into(),
        });
        b.tabs.push(crate::core::model::state::TabInfo {
            id: crate::core::types::TabId(7),
            name: "t7".into(),
            active: true,
        });
        // 建立命令通道，捕获后续 dispatch 的命令
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        b.cmd_tx = Some(tx);
        b.status = crate::core::model::state::BackendStatus::Connected;

        // execute SplitPane（workdir=None）→ 一条原子 split 命令。
        let outcome = b
            .execute(&Task::SplitPane {
                target: Some(crate::core::types::PaneId(3)),
                dir: SplitDir::Horizontal,
                command: None,
                workdir: None,
            })
            .unwrap();
        assert_eq!(outcome, crate::core::model::task::TaskOutcome::Done);
        let split = rx.try_recv().expect("应发送 split-window");
        assert_eq!(
            split, "split-window -t %3 -h -c \"#{pane_current_path}\"\n",
            "必须同时锁定目标 pane 与它的 cwd"
        );
        assert!(
            rx.try_recv().is_err(),
            "原子 split 不得再发送异步 display-message"
        );
    }

    /// %session-window-changed：切换 session 的 active window → active tab 切换。
    #[test]
    fn session_window_changed_updates_active_tab() {
        use crate::core::model::state::StateChange;
        use crate::core::runtime::tmux::protocol::Message;

        let mut b = TmuxRuntime::new(None);
        let session = crate::core::runtime::tmux::protocol::TmuxSessionId(0);
        b.workspace_name = "s0".into();
        b.active_session = Some(session);
        // 预置两个 tab（对应两个 tmux window @0 @1）
        for (id, active) in [(0u32, true), (1, false)] {
            b.tabs.push(crate::core::model::state::TabInfo {
                id: crate::core::types::TabId(id),
                name: format!("t{id}"),
                active,
            });
        }

        b.handle_message(Message::SessionWindowChanged {
            session,
            window: crate::core::types::TabId(1),
        });

        // tab1 应变为 active，tab0 不再 active
        let t1 = b
            .tabs
            .iter()
            .find(|t| t.id == crate::core::types::TabId(1))
            .unwrap();
        assert!(t1.active, "session-window-changed 后 tab1 应 active");
        let t0 = b
            .tabs
            .iter()
            .find(|t| t.id == crate::core::types::TabId(0))
            .unwrap();
        assert!(!t0.active, "tab0 应不再 active");
        // 应有 ActiveTabChanged 事件
        assert!(
            b.events.iter().any(|e| matches!(e, StateChange::ActiveTabChanged { tab, .. } if *tab == crate::core::types::TabId(1))),
            "应有 ActiveTabChanged(tab1)"
        );
    }

    /// 同一 tmux server 上其它 session 的 %session-window-changed 不得改 tab。
    /// 注意：判断「其它」以 %session-changed 落地后的 active_session 为准，
    /// 不要写死 yaklang-workspace=$0（2026-08-15 日志里它是 $4）。
    #[test]
    fn session_window_changed_ignores_other_session() {
        use crate::core::model::state::StateChange;
        use crate::core::runtime::tmux::protocol::Message;

        let mut b = TmuxRuntime::new(None);
        let attached = crate::core::runtime::tmux::protocol::TmuxSessionId(0);
        b.workspace_name = "yaklang-workspace".into();
        b.active_session = Some(attached);
        b.tabs.push(crate::core::model::state::TabInfo {
            id: crate::core::types::TabId(0),
            name: "Monitor".into(),
            active: true,
        });

        b.handle_message(Message::SessionWindowChanged {
            session: crate::core::runtime::tmux::protocol::TmuxSessionId(4),
            window: crate::core::types::TabId(20),
        });

        assert!(b.tabs[0].active, "其它 session 的通知不应取消当前 tab");
        assert!(
            !b.events
                .iter()
                .any(|e| matches!(e, StateChange::ActiveTabChanged { .. })),
            "不应为其它 session 发 ActiveTabChanged"
        );
        assert!(
            !b.tabs.iter().any(|t| t.id == crate::core::types::TabId(20)),
            "不应把其它 session 的 window 收成 tab"
        );
    }

    /// attach 到 yaklang-workspace（该 server 上是 $4）：%session-changed 必须先
    /// 落地 active_session，list-windows 查询才不能默认 `-t $0`。
    /// 尚未收到 %session-changed 时，用 attach 目标名而不是 $0。
    #[test]
    fn session_changed_sets_active_session_before_list_windows() {
        use crate::core::runtime::tmux::protocol::Message;

        let mut b = TmuxRuntime::new_with_attach(None, "yaklang-workspace");
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        b.cmd_tx = Some(tx);

        // 尚未收到 %session-changed：不得拿没有 id 的 $0 去查，用 attach 目标名。
        b.query_list_windows();
        let first = rx.try_recv().expect("应发出 list-windows");
        assert!(
            first.contains("list-windows -t yaklang-workspace"),
            "attach 已给目标名时不得默认 $0: {first}"
        );
        assert!(!first.contains("-t $0"), "不应查询 $0: {first}");

        // %session-changed $4 yaklang-workspace → active_session = $4。
        b.handle_message(Message::SessionChanged {
            session: crate::core::runtime::tmux::protocol::TmuxSessionId(4),
            name: Some("yaklang-workspace".into()),
        });
        b.query_list_windows();
        let second = rx.try_recv().expect("应再次发出 list-windows");
        assert!(
            second.contains("list-windows -t $4"),
            "SessionChanged 后应查询 $4: {second}"
        );
        assert!(!second.contains("-t $0"), "不应查询 $0: {second}");
    }

    /// 今天的 dogfood 日志：attach 的是 $4，`%session-window-changed $4 @21` 必须切 tab。
    #[test]
    fn session_window_changed_applies_when_attached_session_is_4() {
        use crate::core::model::state::StateChange;
        use crate::core::runtime::tmux::protocol::Message;

        let mut b = TmuxRuntime::new(None);
        let attached = crate::core::runtime::tmux::protocol::TmuxSessionId(4);
        b.active_session = Some(attached);
        b.workspace_name = "yaklang-workspace".into();
        b.tabs.push(crate::core::model::state::TabInfo {
            id: crate::core::types::TabId(21),
            name: "code".into(),
            active: false,
        });
        b.tabs.push(crate::core::model::state::TabInfo {
            id: crate::core::types::TabId(29),
            name: "other".into(),
            active: true,
        });

        b.handle_message(Message::SessionWindowChanged {
            session: attached,
            window: crate::core::types::TabId(21),
        });

        let t21 = b
            .tabs
            .iter()
            .find(|t| t.id == crate::core::types::TabId(21))
            .unwrap();
        assert!(t21.active, "attach session 为 $4 时 @21 应变为 active");
        let t29 = b
            .tabs
            .iter()
            .find(|t| t.id == crate::core::types::TabId(29))
            .unwrap();
        assert!(!t29.active, "@29 应取消 active");
        assert!(
            b.events.iter().any(|e| matches!(
                e,
                StateChange::ActiveTabChanged { tab, .. }
                    if *tab == crate::core::types::TabId(21)
            )),
            "应发 ActiveTabChanged(TabId(21))"
        );
    }

    /// 未拥有的 window 的 %layout-change 不得排队 list-panes。
    #[test]
    fn layout_change_ignores_foreign_window() {
        use crate::core::runtime::tmux::protocol::Message;

        let mut b = TmuxRuntime::new(None);
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        b.cmd_tx = Some(tx);
        b.tabs.push(crate::core::model::state::TabInfo {
            id: crate::core::types::TabId(0),
            name: "Monitor".into(),
            active: true,
        });
        b.handle_message(Message::LayoutChange {
            window: crate::core::types::TabId(20),
            layout: crate::core::runtime::tmux::protocol::LayoutChange::parse("abcd,80x24,0,0,1")
                .unwrap(),
            visible_layout: None,
            flags: None,
        });
        assert!(rx.try_recv().is_err(), "外站 window 不应触发 list-panes");
        assert!(!b
            .window_layouts
            .contains_key(&crate::core::types::TabId(20)));
    }

    /// Task::SwitchTab 应乐观更新 active tab：即使 %session-window-changed
    /// 在输出洪峰下延迟到达，前端也能立刻切 tab；通知到达后不重复发事件。
    #[test]
    fn switch_tab_optimistically_marks_active_tab() {
        use crate::core::model::state::StateChange;
        use crate::core::runtime::tmux::protocol::Message;

        let mut b = TmuxRuntime::new(None);
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        b.cmd_tx = Some(tx);
        b.status = crate::core::model::state::BackendStatus::Connected;
        let session = crate::core::runtime::tmux::protocol::TmuxSessionId(0);
        b.workspace_name = "s0".into();
        b.active_session = Some(session);
        for (id, active) in [(0u32, true), (1, false), (2, false)] {
            b.tabs.push(crate::core::model::state::TabInfo {
                id: crate::core::types::TabId(id),
                name: format!("t{id}"),
                active,
            });
        }

        // 切到 tab2：命令发出 + 乐观事件
        let outcome = b.execute(&Task::SwitchTab {
            target: crate::core::types::TabId(2),
        });
        assert!(matches!(outcome, Ok(TaskOutcome::Done)));
        let sent = rx.try_recv().expect("应发送 select-window");
        assert!(sent.starts_with("select-window -t @2"), "命令: {sent}");
        assert!(
            b.events
                .iter()
                .any(|e| matches!(e, StateChange::ActiveTabChanged { tab, .. } if *tab == crate::core::types::TabId(2))),
            "乐观切换应立即产生 ActiveTabChanged(tab2)"
        );
        let t2 = b
            .tabs
            .iter()
            .find(|t| t.id == crate::core::types::TabId(2))
            .unwrap();
        assert!(t2.active);

        // tmux 通知到达：状态一致，不应重复发 ActiveTabChanged(tab2)
        let before = b.events.len();
        b.handle_message(Message::SessionWindowChanged {
            session,
            window: crate::core::types::TabId(2),
        });
        let after = b.events.len();
        assert_eq!(after, before, "幂等通知不应重复产生 ActiveTabChanged");
    }

    /// %extended-output（hyperlink 等）被安全忽略，不破坏 %output 累积或状态机。
    #[test]
    fn extended_output_safely_ignored() {
        use crate::core::runtime::tmux::protocol::Message;

        let mut b = TmuxRuntime::new(None);
        let pane = PaneId(9);

        b.handle_message(Message::Output {
            pane,
            content: b"x".to_vec(),
            raw_content: String::new(),
        });
        // 穿插一个 pause-after 的 %extended-output
        b.handle_message(Message::ExtendedOutput {
            pane,
            age_ms: 12,
            content: b"z".to_vec(),
            raw_content: "z".into(),
        });
        b.handle_message(Message::Output {
            pane,
            content: b"y".to_vec(),
            raw_content: String::new(),
        });

        // output 与 extended-output 都按增量累积（xzy），不互相打断
        let out = b.outputs.get(&pane).cloned().unwrap_or_default();
        assert_eq!(out, b"xzy", "%extended-output 应与 %output 同路径累积");
    }

    /// 布局变化与 %output 交织：%layout-change 不应重置已累积的 pane 输出。
    #[test]
    fn layout_change_does_not_reset_pane_output() {
        use crate::core::runtime::tmux::protocol::Message;

        let mut b = TmuxRuntime::new(None);
        let pane = PaneId(3);

        // 先灌入一些输出
        b.handle_message(Message::Output {
            pane,
            content: b"before-layout".to_vec(),
            raw_content: String::new(),
        });

        // 插入 %layout-change（带合法 layout 字符串）
        b.handle_message(Message::LayoutChange {
            window: crate::core::types::TabId(0),
            layout: crate::core::runtime::tmux::protocol::LayoutChange::parse("80x24,0,0,0")
                .unwrap(),
            visible_layout: None,
            flags: None,
        });

        // 布局变化后再来输出
        b.handle_message(Message::Output {
            pane,
            content: b"-after-layout".to_vec(),
            raw_content: String::new(),
        });

        // 输出累积应完整（前 + 后），布局变化不重置
        let out = b.outputs.get(&pane).cloned().unwrap_or_default();
        assert_eq!(
            String::from_utf8_lossy(&out),
            "before-layout-after-layout",
            "layout-change 不应重置 pane 输出累积"
        );
    }

    /// 回归：shutdown 必须在有限时间内返回（含清理 outputs）。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shutdown_completes_and_clears_buffers() {
        let socket = unique_socket();
        let run = async {
            let mut b = TmuxRuntime::new(Some(&socket));
            if b.connect().await.is_err() {
                return;
            }
            // 人为塞一点输出缓冲
            b.handle_message(crate::core::runtime::tmux::protocol::Message::Output {
                pane: PaneId(1),
                content: vec![b'z'; 1024],
                raw_content: String::new(),
            });
            assert!(!b.outputs.is_empty());
            let _ = b.shutdown().await;
            assert!(b.outputs.is_empty(), "shutdown 后应清空 outputs");
            assert_eq!(b.status(), BackendStatus::Exited);
        };
        let timed = tokio::time::timeout(TMUX_TEST_TIMEOUT, run).await;
        cleanup(&socket);
        assert!(timed.is_ok(), "shutdown_completes_and_clears_buffers 超时");
    }
}
