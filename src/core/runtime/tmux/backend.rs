//! TmuxRuntime：tmux -CC 控制模式后端。
//!
//! 封装现有 `runtime::tmux::client`（spawn tmux -CC + 事件流）和
//! `runtime::tmux::command`（强类型命令构造器），实现 `Runtime` trait。
//!
//! 设计：
//! - `connect()`：spawn tmux -CC new-session，drain 启动事件建立初始 state
//!   （session / 第一个 window / 第一个 pane）
//! - 后台 task 持续读 `TmuxEvent`，把 `Message` 转成内部 state 更新 +
//!   `StateChange` 事件入队；命令响应正文由 reader 聚合成 block 后按边界处理
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
use crate::core::config::Rgb;
use crate::core::model::backend::{Runtime, RuntimeCapability};
use crate::core::model::layout::{LayoutNode, SplitDir, TabLayout};
use crate::core::model::state::{BackendStatus, PaneInfo, State, StateChange, TabInfo};
use crate::core::model::task::{Task, TaskOutcome};
use crate::core::runtime::tmux::client::{
    ConnectMode, TmuxClient, TmuxClientConfig, TmuxClientHandle, TmuxEvent, TmuxEventReceiver,
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
    /// 可见屏 seed 之后按行回填 attach 前历史。不 pause。
    PaneHistory { pane: PaneId },
    /// display-message format：查询 resync 时需要重放的 VT 状态。
    PaneResyncState { pane: PaneId, generation: u64 },
    /// resync 的 primary/alternate capture。
    PaneResyncCapture {
        pane: PaneId,
        alternate: bool,
        generation: u64,
    },
    /// list-sessions：列出 tmux server 上所有 session。
    ListSessions,
    /// 新 tab 未指定目录时，先查询当前 pane cwd，再发 new-window -c。
    NewTabInCurrentDir {
        pane: PaneId,
        session: TmuxSessionId,
        name: Option<String>,
        command: Option<Vec<String>>,
    },
}

/// status bar 订阅名（文档 §B+：`refresh-client -B` 的名字）。
pub const STATUS_LEFT_SUBSCRIPTION: &str = "muxterm.status-left";
const PANE_CMD_SUBSCRIPTION: &str = "muxterm.pane-cmd";
pub const STATUS_RIGHT_SUBSCRIPTION: &str = "muxterm.status-right";

/// 单轮最多处理的控制模式事件。输出洪峰不能让一次 UI poll 无界排空
/// channel，否则后台 pane 会把连接/切 tab 的主线程路径拖住。
const PUMP_EVENT_BUDGET: usize = 2_048;
/// 即使事件数没有达到上限，也要把主线程还给 UI。下一轮 poll 会继续处理
/// 剩余事件；结构事件仍会被保留在 core 队列中。
const PUMP_TIME_BUDGET: Duration = Duration::from_millis(4);
/// Snapshot 是恢复手段，不得把 pane 永久锁在 resyncing；5s 也与 iTerm2 的
/// tmux unresponsive watchdog 保持同一量级。
const RESYNC_TIMEOUT: Duration = Duration::from_secs(5);
/// attach 首屏允许更慢（SSH RTT、远端 TUI）。慢可以，但不能在 5s 时空屏
/// 再抓 `-S -10000` 把控制通道卡死。
const INITIAL_SEED_TIMEOUT: Duration = Duration::from_secs(20);
const RESYNC_COOLDOWN: Duration = Duration::from_secs(5);

/// 单个 pane 的输出流控状态。
#[derive(Debug, Default)]
struct PaneFlow {
    /// 外部 `%pause` 且无法发起查询时的兼容缓冲。
    suppressed: Vec<u8>,
    /// 正在进行 authoritative snapshot transaction。
    resyncing: bool,
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
    generation: u64,
    deadline: Option<Instant>,
    state: Option<PaneReplayState>,
    primary: Option<Vec<u8>>,
    alternate: Option<Vec<u8>>,
    live: Vec<u8>,
    /// attach 首次播种：pause 控制 client 输出，capture 开始前的通知可能
    /// 已经进快照，capture 边界之后的字节才需要 catch-up。
    initial: bool,
    capture_started: bool,
    pre_capture: Vec<u8>,
    post_capture: Vec<u8>,
    /// snapshot 入队后再 `refresh-client -A continue`。
    pause_client: bool,
}

/// tmux -CC 后端。
pub struct TmuxRuntime {
    config: TmuxClientConfig,
    handle: Option<TmuxClientHandle>,
    event_rx: Option<TmuxEventReceiver>,
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
    /// reader 在响应过大时保留尾部并标记截断；该响应不能被当成完整
    /// list/capture 使用，必须走同一套失败释放路径。
    truncated_responses: HashSet<i64>,
    /// 等待响应的命令回调（number → 处理函数）。简化为存命令类型标记。
    pending_queries: VecDeque<PendingQuery>,
    /// `%begin <number>` 到达时从 pending_queries 队首取出的查询，按 number 登记。
    ///
    /// tmux 控制模式是串行的，但高输出下 `%begin/%end` 仍可能与多个在途查询
    /// 交叠。按 number 匹配能避免用简单的 FIFO `pop_front` 错配查询。
    pending_by_number: HashMap<i64, PendingQuery>,
    /// 缓存每个 tab（tmux window）的 layout 字符串（从 list-windows 响应获取），用于重建 LayoutNode。
    window_layouts: HashMap<TabId, String>,
    /// tmux 的可变 window_index。TabInfo 只保存稳定 window id；这里单独
    /// 记录 index，供权威 list-windows 响应后重排稀疏窗口（如 1,2,3,5,7
    /// 新建出 6）而不改 FFI ABI。
    window_indices: HashMap<TabId, u32>,
    /// 当前处于 zoom（`resize-pane -Z` / prefix-z）的 tab。
    /// 此时 tmux 的 `window_layout` 仍是完整 split 树，GUI 必须只渲染 active pane。
    window_zoomed: HashSet<TabId>,
    /// 最近一次由本地 UI 发出的切 tab 目标。tmux 的旧
    /// `%session-window-changed` 可能迟到；有新目标 pending 时只接受该
    /// 目标的确认，避免 @18 → @47 → @18 来回跳。
    latest_switch_target: Option<TabId>,
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
    /// attach 建立后给后台 tab 做轻量索引播种的开关。它只影响异步的可见
    /// 屏 capture，不让 connect 等待，也不提前创建前端 Surface。
    background_index_capture_enabled: bool,
    /// 已经在 Connected 之后发过一轮后台可见屏索引。connect 里只开开关。
    background_index_started: bool,
    /// 最近一次 capture 只取了可见屏。切入时直接用它，不再 pause 重抓。
    background_capture_only: HashSet<PaneId>,
    /// 发出 capture 时的 pane 网格。完成时不得用事后尺寸覆盖；切 tab
    /// 时若当前 cols/rows 对不上，可见索引已经过期，必须 pause 再抓。
    capture_grid: HashMap<PaneId, (u16, u16)>,
    /// 已经按行回填过 attach 前历史的 pane。切 tab 不得再抓。
    history_backfill_done: HashSet<PaneId>,
    /// 历史 capture 还在路上。
    history_backfill_pending: HashSet<PaneId>,
    /// 可见屏已经有了，等控制通道空闲再抓历史。切 tab 当拍不得发 `-S`。
    history_backfill_wanted: HashSet<PaneId>,
    /// 本轮 pump 里刚切了 tab / 刚种完可见屏。历史放到下一轮 poll。
    history_backfill_hold: bool,
    /// 被 `%pause` 暂停输出的 pane（`%continue` 恢复；供背压/诊断）。
    paused_panes: HashSet<PaneId>,
    /// 每个 pane 的输出速率窗口（洪峰 pause / 合并）。
    flow: HashMap<PaneId, PaneFlow>,
    /// 正在进行的 authoritative pane snapshot transaction。
    resyncs: HashMap<PaneId, PaneResync>,
    /// 每个 pane 的 resync generation，迟到的 query 响应只能命中原 generation。
    resync_generation: HashMap<PaneId, u64>,
    /// timeout/error 后的冷却窗口，阻止同一 output gap 立刻重放 capture 风暴。
    resync_cooldown_until: HashMap<PaneId, Instant>,
    /// 事件队列因有界保护而丢弃了某个 pane 的增量。只有这种“确实丢过
    /// 字节”的情况才需要 authoritative resync；正常的高频 CUP 输出不能
    /// 因为时间/字节阈值被主动重拍。
    dropped_output_panes: HashSet<PaneId>,
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
    /// 活动 tab 的 display-message 还没换成可见 `capture-pane`。这期间
    /// `list-sessions` / `-B` / 全 pane `-r` 必须排队，否则 SSH 上会把
    /// 首屏挤过 deadline（1612：pause 之后 9s 都没发出 capture）。
    attach_followup_held: bool,
    /// 已经补发过 attach 后续命令，避免 timeout/capture 各 flush 一次。
    attach_followup_flushed: bool,
    /// seed 进行中收到的 OSC 颜色上报，等可见 capture 发出后再写 tmux。
    held_colour_reports: Vec<(PaneId, Rgb, Rgb)>,
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
/// 非 attach 的索引 dump 去掉尾部纯空白行。attach 的 Surface seed 必须走
/// [`capture_pane_surface_bytes`]：按行 CUP 铺网格，不裁尾部空行，也不用
/// `\r\n` 把光标推过 prompt。resync/TUI 快照走 [`capture_pane_grid_bytes`]。
fn capture_pane_bytes(lines: &[String]) -> Vec<u8> {
    capture_pane_lines(lines, true)
}

/// 保留 capture-pane 的完整网格，包括尾部空行。
fn capture_pane_grid_bytes(lines: &[String]) -> Vec<u8> {
    capture_pane_lines(lines, false)
}

/// attach 索引用的可见屏：从 home 按行地址写入，空行留给 reset 后的空白格。
/// 最后画出的是最后一行非空内容，光标停在 prompt / TUI 输入盒，不会掉到
/// pane 底。pi/Cursor 的中间空行和底栏因此还在原来的格子上。
fn capture_pane_surface_bytes(lines: &[String]) -> Vec<u8> {
    let mut body = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        push_csi(&mut body, &format!("{}H", i + 1));
        body.extend_from_slice(line.as_bytes());
    }
    if body.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    push_csi(&mut out, "H");
    out.extend(body);
    out
}

fn capture_pane_lines(lines: &[String], trim_trailing_blank: bool) -> Vec<u8> {
    let mut end = lines.len();
    if trim_trailing_blank {
        while end > 0 && lines[end - 1].trim().is_empty() {
            end -= 1;
        }
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
    let alternate_on = state.is_some_and(|s| s.alternate_on);
    if alternate_on {
        // 当前屏已经是 TUI。把 saved primary 当 VT 流先灌一遍再 1049h，
        // htop/pi 会先花一屏 shell 历史，列也会对不齐。iTerm2 是直接铺
        // 当前网格；这里只进 alternate 再画可见屏。
        out.extend_from_slice(b"\x1b[?1049h");
        out.extend_from_slice(alternate);
    } else {
        out.extend_from_slice(primary);
    }
    if let Some(state) = state {
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
            truncated_responses: HashSet::new(),
            pending_queries: VecDeque::new(),
            pending_by_number: HashMap::new(),
            window_layouts: HashMap::new(),
            window_indices: HashMap::new(),
            window_zoomed: HashSet::new(),
            latest_switch_target: None,
            expected_panes_per_window: HashMap::new(),
            pending_close_tabs: HashSet::new(),
            initial_capture_pending: HashSet::new(),
            initial_capture_done: HashSet::new(),
            background_index_capture_enabled: false,
            background_index_started: false,
            background_capture_only: HashSet::new(),
            capture_grid: HashMap::new(),
            history_backfill_done: HashSet::new(),
            history_backfill_pending: HashSet::new(),
            history_backfill_wanted: HashSet::new(),
            history_backfill_hold: false,
            paused_panes: HashSet::new(),
            flow: HashMap::new(),
            resyncs: HashMap::new(),
            resync_generation: HashMap::new(),
            resync_cooldown_until: HashMap::new(),
            dropped_output_panes: HashSet::new(),
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
            attach_followup_held: false,
            attach_followup_flushed: false,
            held_colour_reports: Vec::new(),
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
    /// 已经 seed 过、且 capture 网格仍匹配当前尺寸的 pane，切过来只显示，
    /// 不要再 pause/capture。后台抓的网格若已经过期（窗口在后台被
    /// resize），必须作废再抓。从未抓过的 pane 才走 `query_capture_pane`。
    fn query_capture_tab(&mut self, tab: TabId) {
        let panes: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|pane| pane.tab == tab)
            .map(|pane| pane.id)
            .collect();
        for pane in panes {
            if self.background_capture_only.contains(&pane) && self.capture_grid_stale(pane) {
                self.initial_capture_done.remove(&pane);
                self.background_capture_only.remove(&pane);
                self.query_capture_pane(pane);
                continue;
            }
            if self.initial_capture_done.contains(&pane) {
                self.background_capture_only.remove(&pane);
                // 可见屏已经有了。历史放到下一轮 poll，不要和 select-window
                // 抢控制通道。
                self.schedule_pane_history_backfill(pane);
                continue;
            }
            if self.background_capture_only.contains(&pane) {
                if self.initial_capture_pending.contains(&pane) {
                    // 可见屏还在路上：等它完成即可，不要再排队一轮 pause seed。
                    continue;
                }
                self.background_capture_only.remove(&pane);
                self.initial_capture_done.insert(pane);
                self.schedule_pane_history_backfill(pane);
                continue;
            }
            self.query_capture_pane(pane);
        }
    }

    fn pane_grid_size(&self, pane: PaneId) -> Option<(u16, u16)> {
        self.panes
            .iter()
            .find(|item| item.id == pane)
            .map(|item| (item.cols, item.rows))
    }

    /// 记下发出 capture 那一刻的网格。完成响应时不要用事后尺寸覆盖。
    fn record_capture_grid(&mut self, pane: PaneId) {
        if let Some(size) = self.pane_grid_size(pane) {
            self.capture_grid.insert(pane, size);
        }
    }

    /// 没有记录时不当过期，避免旧单测/未索引 pane 误触发 pause。
    fn capture_grid_stale(&self, pane: PaneId) -> bool {
        let Some(captured) = self.capture_grid.get(&pane) else {
            return false;
        };
        self.pane_grid_size(pane)
            .is_some_and(|current| current != *captured)
    }

    fn forget_pane_capture_grid(&mut self, pane: PaneId) {
        self.capture_grid.remove(&pane);
    }

    /// 后台 tab 的轻量首屏 capture：只为 Core 索引提供当前可见内容，响应
    /// 不参与 connect 就绪判定。前台 Workspace 用对应 `PaneSnapshot` 种 Surface；
    /// 切 tab 时若网格仍匹配，不再 pause 重抓；尺寸过期则 `query_capture_tab` 会作废再抓。
    fn query_background_index_tab(&mut self, tab: TabId) {
        let panes: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|pane| pane.tab == tab)
            .map(|pane| pane.id)
            .collect();
        for pane in panes {
            self.query_capture_pane_visible(pane);
        }
    }

    fn query_background_index_captures(&mut self) {
        if !self.background_index_capture_enabled {
            return;
        }
        let active = self.active_tab_id();
        let tabs: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|tab| Some(tab.id) != active)
            .map(|tab| tab.id)
            .collect();
        for tab in tabs {
            self.query_background_index_tab(tab);
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

    /// attach 初始连接交给前端前，确认活动 tab 的 Surface seed 至少已经
    /// 完成。这里只看活动 tab；后台 tab 的可见屏/历史 capture 仍然异步，
    /// 避免慢的 scrollback 阻塞 Connect 或命令面板。
    fn active_tab_capture_ready(&self) -> bool {
        let Some(tab) = self.active_tab_id() else {
            return false;
        };
        let panes: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|pane| pane.tab == tab)
            .map(|pane| pane.id)
            .collect();
        !panes.is_empty()
            && panes
                .iter()
                .all(|pane| self.initial_capture_done.contains(pane))
    }

    /// attach 后所有已知 tab 的轻量可见屏索引是否已经完成。后台只用
    /// `capture-pane -p`，因此这个检查不会把大段 scrollback 带回连接路径。
    fn attach_visible_captures_ready(&self) -> bool {
        !self.tabs.is_empty()
            && self.tabs.iter().all(|tab| {
                let Some(expected) = self.expected_panes_per_window.get(&tab.id) else {
                    return false;
                };
                let panes: Vec<PaneId> = self
                    .panes
                    .iter()
                    .filter(|pane| pane.tab == tab.id)
                    .map(|pane| pane.id)
                    .collect();
                panes.len() >= *expected
                    && panes
                        .iter()
                        .all(|pane| self.initial_capture_done.contains(pane))
            })
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
            if let Some(StateChange::PaneOutput { pane, .. }) = self.events.remove(idx) {
                self.dropped_output_panes.insert(pane);
            }
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

    /// 记录一次 pane 输出。正常情况下原始字节逐块交付；只有事件队列
    /// 确实丢弃过该 pane 的增量时，`maybe_start_resyncs` 才会启动
    /// authoritative snapshot。
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
    }

    /// 收到 `%pause`/`%continue` 时把合并缓冲立即交付（暂停期间不丢字节）。
    fn flush_suppressed_output(&mut self, pane: PaneId) {
        let suppressed = std::mem::take(&mut self.flow.entry(pane).or_default().suppressed);
        if !suppressed.is_empty() {
            self.push_pane_output(pane, suppressed);
        }
    }

    /// 启动一次不会丢帧的 pane resync transaction。
    fn begin_pane_resync(&mut self, pane: PaneId, reason: &'static str) {
        self.begin_pane_snapshot(pane, reason, false, true);
    }

    /// attach 首次 Surface seed：先 pause 该 pane 的控制输出（与 iTerm2
    /// TmuxWindowOpener 一样），再抓当前可见网格和 alternate/cursor。
    /// pause 让 tmux 不再往这条 client 堆 %output，seed 才能在 deadline
    /// 内完成。可见屏一轮就能回来，不能再抓 `-S -10000`。
    fn begin_initial_pane_seed(&mut self, pane: PaneId) {
        self.begin_pane_snapshot(pane, "initial-seed", true, true);
    }

    fn begin_pane_snapshot(
        &mut self,
        pane: PaneId,
        reason: &'static str,
        initial: bool,
        pause_client: bool,
    ) {
        if self.resyncs.contains_key(&pane)
            || self
                .resync_cooldown_until
                .get(&pane)
                .is_some_and(|until| *until > Instant::now())
        {
            return;
        }
        // 不要在 transaction 开始时删除已经排队的 PaneOutput。只有完整 snapshot
        // 成功时才原子替换旧增量；如果查询堵塞/失败，旧增量仍是唯一可用 fallback。
        let generation = self
            .resync_generation
            .entry(pane)
            .and_modify(|value| *value = value.saturating_add(1))
            .or_insert(1);
        let generation = *generation;
        if let Some(flow) = self.flow.get_mut(&pane) {
            flow.resyncing = true;
            flow.suppressed.clear();
        }
        let timeout = if initial {
            INITIAL_SEED_TIMEOUT
        } else {
            RESYNC_TIMEOUT
        };
        self.resyncs.insert(
            pane,
            PaneResync {
                generation,
                deadline: Some(Instant::now() + timeout),
                initial,
                pause_client,
                ..PaneResync::default()
            },
        );
        if initial {
            self.initial_capture_pending.insert(pane);
            self.initial_capture_done.remove(&pane);
        }
        if pause_client {
            self.paused_panes.insert(pane);
        }

        if pause_client
            && self
                .dispatch_tmux_command(&cmd::refresh_client_pause(pane, true))
                .is_err()
        {
            self.abort_pane_resync(pane, "pause-command-failed");
            return;
        }
        self.record_capture_grid(pane);
        let query = cmd::display_message(PaneId(pane.0), PANE_RESYNC_FORMAT);
        if self.dispatch_tmux_command(&query).is_ok() {
            self.replace_last_pending(PendingQuery::PaneResyncState { pane, generation });
            tracing::info!(
                target: "muxterm::tmux::resync",
                pane = pane.0,
                reason,
                "paused pane and requested authoritative state/capture"
            );
        } else {
            self.abort_pane_resync(pane, "state-query-failed");
        }
    }

    fn pending_query_pane(query: &PendingQuery) -> Option<PaneId> {
        match query {
            PendingQuery::ReadyProbe { pane }
            | PendingQuery::DisplayMessage { pane }
            | PendingQuery::CapturePane { pane }
            | PendingQuery::PaneHistory { pane }
            | PendingQuery::PaneResyncState { pane, .. }
            | PendingQuery::PaneResyncCapture { pane, .. }
            | PendingQuery::NewTabInCurrentDir { pane, .. } => Some(*pane),
            PendingQuery::Ignore
            | PendingQuery::ListPanes { .. }
            | PendingQuery::ListWindows
            | PendingQuery::ListSessions => None,
        }
    }

    /// 取消某 pane 的所有在途查询但保留 FIFO tombstone。这样 timeout 后迟到的
    /// `%begin` 仍会消耗原 query 槽位，不会错配到下一次 resync。
    fn cancel_pending_queries_for_pane(&mut self, pane: PaneId) {
        for query in &mut self.pending_queries {
            if Self::pending_query_pane(query) == Some(pane) {
                if matches!(query, PendingQuery::PaneHistory { .. }) {
                    self.history_backfill_pending.remove(&pane);
                }
                *query = PendingQuery::Ignore;
            }
        }
        let numbers: Vec<i64> = self
            .pending_by_number
            .iter()
            .filter_map(|(number, query)| {
                (Self::pending_query_pane(query) == Some(pane)).then_some(*number)
            })
            .collect();
        for number in numbers {
            if matches!(
                self.pending_by_number.get(&number),
                Some(PendingQuery::PaneHistory { .. })
            ) {
                self.history_backfill_pending.remove(&pane);
            }
            self.pending_by_number.insert(number, PendingQuery::Ignore);
            self.response_accum.remove(&number);
        }
    }

    /// 尺寸变了：旧网格作废，保持 pause，立刻再抓当前可见屏。
    fn restart_snapshot_after_resize(&mut self, pane: PaneId) {
        let Some(resync) = self.resyncs.get(&pane) else {
            return;
        };
        let initial = resync.initial;
        self.release_pane_resync(pane, "pane-size-changed", false);
        self.resync_cooldown_until.remove(&pane);
        if initial {
            self.initial_capture_done.remove(&pane);
            self.begin_initial_pane_seed(pane);
        } else {
            self.begin_pane_resync(pane, "pane-size-changed");
        }
    }

    /// 失败/超时时释放 resync。永远不要让旧 pane 画面成为 transaction 成功的
    /// 唯一前提：能交付的 live bytes 先交付，迟到响应全部按 tombstone 忽略。
    fn abort_pane_resync(&mut self, pane: PaneId, reason: &'static str) {
        self.release_pane_resync(pane, reason, true);
    }

    fn release_pane_resync(&mut self, pane: PaneId, reason: &'static str, deliver: bool) {
        let Some(resync) = self.resyncs.remove(&pane) else {
            return;
        };
        self.cancel_pending_queries_for_pane(pane);
        if !deliver {
            if let Some(flow) = self.flow.get_mut(&pane) {
                flow.resyncing = false;
                flow.suppressed.clear();
            }
            tracing::info!(
                target: "muxterm::tmux::resync",
                pane = pane.0,
                generation = resync.generation,
                reason,
                "pane snapshot dropped; recapturing at new size"
            );
            return;
        }
        self.dropped_output_panes.remove(&pane);
        self.resync_cooldown_until
            .insert(pane, Instant::now() + RESYNC_COOLDOWN);

        let mut fallback = resync.pre_capture;
        fallback.extend(resync.post_capture);
        fallback.extend(resync.live);
        let suppressed = if let Some(flow) = self.flow.get_mut(&pane) {
            flow.resyncing = false;
            std::mem::take(&mut flow.suppressed)
        } else {
            Vec::new()
        };
        fallback.extend(suppressed);
        let had_content = !fallback.is_empty();
        if resync.initial {
            // 首屏 seed 超时也必须发 PaneSnapshot，否则 macOS 会把 host 一直
            // 藏在 seedingPanes 里，用户看到的就是 tab 卡住。
            self.initial_capture_pending.remove(&pane);
            self.initial_capture_done.insert(pane);
            self.background_capture_only.remove(&pane);
            self.outputs.insert(pane, fallback.clone());
            self.events.retain(
                |event| !matches!(event, StateChange::PaneOutput { pane: p, .. } if *p == pane),
            );
            self.events.push_back(StateChange::PaneSnapshot {
                pane,
                data: fallback,
            });
            self.trim_event_queue();
        } else if had_content {
            self.push_pane_output(pane, fallback);
        }
        self.paused_panes.remove(&pane);
        if resync.pause_client {
            let _ = self.dispatch_tmux_command(&cmd::refresh_client_pause(pane, false));
        }
        if resync.initial && had_content {
            self.schedule_pane_history_backfill(pane);
        }
        if let Some(receiver) = self.event_rx.as_mut() {
            receiver.resume_output_pane(pane);
        }
        self.release_attach_followup_if_ready();
        tracing::warn!(
            target: "muxterm::tmux::resync",
            pane = pane.0,
            generation = resync.generation,
            reason,
            "pane resync released with live fallback"
        );
    }

    fn expire_resyncs(&mut self) {
        let now = Instant::now();
        let expired: Vec<PaneId> = self
            .resyncs
            .iter()
            .filter_map(|(pane, resync)| {
                resync
                    .deadline
                    .is_some_and(|deadline| deadline <= now)
                    .then_some(*pane)
            })
            .collect();
        for pane in expired {
            self.abort_pane_resync(pane, "deadline");
        }
    }

    fn maybe_start_resyncs(&mut self) {
        if self.cmd_tx.is_none() {
            return;
        }
        // 只有 trim_event_queue 确实移除了 PaneOutput，或 tmux 发出明确的
        // `%pause`，才启动 snapshot。正常的 OMP/CUP burst 即使跨过数十 KB
        // 也必须原样 feed，不能让 Surface 跳到历史顶部再跳回输入框。
        let panes: Vec<PaneId> = self.dropped_output_panes.drain().collect();
        for pane in panes {
            if self.initial_capture_done.contains(&pane)
                || self.is_attach_mode()
                || self.initial_seed_blocks_followup()
            {
                // attach / 已打开 Surface：丢字节只恢复 live。pause+resync
                // 会把活动 tab 的 display-message 挤出 deadline（1612）。
                if let Some(receiver) = self.event_rx.as_mut() {
                    receiver.resume_output_pane(pane);
                }
                continue;
            }
            if !self.resyncs.contains_key(&pane) {
                self.begin_pane_resync(pane, "output-dropped");
            }
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
        // `%window-add` 没有携带 window_index。立即补一份权威列表，避免
        // 新 tab 先 append 到稳定 id 顺序后，Alt-6/Alt-7 与状态栏短暂对调。
        let _ = self.query_list_windows();
    }

    /// tmux window 关闭 → muxterm Tab 关闭。
    /// `%window-close` 与 `%unlinked-window-close` 共用。
    /// 真正关闭一个 tab：先逐 pane 发 PaneClosed，前端才能回收对应的终端视图；
    /// 只发 TabClosed 会让切 tab 后保留的视图泄漏（视图只在 PaneClosed 时移除）。
    fn remove_window_tab(&mut self, tab: TabId) {
        if self.latest_switch_target == Some(tab) {
            self.latest_switch_target = None;
        }
        let closed: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|p| p.tab == tab)
            .map(|p| p.id)
            .collect();
        for id in closed {
            self.forget_pane_capture_grid(id);
            self.events.push_back(StateChange::PaneClosed { pane: id });
        }
        self.panes.retain(|p| p.tab != tab);
        self.layouts.remove(&tab);
        self.window_indices.remove(&tab);
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
                if let Some(resync) = self.resyncs.get_mut(&pane) {
                    let target = if resync.initial && resync.capture_started {
                        &mut resync.post_capture
                    } else if resync.initial {
                        &mut resync.pre_capture
                    } else {
                        &mut resync.live
                    };
                    append_capped(target, &content, MAX_PANE_OUTPUT_BYTES);
                    return;
                }
                // attach 的初始控制流可能先发一个 prompt，再由 list-panes
                // 查询完整屏幕。先暂存这段不完整输出（而不是直接丢弃），
                // capture-pane 返回后以完整快照初始化，并把暂存的实时增量
                // 拼到快照尾部；这样既保留完整屏幕又不丢查询期间的输出。
                if self.is_attach_mode() && !self.initial_capture_done.contains(&pane) {
                    // 若尚未发起 capture 查询（pending 未建立），说明此时
                    // 只是启动期提示。活动 pane 在 capture 返回前仍保持
                    // seed 边界；已知的后台 pane 则直接索引这些输出，保证
                    // 搜索不会漏掉 attach 前已经写入的 token。前端会过滤
                    // 没有可见/已有 Surface 的后台 pane，不会因此创建不可见
                    // 的 SwiftTerm view。
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
                        let is_background_pane = self
                            .panes
                            .iter()
                            .find(|candidate| candidate.id == pane)
                            .is_some_and(|candidate| !self.tab_is_active(candidate.tab));
                        if is_background_pane {
                            self.note_pane_output(pane, &content);
                            tracing::trace!(
                                target: "muxterm::tmux",
                                pane = pane.0,
                                "attach 未 capture 的 pane 输出已进入索引"
                            );
                        } else {
                            tracing::trace!(
                                target: "muxterm::tmux",
                                pane = pane.0,
                                "attach 活动 pane 输出暂存，等待 capture 快照"
                            );
                        }
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
                let zoomed = window_is_zoomed(
                    flags.as_deref(),
                    &layout.raw,
                    visible_layout.as_ref().map(|v| v.raw.as_str()),
                );
                let layout_unchanged = self.window_layouts.get(&tab) == Some(&layout.raw);
                let zoom_unchanged = self.window_zoomed.contains(&tab) == zoomed;
                let has_panes = self.panes.iter().any(|p| p.tab == tab);
                if layout_unchanged && zoom_unchanged && has_panes {
                    // refresh-client -C / 切 tab 会给每个 window 再推一次
                    // 相同 layout。list-panes 是 SSH 往返，N 个 tab 就会卡一下。
                    tracing::trace!(
                        target: "muxterm::tmux",
                        window = window.0,
                        "%layout-change 布局未变，跳过 list-panes"
                    );
                    return;
                }
                tracing::debug!(
                    target: "muxterm::tmux",
                    window = window.0,
                    layout = %layout.raw,
                    flags = flags.as_deref().unwrap_or(""),
                    "%layout-change 已保存并重新查询 pane"
                );
                self.window_layouts.insert(tab, layout.raw.clone());
                if zoomed {
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
                let tab_id = TabId(window.0);
                if let Some(target) = self.latest_switch_target {
                    if target != tab_id {
                        tracing::debug!(
                            target: "muxterm::tmux",
                            requested = target.0,
                            stale = tab_id.0,
                            "忽略迟到的旧 tab 切换确认"
                        );
                        return;
                    }
                    self.latest_switch_target = None;
                }
                // tmux session 的 active window 切换 → muxterm active tab 切换
                self.mark_tab_active(tab_id);
                // 活动 tab 首次出现且从未抓过屏时才 seed。已经有后台索引
                // 的 pane 直接显示，避免切 tab 再 pause 一次。
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
                if let Some(resync) = self.resyncs.get_mut(&pane) {
                    let target = if resync.initial && resync.capture_started {
                        &mut resync.post_capture
                    } else if resync.initial {
                        &mut resync.pre_capture
                    } else {
                        &mut resync.live
                    };
                    append_capped(target, &content, MAX_PANE_OUTPUT_BYTES);
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
                    if self.resyncs.contains_key(&pane) {
                        // 我们自己发的 pause 回声，不要再开一轮 resync。
                        return;
                    }
                    // TODO(surface-7.4): 洪水 pause-after。某个 Surface 跟不上
                    // 时，只对该 pane `refresh-client -A %N:pause`，追上再
                    // continue。本轮不实现；切 tab 也不得走这条 pause 刷新。
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
        let connected = self.status == BackendStatus::Connected;
        if connected {
            self.history_backfill_hold = false;
        }
        self.expire_resyncs();
        for _ in 0..PUMP_EVENT_BUDGET {
            let message = self
                .command_error_rx
                .as_mut()
                .and_then(|rx| rx.try_recv().ok());
            let Some(message) = message else { break };
            tracing::error!(target: "muxterm::tmux", "发送 tmux 命令失败: {message}");
            self.status = BackendStatus::Error;
            self.events
                .push_back(StateChange::BackendStatusChanged(BackendStatus::Error));
        }
        self.release_deferred_writes();
        self.poll_ready_probes();

        let started = Instant::now();
        let mut processed = 0usize;
        while processed < PUMP_EVENT_BUDGET && started.elapsed() < PUMP_TIME_BUDGET {
            // 只借用 receiver 取出一个事件，随后立刻释放借用，允许下面的
            // state/response 处理继续修改 self。剩余事件留在 channel，交给
            // 下一轮 poll，避免一次 OMP 洪峰独占 UI 线程。
            let ev = self.event_rx.as_mut().and_then(|rx| rx.try_recv().ok());
            let Some(ev) = ev else { break };
            processed += 1;
            match ev {
                TmuxEvent::Message(msg) => {
                    // 先处理 ResponseBoundary（begin/end 状态机），再处理其他消息。
                    if let Message::ResponseBoundary(b) = &msg {
                        match b.kind {
                            NotificationKind::Begin => {
                                self.response_accum.insert(b.number, Vec::new());
                                self.truncated_responses.remove(&b.number);
                                // tmux 串行执行命令：`%begin <n>` 到达时，队首查询即
                                // 该命令的响应槽。按 number 登记，end/error 时精确匹配，
                                // 避免高输出下 FIFO pop 错配。
                                if let Some(q) = self.pending_queries.pop_front() {
                                    self.pending_by_number.insert(b.number, q);
                                }
                                match self.pending_by_number.get(&b.number).cloned() {
                                    Some(PendingQuery::CapturePane { pane }) => {
                                        self.capture_response_seen.insert(pane);
                                    }
                                    Some(PendingQuery::PaneResyncCapture {
                                        pane,
                                        generation,
                                        ..
                                    }) => {
                                        if let Some(resync) = self.resyncs.get_mut(&pane) {
                                            if resync.initial && resync.generation == generation {
                                                resync.capture_started = true;
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            NotificationKind::End => {
                                let lines =
                                    self.response_accum.remove(&b.number).unwrap_or_default();
                                if self.truncated_responses.remove(&b.number) {
                                    tracing::warn!(
                                        target = "muxterm::tmux",
                                        number = b.number,
                                        "tmux response exceeded the bounded response buffer"
                                    );
                                    self.handle_response_error(b.number);
                                } else {
                                    self.dispatch_response(b.number, lines);
                                }
                            }
                            NotificationKind::Error => {
                                self.truncated_responses.remove(&b.number);
                                self.handle_response_error(b.number);
                            }
                        }
                    }
                    // 通知消息（WindowAdd / Output 等）先于对应的 %begin/%end 到达，
                    // 所以先 handle_message 处理通知，再在上面处理响应边界。
                    self.handle_message(msg);
                }
                TmuxEvent::ResponseBlock {
                    number,
                    lines,
                    truncated_prefix,
                    ..
                } => {
                    // reader 已经把 begin/end 之间的正文聚合成一个 block；
                    // 下一个 ResponseBoundary::End/Error 只负责提交这一块。
                    self.response_accum.insert(number, lines);
                    if truncated_prefix {
                        self.truncated_responses.insert(number);
                    }
                }
                TmuxEvent::OutputGap { pane } => {
                    // Output lane overflow is an explicit data-loss boundary. Do not
                    // try to replay a guessed suffix; schedule one bounded snapshot.
                    if let Some(receiver) = self.event_rx.as_mut() {
                        receiver.discard_output_pane(pane);
                    }
                    self.dropped_output_panes.insert(pane);
                }
                TmuxEvent::Exit { .. } => {
                    self.status = BackendStatus::Exited;
                    self.events
                        .push_back(StateChange::BackendStatusChanged(BackendStatus::Exited));
                }
            }
        }
        self.expire_resyncs();
        self.maybe_start_resyncs();
        if self.status == BackendStatus::Connected {
            self.flush_deferred_history_backfill();
            self.start_background_index_if_needed();
        }
    }

    /// attach 的后台可见屏索引放到 Connected 之后，并且排在活动 tab
    /// 的 `-S` 历史后面。connect 里排队会把进入 tmux 的第一下卡住。
    fn start_background_index_if_needed(&mut self) {
        if !self.background_index_capture_enabled || self.background_index_started {
            return;
        }
        if self.control_lane_busy() || !self.history_backfill_wanted.is_empty() {
            return;
        }
        self.background_index_started = true;
        self.query_background_index_captures();
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
                PendingQuery::PaneResyncState { pane, generation } => {
                    if self
                        .resyncs
                        .get(&pane)
                        .is_none_or(|resync| resync.generation != generation)
                    {
                        // Timeout/error may have released this generation while its
                        // response was still in the control lane. Never start a new
                        // capture transaction from a stale state response.
                        return;
                    }
                    if let Some(resync) = self.resyncs.get_mut(&pane) {
                        resync.state = lines.first().map(|line| parse_pane_replay_state(line));
                    }
                    // 只抓当前可见网格。`-S -10000` 会把控制流堵死、Surface
                    // 藏到超时；saved primary 也不再当 VT 流重放。
                    let capture = cmd::capture_pane_visible(pane);
                    if self.dispatch_tmux_command(&capture).is_ok() {
                        self.replace_last_pending(PendingQuery::PaneResyncCapture {
                            pane,
                            alternate: false,
                            generation,
                        });
                        self.release_attach_followup_if_ready();
                    } else {
                        self.abort_pane_resync(pane, "capture-command-failed");
                    }
                }
                PendingQuery::PaneResyncCapture {
                    pane, generation, ..
                } => {
                    let active_generation = self
                        .resyncs
                        .get(&pane)
                        .is_some_and(|resync| resync.generation == generation);
                    if !active_generation {
                        // A response from a timed-out generation must not paint a
                        // new snapshot or complete the newer transaction.
                        return;
                    }
                    if let Some(resync) = self.resyncs.get_mut(&pane) {
                        let data = capture_pane_grid_bytes(&lines);
                        let alternate_on = resync
                            .state
                            .as_ref()
                            .is_some_and(|state| state.alternate_on);
                        if alternate_on {
                            resync.alternate = Some(data);
                            resync.primary = Some(Vec::new());
                        } else {
                            resync.primary = Some(data);
                            resync.alternate = Some(Vec::new());
                        }
                    }
                    self.finish_pane_resync(pane);
                }
                PendingQuery::NewTabInCurrentDir {
                    pane: _,
                    session,
                    name,
                    command,
                } => {
                    // `display-message` returns the path as one response line;
                    // preserve spaces and pass it through tmux's C quoting.
                    let path = lines
                        .first()
                        .map(|line| line.trim())
                        .filter(|p| !p.is_empty());
                    let c = cmd::new_window_with_directory(
                        session,
                        name.as_deref(),
                        path,
                        command.as_deref(),
                    );
                    let _ = self.dispatch_tmux_command(&c);
                }
                PendingQuery::CapturePane { pane } => {
                    // capture-pane -p 按行返回当前可见屏幕。attach 必须按网
                    // 格地址铺，不能 trim 掉 TUI 底栏空行；非 attach 的索引
                    // dump 仍可裁尾。
                    let mut data = if self.is_attach_mode() {
                        capture_pane_surface_bytes(&lines)
                    } else {
                        capture_pane_bytes(&lines)
                    };
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
                        // 空屏也是权威快照：前端才能把 host 从 seeding 里放出来。
                        self.events.push_back(StateChange::PaneSnapshot {
                            pane,
                            data: snapshot,
                        });
                        self.trim_event_queue();
                        if self
                            .pane_tab(pane)
                            .is_some_and(|tab| self.tab_is_active(tab))
                        {
                            self.schedule_pane_history_backfill(pane);
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
                PendingQuery::PaneHistory { pane } => {
                    self.finish_pane_history_backfill(pane, lines);
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
        self.dropped_output_panes.remove(&pane);
        let primary = resync.primary.unwrap_or_default();
        let alternate = resync.alternate.unwrap_or_default();
        let initial = resync.initial;
        let pause_client = resync.pause_client;
        let live = if initial {
            resync.post_capture
        } else {
            resync.live
        };
        let mut snapshot = build_pane_snapshot(resync.state.as_ref(), &primary, &alternate, &live);
        if snapshot.len() > MAX_PANE_OUTPUT_BYTES {
            snapshot = snapshot[snapshot.len() - MAX_PANE_OUTPUT_BYTES..].to_vec();
        }
        if let Some(flow) = self.flow.get_mut(&pane) {
            flow.resyncing = false;
        }
        if initial {
            self.initial_capture_pending.remove(&pane);
            self.initial_capture_done.insert(pane);
            self.background_capture_only.remove(&pane);
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
        if let Some(receiver) = self.event_rx.as_mut() {
            receiver.resume_output_pane(pane);
        }
        // snapshot 入队后再 continue；其后的 tmux 输出会形成下一批增量。
        if pause_client {
            let _ = self.dispatch_tmux_command(&cmd::refresh_client_pause(pane, false));
        }
        if initial {
            self.schedule_pane_history_backfill(pane);
        }
        self.release_attach_followup_if_ready();
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
                PendingQuery::PaneResyncState { pane, generation }
                | PendingQuery::PaneResyncCapture {
                    pane, generation, ..
                } => {
                    tracing::warn!(
                        target: "muxterm::tmux::resync",
                        pane = pane.0,
                        number,
                        "pane snapshot query failed; releasing resync"
                    );
                    if self
                        .resyncs
                        .get(&pane)
                        .is_some_and(|resync| resync.generation == generation)
                    {
                        self.abort_pane_resync(pane, "response-error");
                    }
                }
                PendingQuery::NewTabInCurrentDir { pane, .. } => {
                    tracing::warn!(
                        target = "muxterm::tmux",
                        pane = pane.0,
                        number,
                        "current pane cwd query failed; new tab was not created"
                    );
                }
                PendingQuery::ReadyProbe { pane } => {
                    self.ready_probe_in_flight.remove(&pane);
                    self.ready_probe_acknowledged.remove(&pane);
                    self.ready_probe_at.insert(pane, Instant::now());
                }
                PendingQuery::PaneHistory { pane } => {
                    self.history_backfill_pending.remove(&pane);
                    // 失败就停，避免切 tab 反复抓 1 万行把控制通道打满。
                    self.history_backfill_done.insert(pane);
                    tracing::warn!(
                        target: "muxterm::tmux",
                        pane = pane.0,
                        number,
                        "pane history backfill failed"
                    );
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
        let mut size_changed = Vec::new();
        for np in &new_panes {
            let globally_active = tab_is_active && np.active;
            if let Some(existing) = self.panes.iter_mut().find(|p| p.id == np.id) {
                if existing.cols != np.cols || existing.rows != np.rows {
                    size_changed.push(np.id);
                }
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
            self.forget_pane_capture_grid(pid);
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
        for pane in size_changed {
            if self.resyncs.contains_key(&pane) {
                self.restart_snapshot_after_resize(pane);
            }
        }
        self.seed_listed_panes(tab_id, &new_panes);
    }

    /// attach 当中：活动 tab pause+抓可见屏。attach 完成之后：新 window /
    /// split 只抓可见屏，不要 pause——pause+display-message 会跟
    /// list-windows 抢 SSH 控制通道（dogfood 2026-08-25 新建第一张 tab 卡 4s）。
    fn seed_listed_panes(&mut self, tab_id: TabId, new_panes: &[PaneInfo]) {
        if self.background_index_capture_enabled {
            let active = self.tab_is_active(tab_id);
            for pane in new_panes {
                if active {
                    // 新建 pane 没有 attach 前历史，不要再发 `-S -10000`。
                    self.history_backfill_done.insert(pane.id);
                }
                self.query_capture_pane_visible(pane.id);
            }
            return;
        }
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
            let Some((tab, name, active, layout_str, panes_count, zoomed, index)) =
                parse_list_windows_line_with_index(line).or_else(|| {
                    parse_list_windows_line(line).map(
                        |(tab, name, active, layout, panes, zoomed)| {
                            (tab, name, active, layout, panes, zoomed, None)
                        },
                    )
                })
            else {
                tracing::warn!(target: "muxterm::tmux", "list-windows 行解析失败: {line}");
                continue;
            };
            let fallback_position = order.len();
            let sort_key = index
                .map(|index| (index as usize, fallback_position))
                .unwrap_or((usize::MAX, fallback_position));
            order.insert(tab, sort_key);
            if let Some(index) = index {
                self.window_indices.insert(tab, index);
            }
            let was_zoomed = self.window_zoomed.contains(&tab);
            let needs_panes = self.window_needs_pane_query(tab, &layout_str, panes_count);
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

            // 新建/关闭 tab 会把整表再拉一遍。layout 和 pane 数没变的
            // window 不要再 list-panes：SSH 上串行查一遍所有 window 会把
            // 新 tab 的首屏卡住数秒。
            if needs_panes {
                self.query_list_panes(tab);
            } else if was_zoomed != zoomed {
                let panes: Vec<PaneInfo> = self
                    .panes
                    .iter()
                    .filter(|pane| pane.tab == tab)
                    .cloned()
                    .collect();
                self.rebuild_layout(tab, &panes);
            }
        }
        // TabId 是稳定的 @window_id；用户拖动 tab 只会改变 tmux index，
        // 因此必须按 list-windows 的返回顺序重排，不能保留旧 Vec 顺序。
        self.tabs.sort_by_key(|tab| {
            order
                .get(&tab.id)
                .copied()
                .unwrap_or((usize::MAX, usize::MAX))
        });
        // 权威列表已到：裁决 move-window 等临时 unlink 产生的挂起 close。
        let confirmed_tabs: HashSet<TabId> = order.keys().copied().collect();
        self.settle_pending_close_tabs(&confirmed_tabs);
    }

    /// 权威 list-windows 到达时，只有拓扑真的变了才再 list-panes。
    fn window_needs_pane_query(&self, tab: TabId, layout_str: &str, panes_count: usize) -> bool {
        if !self.tabs.iter().any(|item| item.id == tab) {
            return true;
        }
        let known_panes = self.panes.iter().filter(|pane| pane.tab == tab).count();
        match self.window_layouts.get(&tab) {
            Some(known) if known == layout_str => {
                if self.expected_panes_per_window.get(&tab).copied() != Some(panes_count) {
                    return true;
                }
                known_panes != panes_count
            }
            Some(_) => true,
            // `%window-add` 已经 list-panes 过：第一次看到 layout 不必再查。
            None => known_panes != panes_count,
        }
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
        if self.resyncs.contains_key(&pane)
            || self.pending_queries.iter().any(|query| {
                matches!(
                    query,
                    PendingQuery::CapturePane { pane: pending }
                        | PendingQuery::PaneResyncState {
                            pane: pending, ..
                        }
                        | PendingQuery::PaneResyncCapture { pane: pending, .. }
                        if *pending == pane
                )
            })
            || self.initial_capture_done.contains(&pane)
        {
            return;
        }
        // W16a：attach 播种必须恢复 alternate/cursor/mouse，但只抓当前可见
        // 网格。把 `-S -N` 历史当 VT 流重放会卡住控制通道，htop/pi 也会乱码。
        self.begin_initial_pane_seed(pane);
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

    /// 后台索引用的轻量 capture：不读取 scrollback，避免 attach 后多个
    /// inactive tab 的响应挤占控制流。切入时不再升级为 pause seed。
    fn query_capture_pane_visible(&mut self, pane: PaneId) {
        if !self.is_attach_mode()
            || self.pending_queries.iter().any(|query| {
                matches!(query, PendingQuery::CapturePane { pane: pending } if *pending == pane)
            })
            || self.initial_capture_done.contains(&pane)
        {
            return;
        }
        let line = cmd::capture_pane_visible(pane).to_line();
        if self.dispatch_command(line).is_ok() {
            self.record_capture_grid(pane);
            self.initial_capture_buf.remove(&pane);
            self.initial_capture_tail.remove(&pane);
            self.capture_response_seen.remove(&pane);
            self.initial_capture_pending.insert(pane);
            self.background_capture_only.insert(pane);
            self.replace_last_pending(PendingQuery::CapturePane { pane });
        }
    }

    /// 可见屏已经种上之后，按行补 attach 前历史。不 pause，不 reset。
    /// 切 tab 当拍只登记，等控制通道空闲再发，避免和 select-window 抢 SSH。
    fn schedule_pane_history_backfill(&mut self, pane: PaneId) {
        if !self.is_attach_mode()
            || self.history_backfill_done.contains(&pane)
            || self.history_backfill_pending.contains(&pane)
        {
            return;
        }
        self.history_backfill_wanted.insert(pane);
        self.history_backfill_hold = true;
    }

    fn pane_tab(&self, pane: PaneId) -> Option<TabId> {
        self.panes
            .iter()
            .find(|item| item.id == pane)
            .map(|item| item.tab)
    }

    fn control_lane_busy(&self) -> bool {
        self.pending_queries.iter().any(Self::query_blocks_history)
            || self
                .pending_by_number
                .values()
                .any(Self::query_blocks_history)
            || !self.history_backfill_pending.is_empty()
            || !self.resyncs.is_empty()
    }

    fn query_blocks_history(query: &PendingQuery) -> bool {
        !matches!(query, PendingQuery::Ignore | PendingQuery::ListSessions)
    }

    /// 活动 tab 的 display-message 还没发出可见 capture。这时往通道里
    /// 塞 list-sessions / OSC 会把 SSH 上的首屏挤死。
    fn initial_seed_blocks_followup(&self) -> bool {
        self.resyncs
            .values()
            .any(|resync| resync.initial && resync.state.is_none())
    }

    fn release_attach_followup_if_ready(&mut self) {
        if self.attach_followup_flushed || self.initial_seed_blocks_followup() {
            return;
        }
        self.flush_attach_followup_commands();
    }

    fn flush_attach_followup_commands(&mut self) {
        if self.attach_followup_flushed {
            return;
        }
        self.attach_followup_flushed = true;
        self.attach_followup_held = false;
        self.query_list_sessions();
        self.setup_status_subscriptions();
        let colours = std::mem::take(&mut self.held_colour_reports);
        for (pane, fg, bg) in colours {
            let _ = self.dispatch_tmux_command(&cmd::refresh_client_colour(pane, 10, fg));
            let _ = self.dispatch_tmux_command(&cmd::refresh_client_colour(pane, 11, bg));
        }
    }

    fn flush_deferred_history_backfill(&mut self) {
        if self.history_backfill_hold || self.control_lane_busy() {
            return;
        }
        let Some(pane) = self.history_backfill_wanted.iter().copied().find(|pane| {
            !self.history_backfill_done.contains(pane)
                && !self.history_backfill_pending.contains(pane)
        }) else {
            return;
        };
        self.begin_pane_history_backfill(pane);
    }

    fn begin_pane_history_backfill(&mut self, pane: PaneId) {
        self.history_backfill_wanted.remove(&pane);
        if !self.is_attach_mode()
            || self.history_backfill_done.contains(&pane)
            || self.history_backfill_pending.contains(&pane)
            || self.pending_queries.iter().any(|query| {
                matches!(query, PendingQuery::PaneHistory { pane: pending } if *pending == pane)
            })
        {
            return;
        }
        let line = cmd::capture_pane_with_history(
            pane,
            super::pane_history::PaneHistoryPolicy::capture_lines(self.scrollback_lines),
        )
        .to_line();
        if self.dispatch_command(line).is_ok() {
            self.history_backfill_pending.insert(pane);
            self.replace_last_pending(PendingQuery::PaneHistory { pane });
        }
    }

    fn finish_pane_history_backfill(&mut self, pane: PaneId, lines: Vec<String>) {
        self.history_backfill_pending.remove(&pane);
        self.history_backfill_done.insert(pane);
        let data = super::pane_history::PaneHistoryPolicy::encode(&lines);
        if data.is_empty() {
            return;
        }
        self.events
            .push_back(StateChange::PaneHistory { pane, data });
        self.trim_event_queue();
    }

    /// 发送 list-sessions 查询（列出 tmux server 上所有 session）。
    fn query_list_sessions(&mut self) {
        if self.initial_seed_blocks_followup() {
            self.attach_followup_held = true;
            return;
        }
        let line = "list-sessions\n".to_string();
        if self.dispatch_command(line).is_ok() {
            self.replace_last_pending(PendingQuery::ListSessions);
        }
    }

    /// 探测 tmux 是否支持 `refresh-client -r`（颜色上报）：不支持时静默跳过，
    /// 避免老 tmux 每上报一次就打一条 `unknown flag -r` 错误。
    ///
    /// 不在 `connect()` 里调用：SSH attach 等于多一次 `tmux -V` 往返。
    #[allow(dead_code)]
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
        if self.initial_seed_blocks_followup() {
            self.attach_followup_held = true;
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
            "list-windows -t {} -F \"#{{window_id}},#{{window_name}},#{{window_active}},#{{window_layout}},#{{window_panes}},#{{window_zoomed_flag}},#{{window_index}}\"\n",
            sess
        );

        if self.dispatch_command(line).is_ok() {
            self.replace_last_pending(PendingQuery::ListWindows);
            return true;
        }
        false
    }

    /// attach 的控制 client 默认可能先以 80x24 建立。先把已知的窗口尺寸
    /// 写入 client，再发 list-windows/list-panes/capture，避免 TUI 首屏按旧
    /// 网格换行，随后 resize 时输入框被遮住或错位。
    fn refresh_initial_client_size(&mut self) {
        let (Some(cols), Some(rows)) = (self.config.cols, self.config.rows) else {
            return;
        };
        let command = cmd::refresh_client_size(cols, rows);
        if self.dispatch_tmux_command(&command).is_ok() {
            tracing::debug!(
                target: "muxterm::tmux::seed",
                cols,
                rows,
                "attach 首屏先同步 control client 尺寸"
            );
        }
    }

    fn emit_layout_if_changed(&mut self, layout: TabLayout) {
        if self.layouts.get(&layout.tab) == Some(&layout) {
            return;
        }
        self.layouts.insert(layout.tab, layout.clone());
        self.push_layout_changed(layout);
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
            self.emit_layout_if_changed(TabLayout {
                tab: tab_id,
                tree: LayoutNode::leaf(active),
                active,
            });
            return;
        }
        if panes.len() == 1 {
            self.emit_layout_if_changed(TabLayout {
                tab: tab_id,
                tree: LayoutNode::leaf(panes[0].id),
                active,
            });
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
        self.emit_layout_if_changed(TabLayout {
            tab: tab_id,
            tree: layout_node,
            active,
        });
    }

    /// 朴素兜底布局：按顺序水平排列 pane。
    fn build_fallback_layout(&mut self, tab_id: TabId, panes: &[PaneInfo], active: PaneId) {
        let mut sorted: Vec<PaneInfo> = panes.to_vec();
        sorted.sort_by_key(|p| (p.cols, p.id.0));
        let mut tree = LayoutNode::leaf(sorted[0].id);
        for p in &sorted[1..] {
            tree.split_at(sorted[0].id, p.id, SplitDir::Horizontal);
        }
        self.emit_layout_if_changed(TabLayout {
            tab: tab_id,
            tree,
            active,
        });
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

        // attach 的 control client 可能已经先按默认 80x24 建立；在任何
        // capture 之前先同步配置尺寸，避免 Pi/OMP/Cursor/htop 首屏按旧列数
        // 换行。new-session 的 -x/-y 已在 spawn 参数中设置，这条命令同样
        // 兼容两种模式且不会改变产品层 ABI。
        if is_attach {
            self.refresh_initial_client_size();
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
        // 第一次 list-windows 会对每个还没有 pane 快照的 window 发
        // list-panes。连接只需要活动 tab 的拓扑即可交给前端；其它 tab
        // 的 pane 列表继续在后台响应，不能让一个慢 pane 把 Connect
        // 卡住数秒。之后 layout 没变就不再查。
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

        // active tab 的 capture 响应通常紧随 pane 拓扑到达，但不能假设
        // `connect()` 返回时控制流已经处理完它们：调用方可能只在连接后
        // 轮询一次事件。给活动 tab 一个很短的 bounded settle，确保 Core
        // 首屏缓冲可用；后台 tab 的 capture 不在此等待。
        if is_attach && self.active_tab_topology_ready() {
            let settle_deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
            while !self.active_tab_capture_ready() && std::time::Instant::now() < settle_deadline {
                self.pump_events();
                if self.active_tab_capture_ready() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }

        // attach 的 capture 是异步 Surface seed：连接状态不再等待所有 pane
        // 的历史返回。活动 tab 的查询已经排队，前端在收到 PaneSnapshot 后
        // 播种；其它 tab 只做轻量可见屏索引，而且要等 Connected 之后的
        // 第一拍（先发活动 tab 历史，再索引后台）。
        if is_attach {
            self.background_index_capture_enabled = true;
            tracing::info!(
                target: "muxterm::tmux::seed",
                active_tab = self.active_tab_id().map(|tab| tab.0),
                pending = self.initial_capture_pending.len(),
                "attach 首屏 capture 已异步排队"
            );
        }
        self.attach_bootstrap_complete = is_attach;

        // 不要在 connect 里再跑 `tmux -V`（SSH 等于多一次往返）。
        // 版本未知时颜色上报默认开；status 订阅直接尝试，老 tmux 的
        // unknown flag 走 Ignore 槽，不会卡住控制通道。
        self.status_subscription_supported = true;
        if is_attach && self.initial_seed_blocks_followup() {
            // display-message 还在路上：list-sessions / -B 放到可见
            // capture-pane 发出之后。1612 就是在 pause 之后立刻灌这些
            // 命令，9s 都没抓到首屏。
            self.attach_followup_held = true;
        } else {
            self.flush_attach_followup_commands();
        }

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
                if self.initial_seed_blocks_followup() {
                    self.attach_followup_held = true;
                    self.held_colour_reports.push((*target, *fg, *bg));
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
            Task::NewTab {
                name,
                command,
                workdir,
            } => {
                // tmux 的 tab = tmux window，新建 tab = 新建 tmux window
                let Some(sess) = self.active_session else {
                    return Ok(TaskOutcome::Rejected {
                        reason: "tmux 未连接".into(),
                    });
                };
                if let Some(dir) = workdir {
                    let c = cmd::new_window_with_directory(
                        sess,
                        name.as_deref(),
                        Some(dir),
                        command.as_deref(),
                    );
                    if self.dispatch_tmux_command(&c).is_err() {
                        return Ok(TaskOutcome::Rejected {
                            reason: "发送命令失败".into(),
                        });
                    }
                } else if let Some(pane) = self.active_pane().map(|pane| pane.id) {
                    // New tabs inherit the active pane's cwd. Querying tmux is
                    // authoritative even when the GUI's project path is `~` or
                    // the shell has changed directory since attach.
                    let q = cmd::display_message(pane, "#{pane_current_path}");
                    if self.dispatch_tmux_command(&q).is_err() {
                        return Ok(TaskOutcome::Rejected {
                            reason: "发送命令失败".into(),
                        });
                    }
                    self.replace_last_pending(PendingQuery::NewTabInCurrentDir {
                        pane,
                        session: sess,
                        name: name.clone(),
                        command: command.clone(),
                    });
                } else {
                    let c = cmd::new_window_with_directory(
                        sess,
                        name.as_deref(),
                        None,
                        command.as_deref(),
                    );
                    if self.dispatch_tmux_command(&c).is_err() {
                        return Ok(TaskOutcome::Rejected {
                            reason: "发送命令失败".into(),
                        });
                    }
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
                    self.latest_switch_target = None;
                    return Ok(TaskOutcome::Rejected {
                        reason: "发送命令失败".into(),
                    });
                }
                // 乐观更新 active tab：tmux 在输出洪峰下可能延迟回
                // %session-window-changed，前端等太久会以为切 tab 不生效。
                // 真正的通知到达后 mark_tab_active 幂等，不会重复切换。
                self.latest_switch_target = Some(*target);
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
        self.events.drain(..).collect()
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
/// 格式：`@N,name,active,LAYOUT,panes,zoomed[,index]`。
/// LAYOUT 含逗号，因此前三个字段用 `split_once`，尾部字段从右侧解析。
type ParsedWindowLineWithIndex = (TabId, String, bool, String, usize, bool, Option<u32>);

fn parse_list_windows_line_with_index(line: &str) -> Option<ParsedWindowLineWithIndex> {
    let (id_str, rest) = line.split_once(',')?;
    let (name, rest) = rest.split_once(',')?;
    let (active_str, rest) = rest.split_once(',')?;
    let tab = TabId::parse(id_str).ok()?;
    let active = active_str == "1";

    let mut tail = rest.rsplitn(4, ',');
    let index = tail.next()?.parse::<u32>().ok()?;
    let zoomed_str = tail.next()?;
    let panes_str = tail.next()?;
    let layout_str = tail.next()?;
    let panes_count = panes_str.parse().ok()?;
    let zoomed = zoomed_str == "1";
    Some((
        tab,
        name.to_string(),
        active,
        layout_str.to_string(),
        panes_count,
        zoomed,
        Some(index),
    ))
}

/// 兼容旧测试样例/调用方的无 index 解析结果。
fn parse_list_windows_line(line: &str) -> Option<(TabId, String, bool, String, usize, bool)> {
    let (id_str, rest) = line.split_once(',')?;
    let (name, rest) = rest.split_once(',')?;
    let (active_str, rest) = rest.split_once(',')?;
    let (layout_and_panes, zoomed_str) = rest.rsplit_once(',')?;
    let (layout_str, panes_str) = layout_and_panes.rsplit_once(',')?;
    let tab = TabId::parse(id_str).ok()?;
    let panes_count = panes_str.parse().ok()?;
    Some((
        tab,
        name.to_string(),
        active_str == "1",
        layout_str.to_string(),
        panes_count,
        zoomed_str == "1",
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
    fn parse_list_windows_line_reads_real_window_index() {
        let line = "@7,codex,1,bbcd,80x24,0,0,1,1,1,6";
        let (tab, name, active, layout, panes, zoomed, index) =
            parse_list_windows_line_with_index(line).unwrap();
        assert_eq!(tab, TabId(7));
        assert_eq!(name, "codex");
        assert!(active);
        assert_eq!(layout, "bbcd,80x24,0,0,1");
        assert_eq!(panes, 1);
        assert!(zoomed);
        assert_eq!(index, Some(6));
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
    fn attach_seed_capture_must_use_visible_grid() {
        let src = include_str!("backend.rs");
        let visible = concat!("cmd", "::", "capture_pane_visible");
        assert!(
            src.contains(visible),
            "query_capture_pane / seed 必须调用 cmd::capture_pane_visible"
        );
        let history = concat!("capture_pane", "_with_history");
        assert!(
            src.contains("begin_pane_history_backfill"),
            "可见屏 seed 之后必须另开按行历史回填"
        );
        assert!(
            src.contains(history),
            "历史回填必须走 cmd::capture_pane_with_history，不能把 -S 写进首屏 seed"
        );
        assert!(
            src.contains(r#"begin_pane_snapshot(pane, "initial-seed", true, true)"#),
            "attach 首屏必须 pause 控制输出（第四参 true），否则 TUI 会在抓屏期间把 output lane 打满"
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
    fn attach_output_without_pending_capture_is_kept_for_background_index() {
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        let pane = PaneId(9);
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
        b.panes.push(PaneInfo {
            id: pane,
            tab: TabId(2),
            active: true,
            title: String::new(),
            cols: 80,
            rows: 24,
        });
        b.handle_message(Message::Output {
            pane,
            content: b"background-token\r\n".to_vec(),
            raw_content: "background-token\\r\\n".into(),
        });

        assert_eq!(
            b.outputs.get(&pane),
            Some(&b"background-token\r\n".to_vec())
        );
        assert!(b.events.iter().any(
            |event| matches!(event, StateChange::PaneOutput { pane: p, data } if *p == pane && data == b"background-token\r\n")
        ));
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
            b"\x1b[H\x1b[1Hsamesame\r\n",
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
        assert_eq!(
            b.outputs.get(&pane).unwrap(),
            b"\x1b[H\x1b[1Hold command\x1b[2Hprompt$ "
        );

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

        // tmux 屏幕：prompt 在第 3 行（1 基），下方全是空白行。按行 CUP
        // 铺网格后光标停在 ❯，不能 trim 成三行 \r\n dump。
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
        let snap = b.outputs.get(&pane).unwrap();
        let text = String::from_utf8_lossy(snap);
        assert!(
            snap.starts_with(b"\x1b[H"),
            "attach 索引快照必须从 home 铺格子: {text:?}"
        );
        assert!(
            text.contains("\u{1b}[3H❯"),
            "prompt 必须还在第 3 行，不能被尾部空行推到 pane 底: {text:?}"
        );
        assert!(
            !text.contains("\r\n"),
            "索引用的 Surface seed 不得靠 \\r\\n 堆行，否则 TUI 网格会塌: {text:?}"
        );
    }

    #[test]
    fn attach_index_snapshot_keeps_tui_top_and_input_box() {
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        let pane = PaneId(12);
        b.pending_by_number
            .insert(1, PendingQuery::CapturePane { pane });
        b.dispatch_response(
            1,
            vec![
                "PI_STATUS".into(),
                String::new(),
                "CONV_MIDDLE".into(),
                String::new(),
                "PROMPT>".into(),
                String::new(),
            ],
        );
        let snap = b.outputs.get(&pane).unwrap();
        let text = String::from_utf8_lossy(snap);
        assert!(
            text.contains("\u{1b}[1HPI_STATUS"),
            "TUI 顶栏必须在第 1 行: {text:?}"
        );
        assert!(
            text.contains("\u{1b}[5HPROMPT>"),
            "输入盒必须在原来的行，不能被 trim 后挤到中间: {text:?}"
        );
        assert!(text.contains("CONV_MIDDLE"), "中间内容也要在: {text:?}");
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

        assert_eq!(b.outputs.get(&pane).unwrap(), b"\x1b[H\x1b[1Hrestored");
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
            b"\x1b[H\x1b[1Hscreen linelive-during-capture\r\n"
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
        assert!(
            data.windows(14).any(|w| w == b"SNAPSHOT_TOKEN"),
            "快照正文必须在: {:?}",
            String::from_utf8_lossy(data)
        );
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
        assert!(
            bytes.starts_with(b"\x1b[?1049hcursor frame"),
            "alt-screen 首屏不得先重放 saved primary，实际 {:?}",
            String::from_utf8_lossy(&bytes[..bytes.len().min(64)])
        );
        assert!(
            !bytes
                .windows(b"primary\r\n".len())
                .any(|w| w == b"primary\r\n"),
            "htop/pi 乱码就是把 shell 历史灌进 1049h 之前"
        );
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
        b.pending_by_number.insert(
            1,
            PendingQuery::PaneResyncState {
                pane,
                generation: 1,
            },
        );
        b.dispatch_response(
            1,
            vec!["2|1|1|block|0|1|0|0|0|1|0|0|0|0|0|0|0|0|0|1".into()],
        );
        b.pending_by_number.insert(
            2,
            PendingQuery::PaneResyncCapture {
                pane,
                alternate: false,
                generation: 1,
            },
        );
        b.dispatch_response(2, vec!["tui-grid".into()]);

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
        assert!(
            snapshots[0]
                .windows(b"\x1b[?1049h".len())
                .any(|w| w == b"\x1b[?1049h"),
            "alt-screen 必须进 1049h"
        );
        assert!(snapshots[0]
            .windows(b"tui-grid".len())
            .any(|w| w == b"tui-grid"));
        assert!(snapshots[0]
            .windows(b"live-after-pause".len())
            .any(|w| w == b"live-after-pause"));
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
    fn normal_cup_burst_does_not_start_resync_by_time_or_byte_rate() {
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new(None);
        b.cmd_tx = Some(tx);
        let pane = PaneId(31);
        for _ in 0..256 {
            b.note_pane_output(pane, b"\x1b[H\x1b[2Jframe\r\n");
        }
        b.maybe_start_resyncs();
        assert!(
            !b.resyncs.contains_key(&pane),
            "正常 OMP/CUP burst 不应因 64KB/250ms 阈值重拍"
        );
    }

    #[test]
    fn dropped_pane_output_starts_authoritative_resync() {
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new(None);
        b.cmd_tx = Some(tx);
        let pane = PaneId(32);
        b.dropped_output_panes.insert(pane);
        b.maybe_start_resyncs();
        assert!(b.resyncs.contains_key(&pane));
        assert!(b.pending_queries.iter().any(
            |query| matches!(query, PendingQuery::PaneResyncState { pane: p, .. } if *p == pane)
        ));
    }

    #[test]
    fn dropped_output_does_not_resync_already_seeded_surface() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new(None);
        b.cmd_tx = Some(tx);
        let pane = PaneId(32);
        b.initial_capture_done.insert(pane);
        b.dropped_output_panes.insert(pane);
        b.maybe_start_resyncs();
        assert!(
            !b.resyncs.contains_key(&pane),
            "已经 seed 的 Surface 丢字节不得 pause+capture"
        );
        let cmds = drain_tmux_cmds(&mut rx);
        assert!(
            !cmds
                .iter()
                .any(|cmd| cmd.contains("pause") || cmd.contains("capture-pane")),
            "output-dropped 不得再抓已打开的 Surface: {cmds:?}"
        );
    }

    fn drain_tmux_cmds(rx: &mut mpsc::UnboundedReceiver<String>) -> Vec<String> {
        let mut cmds = Vec::new();
        while let Ok(cmd) = rx.try_recv() {
            cmds.push(cmd);
        }
        cmds
    }

    #[test]
    fn initial_seed_pauses_and_caps_capture_history() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        b.cmd_tx = Some(tx);
        b.set_scrollback_lines(10_000);
        let pane = PaneId(2);
        b.begin_initial_pane_seed(pane);

        let cmds = drain_tmux_cmds(&mut rx);
        assert!(
            cmds.iter().any(|cmd| cmd.contains(r#"%2:pause"#)),
            "attach seed 必须 pause 控制输出，否则 Codex 会在抓屏期间继续灌 %output: {cmds:?}"
        );
        assert!(b
            .resyncs
            .get(&pane)
            .is_some_and(|resync| resync.pause_client));

        b.pending_by_number.insert(
            1,
            PendingQuery::PaneResyncState {
                pane,
                generation: 1,
            },
        );
        b.dispatch_response(1, vec!["0|0|1|||1|||||||||||||||".into()]);
        let cmds = drain_tmux_cmds(&mut rx);
        assert!(
            cmds.iter()
                .any(|cmd| cmd.contains("capture-pane -e -p -N -t %2")),
            "seed 必须只抓可见网格并保留行尾空格: {cmds:?}"
        );
        assert!(
            !cmds.iter().any(|cmd| cmd.contains("-S ")),
            "不得再抓 scrollback 把 seed 拖过 deadline: {cmds:?}"
        );
        assert_eq!(
            cmds.iter()
                .filter(|cmd| cmd.contains("capture-pane"))
                .count(),
            1,
            "一轮 seed 只抓当前屏，不要再发 -a 历史: {cmds:?}"
        );
    }

    #[test]
    fn attach_followup_waits_until_visible_capture_is_sent() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        b.cmd_tx = Some(tx);
        b.status = BackendStatus::Connected;
        b.status_subscription_supported = true;
        let pane = PaneId(79);
        b.panes.push(PaneInfo {
            id: pane,
            tab: TabId(34),
            active: true,
            title: String::new(),
            cols: 80,
            rows: 24,
        });
        b.begin_initial_pane_seed(pane);
        let _ = drain_tmux_cmds(&mut rx);

        b.query_list_sessions();
        b.setup_status_subscriptions();
        let _ = b.execute(&crate::core::model::task::Task::ReportPaneColours {
            target: pane,
            fg: crate::core::config::Rgb(0, 0, 0),
            bg: crate::core::config::Rgb(255, 255, 255),
        });
        let held = drain_tmux_cmds(&mut rx);
        assert!(
            !held.iter().any(|cmd| cmd.contains("list-sessions")
                || cmd.contains("refresh-client -B")
                || cmd.contains("refresh-client -r")),
            "display-message 回来前不得灌 list-sessions/OSC: {held:?}"
        );
        assert!(b.initial_seed_blocks_followup());
        assert!(b.attach_followup_held);
        assert_eq!(b.held_colour_reports.len(), 1);

        b.pending_by_number.insert(
            1,
            PendingQuery::PaneResyncState {
                pane,
                generation: 1,
            },
        );
        b.dispatch_response(1, vec!["0|0|1|||1|||||||||||||||".into()]);
        let after = drain_tmux_cmds(&mut rx);
        let capture_at = after
            .iter()
            .position(|cmd| cmd.contains("capture-pane -e -p -N -t %79"))
            .expect("必须先发可见 capture");
        assert!(
            after.iter().any(|cmd| cmd.contains("list-sessions")),
            "可见 capture 发出后才能补 list-sessions: {after:?}"
        );
        let list_at = after
            .iter()
            .position(|cmd| cmd.contains("list-sessions"))
            .unwrap();
        assert!(
            capture_at < list_at,
            "list-sessions 必须排在可见 capture 后面: {after:?}"
        );
        assert!(
            after.iter().any(|cmd| cmd.contains("refresh-client -r")),
            "排队的 OSC 也要在 capture 之后发出: {after:?}"
        );
        assert!(!b.initial_seed_blocks_followup());
        assert!(!b.attach_followup_held);
        assert!(b.held_colour_reports.is_empty());
    }

    #[test]
    fn empty_initial_seed_deadline_does_not_grab_history() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        b.cmd_tx = Some(tx);
        let pane = PaneId(2);
        b.begin_initial_pane_seed(pane);
        let _ = drain_tmux_cmds(&mut rx);
        b.resyncs.get_mut(&pane).unwrap().deadline =
            Some(Instant::now() - Duration::from_millis(1));
        b.expire_resyncs();
        let cmds = drain_tmux_cmds(&mut rx);
        assert!(
            !cmds.iter().any(|cmd| cmd.contains("-S ")),
            "空超时不得再抓历史，否则 1612 会把通道卡死: {cmds:?}"
        );
        assert!(
            !b.history_backfill_wanted.contains(&pane),
            "空 snapshot 不是可滚的历史"
        );
        assert!(b.initial_capture_done.contains(&pane));
    }

    #[test]
    fn attach_dropped_output_does_not_pause_background_panes() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        b.cmd_tx = Some(tx);
        b.begin_initial_pane_seed(PaneId(79));
        let _ = drain_tmux_cmds(&mut rx);
        b.dropped_output_panes.insert(PaneId(0));
        b.dropped_output_panes.insert(PaneId(116));
        b.maybe_start_resyncs();
        let cmds = drain_tmux_cmds(&mut rx);
        assert!(
            !cmds
                .iter()
                .any(|cmd| cmd.contains("%0:pause") || cmd.contains("%116:pause")),
            "attach 首屏期间后台 pane 丢字节不得再 pause: {cmds:?}"
        );
        assert!(!b.resyncs.contains_key(&PaneId(0)));
        assert!(!b.resyncs.contains_key(&PaneId(116)));
    }

    #[test]
    fn history_backfill_caps_ssh_chunk_not_configured_10000() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        b.cmd_tx = Some(tx);
        b.set_scrollback_lines(10_000);
        let pane = PaneId(4);
        b.history_backfill_wanted.insert(pane);
        b.history_backfill_hold = false;
        b.flush_deferred_history_backfill();
        let cmds = drain_tmux_cmds(&mut rx);
        assert!(
            cmds.iter()
                .any(|cmd| cmd.contains("-S -256") && cmd.contains("-E -1")),
            "单次历史必须封顶 256 行: {cmds:?}"
        );
        assert!(
            !cmds.iter().any(|cmd| cmd.contains("-S -10000")),
            "配置 10000 是容量，不能一次经 SSH 拉完: {cmds:?}"
        );
    }

    #[test]
    fn output_dropped_resync_caps_capture_history() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new(None);
        b.cmd_tx = Some(tx);
        b.set_scrollback_lines(10_000);
        let pane = PaneId(24);
        b.dropped_output_panes.insert(pane);
        b.maybe_start_resyncs();
        let _ = drain_tmux_cmds(&mut rx);

        b.pending_by_number.insert(
            1,
            PendingQuery::PaneResyncState {
                pane,
                generation: 1,
            },
        );
        b.dispatch_response(1, vec!["0|0|1|||0|||||||||||||||".into()]);
        let cmds = drain_tmux_cmds(&mut rx);
        assert!(
            cmds.iter()
                .any(|cmd| cmd.contains("capture-pane -e -p -N -t %24")),
            "output-dropped 恢复也必须只抓可见屏: {cmds:?}"
        );
        assert!(
            !cmds.iter().any(|cmd| cmd.contains("-S ")),
            "output-dropped 再抓历史就是卡死重绘循环: {cmds:?}"
        );
    }

    #[test]
    fn tab_switch_does_not_pause_already_indexed_pane() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        b.cmd_tx = Some(tx);
        b.background_index_capture_enabled = true;
        let pane = PaneId(1);
        b.tabs = vec![
            TabInfo {
                id: TabId(1),
                name: "active".into(),
                active: true,
            },
            TabInfo {
                id: TabId(2),
                name: "bg".into(),
                active: false,
            },
        ];
        b.panes.push(PaneInfo {
            id: pane,
            tab: TabId(2),
            active: true,
            title: String::new(),
            cols: 80,
            rows: 24,
        });
        b.background_capture_only.insert(pane);
        b.initial_capture_done.insert(pane);

        b.mark_tab_active(TabId(2));
        b.query_capture_tab(TabId(2));
        let cmds = drain_tmux_cmds(&mut rx);
        assert!(
            !cmds.iter().any(|cmd| cmd.contains("pause")),
            "已有可见屏索引的 tab 切过去不得再 pause: {cmds:?}"
        );
        assert!(
            !cmds
                .iter()
                .any(|cmd| cmd.contains("capture-pane -e -p -N -t") && !cmd.contains("-S ")),
            "切 tab 不得再抓可见屏: {cmds:?}"
        );
        assert!(
            !cmds.iter().any(|cmd| cmd.contains("-S -")),
            "切 tab 当拍不得抓 1 万行历史: {cmds:?}"
        );
        assert!(b.history_backfill_wanted.contains(&pane));
        assert!(b.initial_capture_done.contains(&pane));
        assert!(!b.background_capture_only.contains(&pane));
        assert!(!b.resyncs.contains_key(&pane));

        b.history_backfill_hold = false;
        b.flush_deferred_history_backfill();
        let history = drain_tmux_cmds(&mut rx);
        assert!(
            history
                .iter()
                .any(|cmd| cmd.contains("-S -") && cmd.contains("-E -1")),
            "控制通道空闲后才按行补历史: {history:?}"
        );
        assert!(b.history_backfill_pending.contains(&pane));

        b.history_backfill_done.insert(pane);
        b.history_backfill_pending.remove(&pane);
        b.history_backfill_wanted.remove(&pane);
        b.mark_tab_active(TabId(1));
        b.mark_tab_active(TabId(2));
        b.query_capture_tab(TabId(2));
        let again = drain_tmux_cmds(&mut rx);
        assert!(
            !again
                .iter()
                .any(|cmd| cmd.contains("capture-pane") || cmd.contains("pause")),
            "历史已经补过的 tab 再切回来不得再抓: {again:?}"
        );
    }

    #[test]
    fn tab_switch_recaptures_when_capture_grid_stale() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        b.cmd_tx = Some(tx);
        b.background_index_capture_enabled = true;
        let pane = PaneId(1);
        b.tabs = vec![
            TabInfo {
                id: TabId(1),
                name: "active".into(),
                active: true,
            },
            TabInfo {
                id: TabId(2),
                name: "bg".into(),
                active: false,
            },
        ];
        b.panes.push(PaneInfo {
            id: pane,
            tab: TabId(2),
            active: true,
            title: String::new(),
            cols: 125,
            rows: 25,
        });
        b.background_capture_only.insert(pane);
        b.initial_capture_done.insert(pane);
        b.capture_grid.insert(pane, (94, 25));

        b.mark_tab_active(TabId(2));
        b.query_capture_tab(TabId(2));
        let cmds = drain_tmux_cmds(&mut rx);
        assert!(
            cmds.iter().any(|cmd| cmd.contains("pause")),
            "后台索引网格已经过期，切 tab 必须 pause 再抓: {cmds:?}"
        );
        assert!(
            !b.initial_capture_done.contains(&pane),
            "过期索引必须作废，不能当成 initial_capture_done"
        );
        assert!(
            !b.background_capture_only.contains(&pane),
            "过期的 background_capture_only 不得直接 promote"
        );
        assert!(b.resyncs.contains_key(&pane), "必须进入 pause seed");
        assert_eq!(
            b.capture_grid.get(&pane).copied(),
            Some((125, 25)),
            "再抓必须记下当前网格，不能用事后尺寸或旧 94 列"
        );
    }

    #[test]
    fn tab_switch_does_not_recapture_open_surface_after_resize() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        b.cmd_tx = Some(tx);
        let pane = PaneId(1);
        b.tabs = vec![
            TabInfo {
                id: TabId(1),
                name: "active".into(),
                active: true,
            },
            TabInfo {
                id: TabId(2),
                name: "open".into(),
                active: false,
            },
        ];
        b.panes.push(PaneInfo {
            id: pane,
            tab: TabId(2),
            active: true,
            title: String::new(),
            cols: 125,
            rows: 25,
        });
        b.initial_capture_done.insert(pane);
        b.history_backfill_done.insert(pane);
        b.capture_grid.insert(pane, (94, 25));

        b.mark_tab_active(TabId(2));
        b.query_capture_tab(TabId(2));
        let cmds = drain_tmux_cmds(&mut rx);

        assert!(
            !cmds
                .iter()
                .any(|cmd| cmd.contains("pause") || cmd.contains("capture-pane")),
            "已经打开的 Surface resize 后切回只能复用 live VT，不得重抓: {cmds:?}"
        );
        assert!(b.initial_capture_done.contains(&pane));
        assert!(!b.resyncs.contains_key(&pane));
    }

    #[test]
    fn tab_switch_skips_seed_when_only_initial_capture_done() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        b.cmd_tx = Some(tx);
        let pane = PaneId(1);
        b.tabs = vec![
            TabInfo {
                id: TabId(1),
                name: "active".into(),
                active: true,
            },
            TabInfo {
                id: TabId(2),
                name: "bg".into(),
                active: false,
            },
        ];
        b.panes.push(PaneInfo {
            id: pane,
            tab: TabId(2),
            active: true,
            title: String::new(),
            cols: 80,
            rows: 24,
        });
        b.initial_capture_done.insert(pane);

        b.mark_tab_active(TabId(2));
        b.query_capture_tab(TabId(2));
        let first = drain_tmux_cmds(&mut rx);
        assert!(
            !first.iter().any(|cmd| cmd.contains("pause")),
            "initial_capture_done 的 pane 切 tab 不得 pause: {first:?}"
        );
        assert!(
            !first.iter().any(|cmd| cmd.contains("-S -")),
            "切 tab 当拍不得抓历史: {first:?}"
        );
        b.history_backfill_hold = false;
        b.flush_deferred_history_backfill();
        let history = drain_tmux_cmds(&mut rx);
        assert!(
            history
                .iter()
                .any(|cmd| cmd.contains("-S -") && cmd.contains("-E -1")),
            "空闲后必须补历史行: {history:?}"
        );

        b.history_backfill_done.insert(pane);
        b.history_backfill_pending.remove(&pane);
        b.history_backfill_wanted.remove(&pane);
        b.query_capture_tab(TabId(2));
        let cmds = drain_tmux_cmds(&mut rx);
        assert!(
            cmds.is_empty(),
            "历史回填完成后切 tab 不得再发控制命令: {cmds:?}"
        );
        assert!(!b.resyncs.contains_key(&pane));
    }

    #[test]
    fn history_backfill_emits_lines_not_vt_snapshot() {
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        let pane = PaneId(3);
        b.pending_by_number
            .insert(1, PendingQuery::PaneHistory { pane });
        b.history_backfill_pending.insert(pane);
        b.dispatch_response(
            1,
            vec!["\u{1b}[32mHIST_OFFSCREEN\u{1b}[0m".into(), "pad-01".into()],
        );
        assert!(b.history_backfill_done.contains(&pane));
        assert!(!b.history_backfill_pending.contains(&pane));
        let Some(StateChange::PaneHistory { pane: p, data }) = b.events.back() else {
            panic!("expected PaneHistory, got {:?}", b.events.back());
        };
        assert_eq!(*p, pane);
        let text = String::from_utf8_lossy(data);
        assert!(text.contains("HIST_OFFSCREEN"), "{text}");
        assert!(!text.contains('\u{1b}'), "历史事件不得带 SGR: {text}");
        assert!(!text.contains("[2J"));
        assert!(
            !b.events.iter().any(
                |event| matches!(event, StateChange::PaneSnapshot { pane: sp, .. } if *sp == pane)
            ),
            "历史不得再发一份 PaneSnapshot"
        );
    }

    #[test]
    fn empty_visible_capture_still_emits_snapshot() {
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        let pane = PaneId(8);
        b.pending_by_number
            .insert(1, PendingQuery::CapturePane { pane });
        b.initial_capture_pending.insert(pane);
        b.dispatch_response(1, Vec::new());
        assert!(b.initial_capture_done.contains(&pane));
        assert!(
            b.events.iter().any(|event| matches!(
                event,
                StateChange::PaneSnapshot { pane: p, data } if *p == pane && data.is_empty()
            )),
            "空屏也要发 PaneSnapshot，否则 host 会一直藏着: {:?}",
            b.events.back()
        );
    }

    #[test]
    fn finish_initial_seed_queues_history_after_continue() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        b.cmd_tx = Some(tx);
        let pane = PaneId(4);
        b.resyncs.insert(
            pane,
            PaneResync {
                generation: 1,
                initial: true,
                pause_client: true,
                primary: Some(b"prompt$\r\n".to_vec()),
                ..PaneResync::default()
            },
        );
        b.paused_panes.insert(pane);
        b.finish_pane_resync(pane);
        let cmds = drain_tmux_cmds(&mut rx);
        assert!(
            cmds.iter().any(|cmd| cmd.contains("%4:continue")),
            "seed 完成后必须 continue: {cmds:?}"
        );
        assert!(
            !cmds.iter().any(|cmd| cmd.contains("-S -")),
            "continue 当拍不得抓历史，否则首屏要等 1 万行: {cmds:?}"
        );
        assert!(b.history_backfill_wanted.contains(&pane));
        b.history_backfill_hold = false;
        b.flush_deferred_history_backfill();
        let history = drain_tmux_cmds(&mut rx);
        assert!(
            history
                .iter()
                .any(|cmd| cmd.contains("-S -") && cmd.contains("-E -1")),
            "空闲后才按行抓历史: {history:?}"
        );
        assert!(
            !history.iter().any(|cmd| cmd.contains(":pause")),
            "历史回填不得再 pause: {history:?}"
        );
        assert!(b
            .events
            .iter()
            .any(|event| matches!(event, StateChange::PaneSnapshot { pane: p, .. } if *p == pane)));
    }

    #[test]
    fn own_pause_echo_does_not_start_second_resync() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        b.cmd_tx = Some(tx);
        let pane = PaneId(2);
        b.begin_initial_pane_seed(pane);
        let _ = drain_tmux_cmds(&mut rx);
        let generation = b.resyncs.get(&pane).unwrap().generation;

        b.handle_message(Message::Pause {
            pane: Some(pane),
            args: String::new(),
        });
        assert_eq!(
            b.resyncs.get(&pane).map(|resync| resync.generation),
            Some(generation),
            "自己发的 %pause 回声不得再开一轮 resync"
        );
        let cmds = drain_tmux_cmds(&mut rx);
        assert!(cmds.is_empty(), "pause 回声不得再发命令: {cmds:?}");
    }

    #[test]
    fn pane_size_change_restarts_in_flight_seed_without_stale_snapshot() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        b.cmd_tx = Some(tx);
        b.tabs = vec![TabInfo {
            id: TabId(0),
            name: "t".into(),
            active: true,
        }];
        let pane = PaneId(2);
        b.panes.push(PaneInfo {
            id: pane,
            tab: TabId(0),
            active: true,
            title: String::new(),
            cols: 128,
            rows: 63,
        });
        b.begin_initial_pane_seed(pane);
        let _ = drain_tmux_cmds(&mut rx);
        b.events.clear();

        b.handle_list_panes_response(TabId(0), vec!["0: [93x51] %2 (active)".into()]);

        assert!(
            !b.events.iter().any(|event| matches!(
                event, StateChange::PaneSnapshot { pane: p, .. } if *p == pane
            )),
            "旧尺寸的 seed 不得当成快照发出，否则 TUI 会按 128 列折到 93 列"
        );
        assert!(b.resyncs.contains_key(&pane), "必须立刻按新尺寸再 seed");
        let cmds = drain_tmux_cmds(&mut rx);
        assert!(
            cmds.iter()
                .any(|cmd| cmd.contains("%2:pause") || cmd.contains("display-message")),
            "resize 后应重新发起 seed: {cmds:?}"
        );
        assert!(
            !cmds.iter().any(|cmd| cmd.contains("-S ")),
            "resize 重抓也只能是可见屏: {cmds:?}"
        );
    }

    #[test]
    fn resync_capture_keeps_tui_trailing_blank_rows() {
        let lines = vec!["PROMPT>".into(), String::new(), String::new()];
        assert_eq!(
            capture_pane_bytes(&lines),
            b"PROMPT>",
            "索引用的可见屏 capture 仍可裁尾部空行"
        );
        assert_eq!(
            capture_pane_grid_bytes(&lines),
            b"PROMPT>\r\n\r\n",
            "TUI 快照必须保留底部空行，否则 alternate screen 网格上移"
        );
        assert_eq!(
            capture_pane_surface_bytes(&lines),
            b"\x1b[H\x1b[1HPROMPT>",
            "索引 Surface seed 必须 CUP 到第 1 行，不能 trim 成光秃正文"
        );

        let state = parse_pane_replay_state("0|2|1|block|0|1|0|0|0|1|0|0|0|0|0|0|0|0|0|0");
        let alt = capture_pane_grid_bytes(&["row0".into(), String::new(), String::new()]);
        let snapshot = build_pane_snapshot(Some(&state), b"", &alt, b"");
        let marker = b"\x1b[?1049h";
        let alt_at = snapshot
            .windows(marker.len())
            .position(|window| window == marker)
            .expect("alternate screen 必须进入 1049h");
        let after = &snapshot[alt_at + marker.len()..];
        assert!(
            after.starts_with(b"row0\r\n\r\n"),
            "1049h 之后必须是完整网格，实际 {:?}",
            String::from_utf8_lossy(after)
        );
    }

    #[test]
    fn initial_seed_deadline_emits_snapshot_so_surface_can_unhide() {
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        b.cmd_tx = Some(tx);
        let pane = PaneId(2);
        b.begin_initial_pane_seed(pane);
        b.handle_message(Message::Output {
            pane,
            content: b"live-during-seed".to_vec(),
            raw_content: String::new(),
        });
        b.resyncs.get_mut(&pane).unwrap().deadline =
            Some(Instant::now() - Duration::from_millis(1));
        b.expire_resyncs();

        assert!(!b.resyncs.contains_key(&pane));
        assert!(b.initial_capture_done.contains(&pane));
        assert!(
            b.events.iter().any(|event| matches!(
                event,
                StateChange::PaneSnapshot { pane: p, data }
                    if *p == pane
                        && data
                            .windows(b"live-during-seed".len())
                            .any(|window| window == b"live-during-seed")
            )),
            "seed 超时必须用 snapshot 解开 Surface 隐藏，不能只追加半截 live"
        );
        assert!(
            !b.events.iter().any(
                |event| matches!(event, StateChange::PaneOutput { pane: p, .. } if *p == pane)
            ),
            "首屏超时不得再发 PaneOutput 让半截字节叠在空屏上"
        );
    }

    #[test]
    fn resync_deadline_releases_live_fallback_and_starts_cooldown() {
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new(None);
        b.cmd_tx = Some(tx);
        let pane = PaneId(33);
        b.flow.entry(pane).or_default().resyncing = true;
        b.flow
            .get_mut(&pane)
            .unwrap()
            .suppressed
            .extend_from_slice(b"suppressed\r\n");
        b.resyncs.insert(
            pane,
            PaneResync {
                deadline: Some(Instant::now() - Duration::from_millis(1)),
                live: b"live\r\n".to_vec(),
                pause_client: false,
                ..PaneResync::default()
            },
        );

        b.expire_resyncs();

        assert!(!b.resyncs.contains_key(&pane));
        assert!(!b.flow.get(&pane).unwrap().resyncing);
        assert!(b.outputs.get(&pane).is_some_and(|data| {
            data.windows(b"live\r\n".len())
                .any(|window| window == b"live\r\n")
                && data
                    .windows(b"suppressed\r\n".len())
                    .any(|window| window == b"suppressed\r\n")
        }));
        assert!(b
            .resync_cooldown_until
            .get(&pane)
            .is_some_and(|until| *until > Instant::now()));
    }

    #[test]
    fn stale_resync_generation_cannot_paint_or_complete_new_transaction() {
        let mut b = TmuxRuntime::new(None);
        let pane = PaneId(34);
        b.resyncs.insert(
            pane,
            PaneResync {
                generation: 2,
                ..PaneResync::default()
            },
        );

        b.pending_by_number.insert(
            101,
            PendingQuery::PaneResyncState {
                pane,
                generation: 1,
            },
        );
        b.dispatch_response(101, vec!["0|0|1|block|0|0".into()]);
        assert_eq!(b.resyncs.get(&pane).unwrap().generation, 2);
        assert!(b
            .pending_queries
            .iter()
            .all(|query| !matches!(query, PendingQuery::PaneResyncCapture { .. })));

        b.pending_by_number.insert(
            102,
            PendingQuery::PaneResyncCapture {
                pane,
                alternate: true,
                generation: 1,
            },
        );
        b.dispatch_response(102, vec!["stale frame".into()]);
        assert!(b.resyncs.contains_key(&pane));
        assert!(!b.events.iter().any(|event| {
            matches!(event, StateChange::PaneSnapshot { pane: p, .. } if *p == pane)
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
    async fn attach_chatty_tui_does_not_gap_control_output() {
        let socket = unique_socket();
        let run = async {
            let created = std::process::Command::new("tmux")
                .args([
                    "-L",
                    &socket,
                    "new-session",
                    "-d",
                    "-s",
                    "chatty",
                    "-x",
                    "80",
                    "-y",
                    "24",
                    "sh",
                    "-c",
                    "i=0; while [ \"$i\" -lt 400 ]; do printf '\\033[H\\033[2Jframe-%s\\n' \"$i\"; i=$((i+1)); done; exec sleep 30",
                ])
                .status();
            if !created
                .as_ref()
                .is_ok_and(std::process::ExitStatus::success)
            {
                eprintln!("skip: 无法在隔离 socket 上创建 chatty session");
                return;
            }
            let mut b = TmuxRuntime::new_with_attach(Some(&socket), "chatty");
            if b.connect().await.is_err() {
                return;
            }
            let deadline = Instant::now() + Duration::from_millis(800);
            while Instant::now() < deadline {
                let _ = b.take_events();
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(
                b.dropped_output_panes.is_empty(),
                "CUP 洪峰不得把共享 output lane 打成 OutputGap，否则就会 pause+大 capture 再整屏重绘"
            );
            let _ = b.shutdown().await;
        };
        let timed = tokio::time::timeout(TMUX_TEST_TIMEOUT, run).await;
        cleanup(&socket);
        if timed.is_err() {
            panic!("attach_chatty_tui_does_not_gap_control_output 超时");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn attach_first_paint_beats_background_flood() {
        let socket = unique_socket();
        let run = async {
            let created = std::process::Command::new("tmux")
                .args([
                    "-L",
                    &socket,
                    "new-session",
                    "-d",
                    "-s",
                    "paint",
                    "-n",
                    "main",
                    "-x",
                    "80",
                    "-y",
                    "24",
                    "sh",
                    "-c",
                    "printf 'FIRST_PAINT_OK\\n'; exec cat",
                ])
                .status();
            if !created
                .as_ref()
                .is_ok_and(std::process::ExitStatus::success)
            {
                eprintln!("skip: 无法在隔离 socket 上创建 paint session");
                return;
            }
            let _ = std::process::Command::new("tmux")
                .args([
                    "-L",
                    &socket,
                    "new-window",
                    "-t",
                    "paint",
                    "-n",
                    "flood",
                    "sh",
                    "-c",
                    "i=0; while [ \"$i\" -lt 800 ]; do printf '\\033[H\\033[2Jflood-%s\\n' \"$i\"; i=$((i+1)); done; exec sleep 30",
                ])
                .status();
            let _ = std::process::Command::new("tmux")
                .args(["-L", &socket, "select-window", "-t", "paint:main"])
                .status();
            let wait_token = Instant::now() + Duration::from_secs(2);
            loop {
                let out = std::process::Command::new("tmux")
                    .args(["-L", &socket, "capture-pane", "-p", "-t", "paint:main"])
                    .output();
                if out
                    .as_ref()
                    .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("FIRST_PAINT_OK"))
                {
                    break;
                }
                if Instant::now() >= wait_token {
                    eprintln!("skip: 隔离 session 没有 FIRST_PAINT_OK");
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }

            let mut b = TmuxRuntime::new_with_attach(Some(&socket), "paint");
            let started = Instant::now();
            if b.connect().await.is_err() {
                return;
            }
            let panes: Vec<PaneId> = b.panes.iter().map(|pane| pane.id).collect();
            for pane in panes {
                let _ = b.execute(&crate::core::model::task::Task::ReportPaneColours {
                    target: pane,
                    fg: crate::core::config::Rgb(0, 0, 0),
                    bg: crate::core::config::Rgb(255, 255, 255),
                });
            }
            let paint_deadline = Duration::from_millis(1000);
            let mut painted = false;
            while started.elapsed() < paint_deadline {
                painted = b.events.iter().any(|event| {
                    matches!(
                        event,
                        StateChange::PaneSnapshot { data, .. }
                            if data.windows(b"FIRST_PAINT_OK".len()).any(|w| w == b"FIRST_PAINT_OK")
                    )
                }) || b.take_events().iter().any(|event| {
                    matches!(
                        event,
                        StateChange::PaneSnapshot { data, .. }
                            if data.windows(b"FIRST_PAINT_OK".len()).any(|w| w == b"FIRST_PAINT_OK")
                    )
                });
                if painted {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(
                painted,
                "后台 flood + OSC 也必须在 1s 内种上活动 pane，不能空超时"
            );
            assert!(
                started.elapsed() < paint_deadline,
                "本机隔离 socket 首屏不得超过 1s，实际 {:?}",
                started.elapsed()
            );
            assert!(b.resyncs.is_empty(), "首屏成功后不得还停在 pause/resync");
            let _ = b.shutdown().await;
        };
        let timed = tokio::time::timeout(TMUX_TEST_TIMEOUT, run).await;
        cleanup(&socket);
        if timed.is_err() {
            panic!("attach_first_paint_beats_background_flood 超时");
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
            b.capture_grid
                .insert(crate::core::types::PaneId(id), (40, 24));
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
        assert!(
            [5u32, 6]
                .into_iter()
                .all(|id| !b.capture_grid.contains_key(&crate::core::types::PaneId(id))),
            "window 关闭后 pane capture 网格也必须回收"
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
            "@0,first,1,aaaa,80x24,0,0,1,0,0".into(),
            "@1,second,0,bbbb,80x24,0,0,1,0,1".into(),
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
            "@0,first,1,aaaa,80x24,0,0,1,0,0".into(),
            "@1,second,0,bbbb,80x24,0,0,1,0,1".into(),
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

    #[test]
    fn list_windows_skips_unchanged_windows_when_a_tab_is_added() {
        let mut b = TmuxRuntime::new(None);
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        b.cmd_tx = Some(tx);
        b.status = BackendStatus::Connected;

        b.handle_list_windows_response(vec![
            "@0,first,1,aaaa,80x24,0,0,1,0,0".into(),
            "@1,second,0,bbbb,80x24,0,0,1,0,1".into(),
        ]);
        let first = drain_tmux_cmds(&mut rx);
        assert_eq!(
            first
                .iter()
                .filter(|cmd| cmd.contains("list-panes"))
                .count(),
            2,
            "第一次必须查每个 window: {first:?}"
        );
        b.handle_list_panes_response(TabId(0), vec!["0: [80x24] %0 (active)".into()]);
        b.handle_list_panes_response(TabId(1), vec!["1: [80x24] %1 (active)".into()]);
        let _ = drain_tmux_cmds(&mut rx);
        b.pending_queries.clear();

        b.handle_list_windows_response(vec![
            "@0,first,0,aaaa,80x24,0,0,1,0,0".into(),
            "@1,second,0,bbbb,80x24,0,0,1,0,1".into(),
            "@2,third,1,cccc,80x24,0,0,1,0,2".into(),
        ]);
        let cmds = drain_tmux_cmds(&mut rx);
        let list_panes: Vec<_> = cmds
            .iter()
            .filter(|cmd| cmd.contains("list-panes"))
            .collect();
        assert_eq!(
            list_panes.len(),
            1,
            "旧 window layout 没变就不要再查: {cmds:?}"
        );
        assert!(list_panes[0].contains("@2"), "只能查新 window: {cmds:?}");
    }

    #[test]
    fn list_windows_skips_remaining_windows_after_close() {
        let mut b = TmuxRuntime::new(None);
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        b.cmd_tx = Some(tx);
        b.status = BackendStatus::Connected;

        b.handle_list_windows_response(vec![
            "@0,first,1,aaaa,80x24,0,0,1,0,0".into(),
            "@1,second,0,bbbb,80x24,0,0,1,0,1".into(),
        ]);
        b.handle_list_panes_response(TabId(0), vec!["0: [80x24] %0 (active)".into()]);
        b.handle_list_panes_response(TabId(1), vec!["1: [80x24] %1 (active)".into()]);
        let _ = drain_tmux_cmds(&mut rx);
        b.pending_queries.clear();

        b.handle_list_windows_response(vec!["@0,first,1,aaaa,80x24,0,0,1,0,0".into()]);
        let cmds = drain_tmux_cmds(&mut rx);
        assert!(
            !cmds.iter().any(|cmd| cmd.contains("list-panes")),
            "关掉一张 tab 不得把剩下的 window 再 list-panes 一遍: {cmds:?}"
        );
    }

    #[test]
    fn list_windows_requeries_when_layout_changes() {
        let mut b = TmuxRuntime::new(None);
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        b.cmd_tx = Some(tx);
        b.status = BackendStatus::Connected;

        b.handle_list_windows_response(vec!["@0,first,1,aaaa,80x24,0,0,1,0,0".into()]);
        b.handle_list_panes_response(TabId(0), vec!["0: [80x24] %0 (active)".into()]);
        let _ = drain_tmux_cmds(&mut rx);
        b.pending_queries.clear();

        b.handle_list_windows_response(vec![
            "@0,first,1,dddd,80x24,0,0{40x24,0,0,0,39x24,41,0,1},2,0,0".into(),
        ]);
        let cmds = drain_tmux_cmds(&mut rx);
        assert!(
            cmds.iter().any(|cmd| cmd.contains("list-panes -t @0")),
            "layout 变了必须再查 pane: {cmds:?}"
        );
    }

    #[test]
    fn post_attach_new_pane_uses_visible_capture_without_pause() {
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        b.cmd_tx = Some(tx);
        b.status = BackendStatus::Connected;
        b.background_index_capture_enabled = true;
        b.tabs = vec![TabInfo {
            id: TabId(3),
            name: "new".into(),
            active: true,
        }];
        b.handle_list_panes_response(TabId(3), vec!["0: [80x24] %9 (active)".into()]);
        let cmds = drain_tmux_cmds(&mut rx);
        assert!(
            cmds.iter().any(|cmd| cmd.contains("capture-pane")),
            "新建 pane 仍要抓一帧可见屏，否则 prompt 可能还没到: {cmds:?}"
        );
        assert!(
            !cmds
                .iter()
                .any(|cmd| cmd.contains("pause") || cmd.contains("-S ")),
            "attach 完成后新建 pane 不得 pause，也不得抓 1 万行历史: {cmds:?}"
        );
        assert!(b.history_backfill_done.contains(&PaneId(9)));
        assert!(!b.initial_capture_done.contains(&PaneId(9)));
        assert!(b.initial_capture_pending.contains(&PaneId(9)));

        b.pending_by_number
            .insert(1, PendingQuery::CapturePane { pane: PaneId(9) });
        b.dispatch_response(1, vec!["prompt$".into()]);
        assert!(b.initial_capture_done.contains(&PaneId(9)));
        assert!(
            b.events.iter().any(|event| matches!(
                event,
                StateChange::PaneSnapshot { pane, data }
                    if *pane == PaneId(9) && data.windows(7).any(|w| w == b"prompt$")
            )),
            "新建 pane 的可见屏必须变成 PaneSnapshot，否则 host 会一直藏着: {:?}",
            b.events.back()
        );
        assert!(
            !b.events.iter().any(
                |event| matches!(event, StateChange::PaneHistory { pane, .. } if *pane == PaneId(9))
            ),
            "新建 pane 没有 attach 前历史，不得再发 PaneHistory"
        );
    }

    #[test]
    fn attach_active_tab_still_pause_seeds() {
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        b.cmd_tx = Some(tx);
        b.tabs = vec![TabInfo {
            id: TabId(1),
            name: "active".into(),
            active: true,
        }];
        b.handle_list_panes_response(TabId(1), vec!["0: [80x24] %0 (active)".into()]);
        let cmds = drain_tmux_cmds(&mut rx);
        assert!(
            cmds.iter().any(|cmd| cmd.contains("pause")),
            "attach 首屏仍必须 pause-seed: {cmds:?}"
        );
        assert!(b.initial_capture_pending.contains(&PaneId(0)));
    }

    #[test]
    fn attach_connect_must_not_wait_on_background_index_or_tmux_version() {
        let src = include_str!("backend.rs");
        let connect = src
            .split("async fn connect(")
            .nth(1)
            .expect("connect")
            .split("\n    fn execute(")
            .next()
            .expect("execute after connect");
        assert!(
            !connect.contains("attach_visible_captures_ready"),
            "connect 不得等后台可见屏索引，否则进入 tmux 会先卡一下"
        );
        assert!(
            !connect.contains("detect_colour_report_support"),
            "connect 不得再跑 tmux -V / SSH 往返"
        );
        assert!(
            connect.contains("background_index_capture_enabled = true"),
            "attach 仍要打开后台索引，只是放到 Connected 之后"
        );
        assert!(
            connect.contains("status_subscription_supported = true"),
            "跳过 -V 之后仍要尝试 status 订阅"
        );
        assert!(
            connect.contains("initial_seed_blocks_followup")
                && connect.contains("flush_attach_followup_commands"),
            "connect 必须把 list-sessions/-B 排到可见 capture 之后"
        );
        assert!(
            !connect.contains("self.query_list_sessions();"),
            "connect 不得在 seed 还在飞时无条件 list-sessions"
        );
    }

    #[test]
    fn history_backfill_and_background_index_wait_until_connected() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        b.cmd_tx = Some(tx);
        b.status = BackendStatus::Connecting;
        b.background_index_capture_enabled = true;
        b.tabs = vec![
            TabInfo {
                id: TabId(1),
                name: "active".into(),
                active: true,
            },
            TabInfo {
                id: TabId(2),
                name: "bg".into(),
                active: false,
            },
        ];
        b.panes = vec![
            PaneInfo {
                id: PaneId(0),
                tab: TabId(1),
                active: true,
                title: String::new(),
                cols: 80,
                rows: 24,
            },
            PaneInfo {
                id: PaneId(1),
                tab: TabId(2),
                active: true,
                title: String::new(),
                cols: 80,
                rows: 24,
            },
        ];
        b.history_backfill_wanted.insert(PaneId(0));
        b.history_backfill_hold = false;
        b.pump_events();
        let during_connect = drain_tmux_cmds(&mut rx);
        assert!(
            !during_connect
                .iter()
                .any(|cmd| cmd.contains("-S ") || cmd.contains("capture-pane")),
            "connect settle 期间不得发历史或后台索引: {during_connect:?}"
        );
        assert!(
            !b.attach_visible_captures_ready(),
            "后台索引还没开始时 attach_visible_captures_ready 必须是 false"
        );

        b.status = BackendStatus::Connected;
        b.pump_events();
        let after_connect = drain_tmux_cmds(&mut rx);
        assert!(
            after_connect
                .iter()
                .any(|cmd| cmd.contains("-S ") && cmd.contains("%0")),
            "Connected 后先补活动 pane 历史: {after_connect:?}"
        );
        assert!(
            !after_connect
                .iter()
                .any(|cmd| cmd.contains("%1") && cmd.contains("capture-pane")),
            "历史还在路上时不得插进后台索引: {after_connect:?}"
        );

        b.pending_queries.clear();
        b.pending_by_number.clear();
        b.history_backfill_done.insert(PaneId(0));
        b.history_backfill_pending.remove(&PaneId(0));
        b.history_backfill_wanted.clear();
        b.pump_events();
        let index = drain_tmux_cmds(&mut rx);
        assert!(
            index
                .iter()
                .any(|cmd| cmd.contains("capture-pane") && cmd.contains("%1")),
            "历史走完后再索引后台 tab: {index:?}"
        );
        assert!(
            !index
                .iter()
                .any(|cmd| cmd.contains("pause") || cmd.contains("-S ")),
            "后台索引只抓可见屏: {index:?}"
        );
    }

    /// 建 tab / 关 tab / 第一次点从未打开的 tab / 再切入已打开的 tab
    /// 共用控制通道。一条路径的优化不得把另一条的 seed/历史标干掉。
    #[test]
    fn tab_create_close_must_not_break_first_open_or_cached_switch() {
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        b.cmd_tx = Some(tx);
        b.status = BackendStatus::Connected;
        b.background_index_capture_enabled = true;
        b.tabs = vec![
            TabInfo {
                id: TabId(0),
                name: "first".into(),
                active: true,
            },
            TabInfo {
                id: TabId(1),
                name: "second".into(),
                active: false,
            },
        ];
        b.panes = vec![
            PaneInfo {
                id: PaneId(0),
                tab: TabId(0),
                active: true,
                title: String::new(),
                cols: 80,
                rows: 24,
            },
            PaneInfo {
                id: PaneId(1),
                tab: TabId(1),
                active: true,
                title: String::new(),
                cols: 80,
                rows: 24,
            },
        ];
        b.window_layouts.insert(TabId(0), "aaaa,80x24,0,0".into());
        b.window_layouts.insert(TabId(1), "bbbb,80x24,0,0".into());
        b.expected_panes_per_window.insert(TabId(0), 1);
        b.expected_panes_per_window.insert(TabId(1), 1);
        b.initial_capture_done.insert(PaneId(0));
        b.history_backfill_done.insert(PaneId(0));
        b.initial_capture_done.insert(PaneId(1));
        b.background_capture_only.insert(PaneId(1));

        b.handle_list_windows_response(vec![
            "@0,first,0,aaaa,80x24,0,0,1,0,0".into(),
            "@1,second,0,bbbb,80x24,0,0,1,0,1".into(),
            "@2,third,1,cccc,80x24,0,0,1,0,2".into(),
        ]);
        let created = drain_tmux_cmds(&mut rx);
        let list_panes: Vec<_> = created
            .iter()
            .filter(|cmd| cmd.contains("list-panes"))
            .collect();
        assert_eq!(
            list_panes.len(),
            1,
            "新建 tab 不得把旧 window 再 list-panes: {created:?}"
        );
        assert!(list_panes[0].contains("@2"), "{created:?}");
        assert!(
            !b.history_backfill_done.contains(&PaneId(1)),
            "新建 tab 不得把其它 tab 的历史标成已完成"
        );
        assert!(b.initial_capture_done.contains(&PaneId(0)));
        assert!(b.initial_capture_done.contains(&PaneId(1)));

        b.pending_queries.clear();
        b.handle_list_panes_response(TabId(2), vec!["0: [80x24] %9 (active)".into()]);
        let seed = drain_tmux_cmds(&mut rx);
        assert!(
            seed.iter().any(|cmd| cmd.contains("capture-pane")),
            "新 pane 仍要抓可见屏: {seed:?}"
        );
        assert!(
            !seed
                .iter()
                .any(|cmd| cmd.contains("pause") || cmd.contains("-S ")),
            "新 pane 不得 pause、不得抓 1 万行: {seed:?}"
        );
        assert!(b.history_backfill_done.contains(&PaneId(9)));
        assert!(
            !b.history_backfill_done.contains(&PaneId(1)),
            "给新 pane 标 history_backfill_done 不得连同旧 pane 一起标"
        );

        b.pending_queries.clear();
        b.pending_by_number.clear();
        b.mark_tab_active(TabId(1));
        b.query_capture_tab(TabId(1));
        let first_open = drain_tmux_cmds(&mut rx);
        assert!(
            !first_open.iter().any(|cmd| cmd.contains("pause")),
            "第一次点从未打开的 tab 仍不得 pause: {first_open:?}"
        );
        assert!(
            !first_open.iter().any(|cmd| cmd.contains("-S -")),
            "切 tab 当拍不得抓历史: {first_open:?}"
        );
        b.history_backfill_hold = false;
        b.flush_deferred_history_backfill();
        let history = drain_tmux_cmds(&mut rx);
        assert!(
            history
                .iter()
                .any(|cmd| cmd.contains("-S -") && cmd.contains("%1")),
            "新建 tab 之后，从未打开过的 tab 仍必须能补历史: {history:?}"
        );

        b.history_backfill_done.insert(PaneId(1));
        b.history_backfill_pending.remove(&PaneId(1));
        b.history_backfill_wanted.remove(&PaneId(1));
        b.mark_tab_active(TabId(0));
        b.query_capture_tab(TabId(0));
        let cached = drain_tmux_cmds(&mut rx);
        assert!(cached.is_empty(), "已打开的 tab 再切入不得再抓: {cached:?}");

        b.pending_queries.clear();
        b.handle_list_windows_response(vec![
            "@0,first,1,aaaa,80x24,0,0,1,0,0".into(),
            "@1,second,0,bbbb,80x24,0,0,1,0,1".into(),
        ]);
        let closed = drain_tmux_cmds(&mut rx);
        assert!(
            !closed.iter().any(|cmd| cmd.contains("list-panes")),
            "关掉一张 tab 不得把剩下的 window 再查一遍: {closed:?}"
        );
        b.query_capture_tab(TabId(0));
        let after_close = drain_tmux_cmds(&mut rx);
        assert!(
            after_close.is_empty(),
            "关 tab 之后已打开的 tab 仍不得再抓: {after_close:?}"
        );
    }

    #[test]
    fn split_after_attach_does_not_pause_already_seeded_pane() {
        let mut b = TmuxRuntime::new_with_attach(None, "existing");
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        b.cmd_tx = Some(tx);
        b.status = BackendStatus::Connected;
        b.background_index_capture_enabled = true;
        b.handle_list_windows_response(vec!["@0,first,1,aaaa,80x24,0,0,1,0,0".into()]);
        b.handle_list_panes_response(TabId(0), vec!["0: [80x24] %0 (active)".into()]);
        let _ = drain_tmux_cmds(&mut rx);
        b.initial_capture_done.insert(PaneId(0));
        b.history_backfill_done.insert(PaneId(0));
        b.pending_queries.clear();

        b.handle_list_windows_response(vec![
            "@0,first,1,dddd,80x24,0,0{40x24,0,0,0,39x24,41,0,1},2,0,0".into(),
        ]);
        let requery = drain_tmux_cmds(&mut rx);
        assert!(
            requery.iter().any(|cmd| cmd.contains("list-panes -t @0")),
            "split 后 layout 变了必须再查 pane: {requery:?}"
        );
        assert!(
            !requery.iter().any(|cmd| cmd.contains("pause")),
            "list-windows 自己不得 pause: {requery:?}"
        );

        b.handle_list_panes_response(
            TabId(0),
            vec!["0: [40x24] %0 (active)".into(), "1: [39x24] %1".into()],
        );
        let seed = drain_tmux_cmds(&mut rx);
        assert!(
            !seed.iter().any(|cmd| cmd.contains("pause")),
            "split 后已 seed 的 pane 不得再 pause: {seed:?}"
        );
        assert!(
            seed.iter()
                .any(|cmd| cmd.contains("capture-pane") && cmd.contains("%1")),
            "新 split 出来的 pane 仍要抓可见屏: {seed:?}"
        );
        assert!(
            !seed
                .iter()
                .any(|cmd| cmd.contains("capture-pane") && cmd.contains("%0")),
            "已经 seed 过的 pane 不得再 capture: {seed:?}"
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

    /// 相同 layout 的 %layout-change（切 tab / 重复 -C）不得再 list-panes。
    #[test]
    fn identical_layout_change_does_not_requery_panes() {
        use crate::core::runtime::tmux::protocol::Message;

        let mut b = TmuxRuntime::new(None);
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        b.cmd_tx = Some(tx);
        b.tabs.push(crate::core::model::state::TabInfo {
            id: crate::core::types::TabId(5),
            name: "code".into(),
            active: true,
        });
        let raw = "abcd,80x24,0,0,1";
        b.handle_message(Message::LayoutChange {
            window: crate::core::types::TabId(5),
            layout: crate::core::runtime::tmux::protocol::LayoutChange::parse(raw).unwrap(),
            visible_layout: None,
            flags: Some("*".into()),
        });
        let first = drain_tmux_cmds(&mut rx);
        assert!(
            first.iter().any(|cmd| cmd.contains("list-panes -t @5")),
            "首次 layout-change 应查询 pane: {first:?}"
        );
        b.handle_list_panes_response(
            crate::core::types::TabId(5),
            vec!["0: [80x24] %1 (active)".into()],
        );
        let _ = drain_tmux_cmds(&mut rx);

        b.handle_message(Message::LayoutChange {
            window: crate::core::types::TabId(5),
            layout: crate::core::runtime::tmux::protocol::LayoutChange::parse(raw).unwrap(),
            visible_layout: None,
            flags: Some("-".into()),
        });
        let second = drain_tmux_cmds(&mut rx);
        assert!(
            !second.iter().any(|cmd| cmd.contains("list-panes")),
            "相同 layout 不得再 list-panes: {second:?}"
        );
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

    #[test]
    fn stale_switch_confirmation_cannot_override_latest_target() {
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let mut b = TmuxRuntime::new(None);
        b.cmd_tx = Some(tx);
        b.status = crate::core::model::state::BackendStatus::Connected;
        let session = crate::core::runtime::tmux::protocol::TmuxSessionId(0);
        b.active_session = Some(session);
        for (id, active) in [(18u32, true), (47, false), (52, false)] {
            b.tabs.push(crate::core::model::state::TabInfo {
                id: crate::core::types::TabId(id),
                name: format!("t{id}"),
                active,
            });
        }

        assert!(matches!(
            b.execute(&Task::SwitchTab {
                target: crate::core::types::TabId(47),
            }),
            Ok(TaskOutcome::Done)
        ));
        assert!(matches!(
            b.execute(&Task::SwitchTab {
                target: crate::core::types::TabId(52),
            }),
            Ok(TaskOutcome::Done)
        ));

        b.handle_message(Message::SessionWindowChanged {
            session,
            window: crate::core::types::TabId(47),
        });
        assert_eq!(
            b.active_tab().map(|tab| tab.id),
            Some(crate::core::types::TabId(52)),
            "旧目标确认不能覆盖最新目标"
        );
        assert!(b.latest_switch_target.is_some());

        b.handle_message(Message::SessionWindowChanged {
            session,
            window: crate::core::types::TabId(52),
        });
        assert!(b.latest_switch_target.is_none());
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
