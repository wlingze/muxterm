//! Pane 视图：VTE4 渲染 + FFI 输入/输出。
//!
//! 与 macOS TerminalManager/TerminalView 对齐：
//! - 新视图先用后端报告的 pane 字符格尺寸 resize 模型，再喂快照；
//! - 输出按 pane 合并后 feed（短窗口），避免 agent 帧被逐事件拆开；
//! - tmux/SSH 镜像模式丢弃 feed 期间解析器生成的查询应答（tmux 自己代答）；
//! - 主题 / 字号可在运行期切换。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::glib;
use gtk4::prelude::*;
use vte4::prelude::*;

use crate::core::config::{Rgb, Theme};
use crate::core::protocol::terminal::emulate::TerminalState;
use crate::core::protocol::terminal::mirror::{
    should_forward_mixed_input, should_forward_parser_response, DISABLE_MOUSE_TRACKING,
};
use crate::core::url_detect::UrlOpener;
use crate::platform::linux::quickconnect::font::FontSettings;
use crate::platform::linux::renderer::{TerminalRenderer, VteRenderer};
use crate::platform::linux::scroll_policy::{wheel_action, WheelAction};

/// 同一 pane 输出合并后刷新的窗口（毫秒）。
pub const FEED_COALESCE_MS: u64 = 25;

/// 用户输入回调：`(pane_id, bytes)`。
pub type InputCallback = Box<dyn Fn(u32, &[u8])>;

/// 渲染痕迹：没有视觉时证明「不刷屏」。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderTrace {
    pub resets: u32,
    pub feeds: u32,
    pub bytes_fed: usize,
    pub seeds: u32,
}

/// 一个 pane 的 GTK 视图（薄包装；内部用 Rc 供合并定时器弱引用）。
pub struct PaneView {
    inner: Rc<PaneViewInner>,
}

struct PaneViewInner {
    renderer: VteRenderer,
    pane_id: Cell<u32>,
    /// 无头终端状态：捕获 OSC 10/11/12、CSI DA 等查询并生成应答。
    reply_state: RefCell<TerminalState>,
    /// 已生成、待回写 shell 的应答字节。
    pending_replies: RefCell<Vec<u8>>,
    /// 用户输入回调（connect_input 注册；测试可经 test_emit_input 触发）。
    input_cb: RefCell<Option<InputCallback>>,
    /// tmux/SSH 镜像模式：feed 期间解析器应答一律丢弃。
    is_tmux_mirror: Cell<bool>,
    /// 正在把远端 pane 输出 feed 进 VTE（解析器应答只在这个窗口产生）。
    is_feeding_remote_output: Cell<bool>,
    /// 待合并的输出。
    pending_feed: RefCell<Vec<u8>>,
    feed_flush_source: RefCell<Option<glib::SourceId>>,
    /// attach 历史批次。权威 Snapshot 会 reset native scrollback，所以
    /// 必须保留这些批次，按 Surface generation 重放。
    history_batches: RefCell<Vec<Vec<u8>>>,
    /// 快照/历史 feed 后需要把 viewport 钉回底部（镜像模式
    /// scroll-on-output=false 时 feed 不自动滚动）。
    scroll_to_bottom_pending: Cell<bool>,
    /// 当前 Surface generation 已写入的 history_batches 数量。
    history_applied: Cell<usize>,
    /// 已播种过；未播种前不应走增量 feed。
    seeded: Cell<bool>,
    /// VTE / 无头模型当前字符格。Codex CUP 帧必须与此一致。
    grid_cols: Cell<u16>,
    grid_rows: Cell<u16>,
    /// 渲染痕迹（reset/feed 次数与字节数）。
    render_trace: RefCell<RenderTrace>,
    /// URL 打开出口（测试注入 Recording，生产接 GTK）。
    url_opener: RefCell<Option<Rc<dyn UrlOpener>>>,
}

impl PaneView {
    pub fn new(
        pane_id: u32,
        theme: &Theme,
        font: &FontSettings,
        is_tmux_mirror: bool,
        scrollback_lines: u32,
    ) -> Self {
        let renderer = VteRenderer::new();
        renderer.apply_theme(theme);
        renderer.apply_font(font);
        renderer.apply_mirror_policy(is_tmux_mirror);
        renderer
            .terminal()
            .set_scrollback_lines(scrollback_lines as i64);
        // OSC 8 超链接默认关闭：打开才能 check_hyperlink_at。
        renderer.terminal().set_allow_hyperlink(true);
        // https? URL 正则匹配（点击走 check_hyperlink_at 优先，其次 match）。
        // VTE 要求 regex 带 PCRE2_MULTILINE 编译标志（0x400）。
        if let Ok(re) = vte4::Regex::for_match(r#"https?://[^\s<>"']+"#, 0x400) {
            renderer.terminal().match_add_regex(&re, 0);
        }
        let inner = Rc::new(PaneViewInner {
            renderer,
            pane_id: Cell::new(pane_id),
            reply_state: RefCell::new(TerminalState::new(80, 24)),
            pending_replies: RefCell::new(Vec::new()),
            input_cb: RefCell::new(None),
            is_tmux_mirror: Cell::new(is_tmux_mirror),
            is_feeding_remote_output: Cell::new(false),
            pending_feed: RefCell::new(Vec::new()),
            feed_flush_source: RefCell::new(None),
            history_batches: RefCell::new(Vec::new()),
            history_applied: Cell::new(0),
            scroll_to_bottom_pending: Cell::new(false),
            seeded: Cell::new(false),
            grid_cols: Cell::new(80),
            grid_rows: Cell::new(24),
            render_trace: RefCell::new(RenderTrace::default()),
            url_opener: RefCell::new(None),
        });
        let view = PaneView { inner };
        view.attach_scroll_controller();
        view
    }

    /// W21：生产滚轮路径。主屏滚 VTE 历史；alt-screen 发 CSI 方向键。
    /// 与 EventControllerScroll 同一函数（测试 test_emit_scroll 也走这里）。
    fn handle_scroll(&self, delta_y: f64) {
        let alternate_screen = self.inner.reply_state.borrow().alternate_screen;
        let Some(action) = wheel_action(alternate_screen, delta_y) else {
            return;
        };
        match action {
            WheelAction::ScrollHistory { lines } => {
                if let Some(adj) = self.inner.renderer.terminal().vadjustment() {
                    let step = adj.step_increment().max(1.0);
                    let target = adj.value() + lines as f64 * step;
                    let lower = adj.lower();
                    let upper = (adj.upper() - adj.page_size()).max(lower);
                    adj.set_value(target.clamp(lower, upper));
                }
            }
            WheelAction::SendToApp { bytes } => {
                if let Some(cb) = self.inner.input_cb.borrow().as_ref() {
                    cb(self.inner.pane_id.get(), &bytes);
                }
            }
        }
    }

    /// 挂垂直滚轮控制器（生产路径；测试经 test_emit_scroll 走同一函数）。
    fn attach_scroll_controller(&self) {
        let weak = Rc::downgrade(&self.inner);
        let controller =
            gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::VERTICAL);
        controller.connect_scroll(move |_c, _dx, dy| {
            if let Some(inner) = weak.upgrade() {
                let view = PaneView { inner };
                view.handle_scroll(dy);
            }
            glib::Propagation::Stop
        });
        self.widget().add_controller(controller);
    }

    /// W21 测试钩子：模拟一次滚轮（与生产 EventControllerScroll 同一函数）。
    pub fn test_emit_scroll(&self, delta_y: f64) {
        self.handle_scroll(delta_y);
    }

    /// W21 测试钩子：当前 reply_state 是否在 alt-screen。
    pub fn test_alternate_screen(&self) -> bool {
        self.inner.reply_state.borrow().alternate_screen
    }

    /// 是否已经用完整快照播种。
    pub fn is_seeded(&self) -> bool {
        self.inner.seeded.get()
    }

    /// tmux/SSH 镜像模式：解析器查询应答由 tmux `refresh-client -r` 代答。
    pub fn is_tmux_mirror(&self) -> bool {
        self.inner.is_tmux_mirror.get()
    }

    pub fn pane_id(&self) -> u32 {
        self.inner.pane_id.get()
    }

    pub fn set_pane_id(&self, id: u32) {
        self.inner.pane_id.set(id);
    }

    pub fn widget(&self) -> gtk4::Widget {
        self.inner.renderer.widget()
    }

    pub fn terminal(&self) -> &vte4::Terminal {
        self.inner.renderer.terminal()
    }

    /// 按后端报告的 pane 字符格尺寸 resize 模型。
    pub fn resize_to(&self, cols: u16, rows: u16) {
        self.ensure_grid_size(cols, rows);
    }

    /// 网格已是该尺寸则跳过，避免 16ms 轮询里反复 set_size。
    pub fn ensure_grid_size(&self, cols: u16, rows: u16) {
        let cols = cols.max(2);
        let rows = rows.max(1);
        if self.inner.grid_cols.get() == cols && self.inner.grid_rows.get() == rows {
            return;
        }
        // 先把旧宽度上尚未 flush 的 CUP 画完，再改网格。
        // 否则 25ms 合并窗口会把 htop 的旧帧喂进新列数（表头叠表）。
        if let Some(id) = self.inner.feed_flush_source.borrow_mut().take() {
            id.remove();
        }
        flush_pending_feed(&self.inner);
        self.inner.grid_cols.set(cols);
        self.inner.grid_rows.set(rows);
        self.inner
            .renderer
            .terminal()
            .set_size(cols as i64, rows as i64);
        // Surface 在 resize 时必须保留已有像素状态。tmux 与 Herdr 都会在
        // 新尺寸下继续发送 CUP/diff/full ANSI；主动 reset 会把隐藏 tab 的
        // 唯一 VT 清空，并让“切得动但内容没了”依赖下一次偶然重播才能恢复。
        // 这里只调整两个终端模型的网格，不清屏、不改变 seed 状态。
        self.inner
            .reply_state
            .borrow_mut()
            .resize(cols as usize, rows as usize);
    }

    /// 输出事件入队合并（同一 pane 短窗口内一次 feed）。
    pub fn feed_output(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.inner.pending_feed.borrow_mut().extend_from_slice(data);
        self.schedule_feed_flush();
    }

    /// full 帧（完整状态快照）入队：先清屏再喂 full，避免 resize 触发
    /// 的 full 帧（herdr 不带 ESC[2J）叠加在旧内容上导致行错位
    /// （matrix ctrl_l_stays_clear 偶发失败根因）。不 reset scrollback。
    pub fn feed_full(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.inner
            .pending_feed
            .borrow_mut()
            .extend_from_slice(b"\x1b[2J\x1b[H");
        self.inner.pending_feed.borrow_mut().extend_from_slice(data);
        self.schedule_feed_flush();
    }

    /// attach 前历史按行写进 VTE scrollback，不 reset，也不把历史反喂成
    /// reply_state 的 VT 流。先刷完 live lane，再保存当前可见网格并重建
    /// VTE；reply_state 只用按行 prepend 保持自身 scrollback 一致。首帧
    /// 尚未播种时先排队；后到的 Snapshot reset 后按 generation 重放。
    ///
    /// alternate screen（TUI）上禁止 ESC[2J 回放：会清掉当前 Cursor/htop 屏。
    pub fn prepend_history(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        if !history_replay_allowed(self.inner.reply_state.borrow().alternate_screen) {
            return;
        }
        {
            let mut batches = self.inner.history_batches.borrow_mut();
            // 同一 generation 的多次 resync 可能重发相同 PaneHistory
            // （第二次 resync 覆盖 Surface 后重新 capture 历史）；内容
            // 相同则跳过，避免 scrollback 翻倍。
            if batches.last().is_some_and(|last| last.as_slice() == data) {
                return;
            }
            batches.push(data.to_vec());
        }
        if !self.inner.seeded.get() {
            return;
        }
        flush_unapplied_history(&self.inner);
    }

    /// 新一轮 attach 开始：清掉上一轮保留的历史批次。
    ///
    /// reattach 会重新 capture 同一份 pane 历史；若不清掉旧批次，
    /// 后到的权威 Snapshot reset 会把旧历史再重放一遍，造成重复历史
    /// 与无界增长。`Connecting` 事件先于任何新 generation 的
    /// PaneHistory/PaneSnapshot 到达，因此在这里重置是安全的。
    pub fn begin_attach_generation(&self) {
        self.inner.history_batches.borrow_mut().clear();
        self.inner.history_applied.set(0);
    }

    /// 调度合并 flush（25ms 窗口）。
    fn schedule_feed_flush(&self) {
        if self.inner.feed_flush_source.borrow().is_none() {
            let weak = Rc::downgrade(&self.inner);
            let id = glib::timeout_add_local(
                std::time::Duration::from_millis(FEED_COALESCE_MS),
                move || {
                    if let Some(inner) = weak.upgrade() {
                        flush_pending_feed(&inner);
                    }
                    glib::ControlFlow::Break
                },
            );
            *self.inner.feed_flush_source.borrow_mut() = Some(id);
        }
    }

    /// 立即把合并缓冲写入 VTE（测试 / 窗口关闭用）。
    pub fn flush_pending_feed(&self) {
        flush_pending_feed(&self.inner);
    }

    /// 用完整快照播种（首次挂载 pane 时）。播种前先按 pane 尺寸 resize。
    pub fn seed_snapshot(&self, data: &[u8], cols: u16, rows: u16) {
        // 播种会丢弃尚未 flush 的增量，避免和快照叠在一起。
        if let Some(id) = self.inner.feed_flush_source.borrow_mut().take() {
            id.remove();
        }
        self.inner.pending_feed.borrow_mut().clear();
        if cols >= 2 || rows >= 1 {
            self.resize_to(cols, rows);
        }
        // 不用 vte reset：reset 同步执行但不会清空 VTE 输入队列，旧
        // generation 已入队的字节会在 reset 之后继续渲染，把旧内容画进
        // 新画面（历史翻倍/光标错乱）。改用队列内的清屏序列：旧的
        // pending feed 先被处理，然后 `2J` 清当前屏、`3J` 清 scrollback，
        // 最后才轮到快照字节。
        //
        // scrollback 已应用过本 generation 历史时（history_applied > 0）
        // 不再清 scrollback：同一 generation 的 resize/resync 只是替换
        // 当前屏，历史必须保留且不能重放翻倍。
        let clear_scrollback = self.inner.history_applied.get() == 0;
        {
            let mut trace = self.inner.render_trace.borrow_mut();
            trace.resets += 1;
            trace.seeds += 1;
        }
        *self.inner.reply_state.borrow_mut() =
            TerminalState::new(cols.max(2) as usize, rows.max(1) as usize);
        let mut prefix = b"\x1b[2J\x1b[H".to_vec();
        if clear_scrollback {
            prefix.extend_from_slice(b"\x1b[3J");
        }
        with_remote_feed(&self.inner, || {
            self.inner.renderer.terminal().feed(&prefix);
            feed_reply_state(&self.inner, &prefix);
            if !data.is_empty() {
                self.inner.renderer.terminal().feed(data);
                feed_reply_state(&self.inner, data);
            }
            apply_mirror_mouse_policy(&self.inner);
        });
        if !data.is_empty() {
            // reattach 时新 control client 的尺寸校准（refresh-client -C）
            // 晚于首屏 capture：快照网格行数可能大于 VTE 当前可见行数，
            // 快照末尾的 CUP 因此越界，光标落在错误位置。shell 随后因
            // resize 重绘 prompt（`\r\r ESC M ESC M ESC[J`）会从错误位置
            // 清掉整屏，刚 seed 的历史 token 随之丢失。把光标锚定到
            // buffer 末尾，让重绘只影响底部几行。
            anchor_snapshot_cursor(&self.inner, data);
        }
        self.inner.seeded.set(true);
        // 历史先于 seed 到达（或上一轮 seed 时有未应用批次）时补放。
        flush_unapplied_history(&self.inner);
    }

    /// attach 快照播种：不 reset、不 dump，直接把 capture-pane 原始字节
    /// 喂进 VTE（1820.log 白屏修复；live 路径禁止 visible_ansi → reset）。
    pub fn seed_raw(&self, data: &[u8], cols: u16, rows: u16) {
        tracing::info!(
            target: "muxterm::surface",
            pane = self.inner.pane_id.get(),
            bytes = data.len(),
            cols = cols,
            rows = rows,
            "seed_raw"
        );
        if let Some(id) = self.inner.feed_flush_source.borrow_mut().take() {
            id.remove();
        }
        self.inner.pending_feed.borrow_mut().clear();
        if cols >= 2 || rows >= 1 {
            self.resize_to(cols, rows);
        }
        // seed_raw 不 reset：历史 replay 必须先入队（`\x1b[H\x1b[2J` 会把
        // 历史推进 scrollback 并重画 overlay 屏），快照最后入队覆盖屏幕，
        // scrollback 里的历史才会保留。若快照先入队，replay 的 `\x1b[2J`
        // 会清掉刚画好的快照屏。
        flush_unapplied_history(&self.inner);
        if !data.is_empty() {
            with_remote_feed(&self.inner, || {
                self.inner.renderer.terminal().feed(data);
                feed_reply_state(&self.inner, data);
                apply_mirror_mouse_policy(&self.inner);
            });
            // 与 seed_snapshot 相同：快照网格可能大于 VTE 可见行数，
            // 末尾 CUP 越界会让光标/viewport 停在错误位置，shell 重绘或
            // 测试断言都看不到尾标。锚定到 buffer 末尾。
            anchor_snapshot_cursor(&self.inner, data);
        }
        self.inner.render_trace.borrow_mut().seeds += 1;
        self.inner.seeded.set(true);
    }

    /// 渲染痕迹（测试断言不刷屏）。
    pub fn render_trace(&self) -> RenderTrace {
        *self.inner.render_trace.borrow()
    }

    /// 测试/诊断：Surface 自己记录的字符格尺寸。
    pub fn grid_size(&self) -> (u16, u16) {
        (self.inner.grid_cols.get(), self.inner.grid_rows.get())
    }

    /// 测试/诊断：尚未 flush 到 VTE 的原始字节数。
    pub fn pending_feed_len(&self) -> usize {
        self.inner.pending_feed.borrow().len()
    }

    /// 清空渲染痕迹（测试在独立场景前调用）。
    pub fn clear_render_trace(&self) {
        *self.inner.render_trace.borrow_mut() = RenderTrace::default();
    }

    /// 注入 URL 打开出口（测试用 Recording，生产接 GTK）。
    pub fn set_url_opener(&self, opener: Rc<dyn UrlOpener>) {
        *self.inner.url_opener.borrow_mut() = Some(opener);
    }

    /// 点击坐标 → 打开 URL：先 OSC 8 hyperlink，再正则 match。
    /// 测试钩子 `test_open_url_at` 与真实 GestureClick 走同一函数。
    pub fn open_url_at(&self, x: f64, y: f64) {
        let term = self.inner.renderer.terminal();
        let uri = term
            .check_hyperlink_at(x, y)
            .map(|s| s.to_string())
            .or_else(|| {
                let (m, _tag) = term.check_match_at(x, y);
                m.map(|s| s.to_string())
            });
        if let Some(uri) = uri {
            if let Some(opener) = self.inner.url_opener.borrow().clone() {
                opener.open(&uri);
            }
        }
    }

    /// 测试用：直接调 open_url_at（与真实点击同一路径）。
    #[cfg(test)]
    pub fn test_open_url_at(&self, x: f64, y: f64) {
        self.open_url_at(x, y);
    }

    /// 取出待回写 shell 的查询应答字节。
    pub fn take_replies(&self) -> Vec<u8> {
        std::mem::take(&mut self.inner.pending_replies.borrow_mut())
    }

    /// 用户按键 → 回调（由 window 转发到 FFI send_input）。
    ///
    /// VTE 无 PTY 时会把 OSC/CSI 应答也走 `commit`。tmux 镜像下必须丢掉，
    /// 否则 `git lg` 的 `10;rgb:...` 会经 send-keys 泄漏进 shell。
    pub fn connect_input<F: Fn(u32, &[u8]) + 'static>(&self, f: F) {
        let pid = self.inner.pane_id.clone();
        let feeding = self.inner.is_feeding_remote_output.clone();
        let mirror = self.inner.is_tmux_mirror.clone();
        let weak = Rc::downgrade(&self.inner);
        *self.inner.input_cb.borrow_mut() = Some(Box::new(f));
        self.inner
            .renderer
            .terminal()
            .connect_commit(move |_term, text, _len| {
                let data = text.as_bytes();
                if !should_forward_mixed_input(feeding.get(), mirror.get(), data) {
                    return;
                }
                if let Some(inner) = weak.upgrade() {
                    if let Some(cb) = inner.input_cb.borrow().as_ref() {
                        cb(pid.get(), data);
                    }
                }
            });
    }

    /// 测试用：直接触发输入回调（与 VTE commit 同一路径）。
    pub fn test_emit_input(&self, data: &[u8]) {
        if let Some(cb) = self.inner.input_cb.borrow().as_ref() {
            cb(self.inner.pane_id.get(), data);
        }
    }

    /// 测试用：发出 VTE 的 `commit` 信号，覆盖生产 `connect_commit` 过滤与
    /// 输入回调，不直接调用 Runtime。用于验证逐字输入和 Enter 的真实路由。
    pub fn test_emit_commit(&self, text: &str) {
        self.inner
            .renderer
            .terminal()
            .emit_by_name::<()>("commit", &[&text, &(text.len() as u32)]);
    }

    pub fn copy_clipboard(&self) {
        self.inner
            .renderer
            .terminal()
            .copy_clipboard_format(vte4::Format::Text);
    }

    /// VTE 无 PTY 时 `paste_clipboard` 会走 commit → 空的 `ESC[200~ESC[201~`。
    /// 由窗口读 GTK 剪贴板再 `send_input`。
    pub fn bracketed_paste(&self) -> bool {
        self.inner.reply_state.borrow().bracketed_paste
    }

    /// 运行期切换主题（VTE 调色板 + 重绘）。
    pub fn apply_theme(&self, theme: &Theme) {
        self.inner.renderer.apply_theme(theme);
    }

    /// 运行期修改字号（保留 family；VTE 重算字符格）。
    pub fn set_font_size(&self, size: f32) {
        let settings = FontSettings {
            size,
            ..FontSettings::default()
        };
        self.inner.renderer.apply_font(&settings);
    }

    /// 运行期修改字体（family + size）。
    pub fn set_font(&self, font: &FontSettings) {
        self.inner.renderer.apply_font(font);
    }

    pub fn grab_focus(&self) {
        self.inner.renderer.terminal().grab_focus();
    }

    /// 测试用：VTE 当前 viewport 纯文本。
    pub fn visible_text(&self) -> String {
        self.inner
            .renderer
            .terminal()
            .text_format(vte4::Format::Text)
            .map(|s| s.to_string())
            .unwrap_or_default()
    }

    /// 测试/诊断：VTE scrollback + 当前屏的完整文本。
    pub fn buffer_text(&self) -> String {
        let terminal = self.inner.renderer.terminal();
        let Some(adjustment) = terminal.vadjustment() else {
            return self.visible_text();
        };
        let row_count = (adjustment.upper() - adjustment.lower()).ceil() as i64;
        let end_row = terminal.cursor_position().1;
        let start_row = end_row.saturating_sub(row_count.saturating_sub(1));
        let (text, _) = terminal.text_range_format(vte4::Format::Text, start_row, 0, end_row, -1);
        let visible = self.visible_text();
        match text {
            Some(buffer) if !buffer.is_empty() => format!("{buffer}\n{visible}"),
            _ => visible,
        }
    }

    /// 测试用：VTE 当前屏幕（不含 scrollback）纯文本。
    ///
    /// Ctrl-L 后旧内容会留在 VTE scrollback 里，但当前屏幕必须已清空；
    /// 断言“当前屏不可见 BEFORE”必须只看屏幕，不能把 scrollback 算进去。
    /// vte4 的 text_format() 返回可见屏幕（最后 rows 行，不含 scrollback）；
    /// text_range_format(0,0,rows-1,-1) 返回的是 buffer 前 rows 行（scrollback
    /// 区域），会误把历史 prompt 当“当前屏”（matrix ctrl_l 误报根因）。
    pub fn screen_text(&self) -> String {
        self.visible_text()
    }

    /// 测试用：VTE 光标所在行（0 起；最后一行 = rows-1）。
    pub fn cursor_row(&self) -> i64 {
        self.inner.renderer.terminal().cursor_position().1
    }

    /// 测试用：VTE 屏幕行数。
    pub fn screen_rows(&self) -> i64 {
        self.inner.renderer.terminal().row_count()
    }
}

fn flush_pending_feed(inner: &PaneViewInner) {
    *inner.feed_flush_source.borrow_mut() = None;
    let data = std::mem::take(&mut *inner.pending_feed.borrow_mut());
    if data.is_empty() {
        return;
    }
    // Surface：合并缓冲里的原始 pane 字节按序 feed，永不 reset 追帧。
    // 半帧（1365/2730）必须都留下；CUP 风暴由 VTE 自己演到末帧。
    with_remote_feed(inner, || {
        inner.renderer.terminal().feed(&data);
        feed_reply_state(inner, &data);
        apply_mirror_mouse_policy(inner);
    });
    let mut trace = inner.render_trace.borrow_mut();
    trace.feeds += 1;
    trace.bytes_fed += data.len();
    inner.seeded.set(true);
    drop(trace);
    flush_unapplied_history(inner);
}

/// 快照网格行数：capture 网格是行以 `\r\n` join 的纯文本，`\n` 计数 + 1
/// 即物理行数（CUP/模式序列里不会有 `\n`）。
fn snapshot_grid_rows(data: &[u8]) -> usize {
    data.iter().filter(|&&byte| byte == b'\n').count() + 1
}

/// 快照网格大于 VTE 可见行数时，把光标锚定到 buffer 末尾。
///
/// 否则快照末尾 CUP 越界后光标停留在错误行，shell 的 prompt 重绘
/// （resize 触发）会从错误位置 `ESC[J` 清掉刚 seed 的内容。
fn anchor_snapshot_cursor(inner: &Rc<PaneViewInner>, data: &[u8]) {
    let terminal = inner.renderer.terminal();
    // row_count()/vadjustment 在 VTE 0.84 里受 set_size 与 widget 分配
    // 交互影响（模型 24 行时 row_count 可能 20 或 24，page_size 也可能
    // 24）。唯一可靠的是 widget 实际像素高度 ÷ 字符高。reattach 时快照
    // 网格常大于可见行数，末尾 CUP 越界后光标落在错误位置，shell 的
    // prompt 重绘（resize 触发）会从错误位置 ESC[J 清掉刚 seed 的内容。
    let char_h = terminal.char_height().max(1) as f64;
    let widget_h = terminal.height().max(0) as f64;
    let visible_rows = (widget_h / char_h).floor().max(1.0) as usize;
    let snapshot_rows = snapshot_grid_rows(data);
    if snapshot_rows > visible_rows {
        // CUP 锚到 buffer 末尾：后续 shell 重绘只影响底部几行。
        feed_direct(inner, b"\x1b[999;1H");
        // feed 是异步的，且镜像模式 scroll-on-output=false；等本批 feed
        // 处理完（下一个 idle）再把 view 钉到底部，保证 attach 后可见尾标。
        if !inner.scroll_to_bottom_pending.replace(true) {
            let weak = Rc::downgrade(inner);
            // idle（不是 timeout）：feed 的处理 idle 先注册，默认优先级
            // 下按注册顺序先跑完，这里再滚到底部才能看到新 buffer 高度。
            glib::idle_add_local(move || {
                if let Some(inner) = weak.upgrade() {
                    inner.scroll_to_bottom_pending.set(false);
                    if let Some(adj) = inner.renderer.terminal().vadjustment() {
                        let bottom = (adj.upper() - adj.page_size()).max(adj.lower());
                        adj.set_value(bottom);
                    }
                }
                glib::ControlFlow::Break
            });
        }
    }
}

fn flush_unapplied_history(inner: &PaneViewInner) {
    let start = inner.history_applied.get();
    let pending = {
        let batches = inner.history_batches.borrow();
        batches.iter().skip(start).cloned().collect::<Vec<_>>()
    };
    // prepend_history_seeded 会先 flush live lane；提前推进游标，避免该
    // flush 回调再次进入这里时重复重放同一批历史。
    inner
        .history_applied
        .set(start.saturating_add(pending.len()));
    for data in pending {
        prepend_history_seeded(inner, &data);
    }
}

fn prepend_history_seeded(inner: &PaneViewInner, data: &[u8]) {
    if !history_replay_allowed(inner.reply_state.borrow().alternate_screen) {
        return;
    }
    let lines: Vec<String> = String::from_utf8_lossy(data)
        .split('\n')
        .map(str::to_string)
        .collect();
    if lines.iter().all(|line| line.is_empty()) {
        return;
    }

    if let Some(id) = inner.feed_flush_source.borrow_mut().take() {
        id.remove();
    }
    flush_pending_feed(inner);

    let (rows, visible_overlay) = {
        let state = inner.reply_state.borrow();
        (state.rows(), state.visible_overlay_ansi())
    };
    inner.reply_state.borrow_mut().prepend_history_lines(&lines);

    let replay = history_replay_ansi(&lines, rows, &visible_overlay);
    feed_direct(inner, &replay);
}

fn feed_direct(inner: &PaneViewInner, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    with_remote_feed(inner, || {
        inner.renderer.terminal().feed(bytes);
        apply_mirror_mouse_policy(inner);
    });
    let mut trace = inner.render_trace.borrow_mut();
    trace.feeds += 1;
    trace.bytes_fed += bytes.len();
}

/// 把按行历史滚入 native VT scrollback，再覆盖恢复原来的可见网格。
///
/// `ESC[2J` 只清当前屏，不清 scrollback；这里刻意不用 RIS/reset。调用方
/// 必须把结果只喂给 native Surface，不能再让 reply_state 解析一次。
/// alternate screen 上不得调用（见 `history_replay_allowed`）。
fn history_replay_allowed(alternate_screen: bool) -> bool {
    !alternate_screen
}

fn history_replay_ansi(lines: &[String], rows: usize, visible_overlay: &[u8]) -> Vec<u8> {
    let text_bytes = lines.iter().map(String::len).sum::<usize>();
    let mut replay = Vec::with_capacity(
        b"\x1b[H\x1b[2J".len()
            + text_bytes
            + (lines.len() + rows) * b"\r\n".len()
            + visible_overlay.len(),
    );
    replay.extend_from_slice(b"\x1b[H\x1b[2J");
    for line in lines {
        replay.extend_from_slice(line.as_bytes());
        replay.extend_from_slice(b"\r\n");
    }
    for _ in 0..rows {
        replay.extend_from_slice(b"\r\n");
    }
    replay.extend_from_slice(visible_overlay);
    replay
}

fn apply_mirror_mouse_policy(inner: &PaneViewInner) {
    if !inner.is_tmux_mirror.get() {
        return;
    }
    inner.renderer.terminal().feed(DISABLE_MOUSE_TRACKING);
    feed_reply_state(inner, DISABLE_MOUSE_TRACKING);
}

fn with_remote_feed(inner: &PaneViewInner, f: impl FnOnce()) {
    inner.is_feeding_remote_output.set(true);
    f();
    inner.is_feeding_remote_output.set(false);
}

fn feed_reply_state(inner: &PaneViewInner, data: &[u8]) {
    if data.is_empty() {
        return;
    }
    let mut state = inner.reply_state.borrow_mut();
    // W19e：emulate 热路径不许 panic 穿 glib；失败则重建干净状态，
    // 不留半坏 grid（grid 与 grid_soft_wrapped 可能已不同步）。
    let fed = crate::platform::linux::fault_gtk::run("pane_view.feed_reply_state", || {
        state.feed(data);
    });
    if fed.is_none() {
        let (cols, rows) = (state.cols(), state.rows());
        *state = TerminalState::new(cols.max(1), rows.max(1));
        return;
    }
    let replies = state.take_reply();
    if should_forward_parser_response(true, inner.is_tmux_mirror.get()) && !replies.is_empty() {
        inner
            .pending_replies
            .borrow_mut()
            .extend_from_slice(&replies);
    }
}

/// 是否把解析器查询应答回写给后端（local shell 才回写）。
///
/// tmux 镜像在 feed 远端输出期间生成的应答一律丢弃（git lg 泄漏根因）。
pub fn should_forward_replies(is_tmux_mirror: bool, replies: &[u8]) -> bool {
    !replies.is_empty() && should_forward_parser_response(true, is_tmux_mirror)
}

/// 便于在闭包里共享的 PaneView 句柄。
pub type PaneViewRc = Rc<PaneView>;

/// 主题色转 hex（`rrggbb`，供 tmux refresh-client -r 上报）。
pub fn rgb_hex(c: Rgb) -> String {
    format!("{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_grid_rows_counts_physical_lines() {
        assert_eq!(snapshot_grid_rows(b"a\r\nb\r\nc"), 3);
        assert_eq!(snapshot_grid_rows(b""), 1);
        assert_eq!(snapshot_grid_rows(b"\x1b[20;3H"), 1);
        assert_eq!(snapshot_grid_rows(b"\r\n\r\n\r\n"), 4);
    }

    #[test]
    fn coalesce_window_is_25ms() {
        assert_eq!(FEED_COALESCE_MS, 25);
    }

    #[test]
    fn mirror_mode_drops_parser_query_replies() {
        assert!(!should_forward_replies(true, b"\x1b]11;?\x07"));
        assert!(should_forward_replies(false, b"\x1b]11;?\x07"));
        assert!(!should_forward_replies(false, b""));
    }

    #[test]
    fn tmux_commit_drops_gitlg_osc_color_reply() {
        let leaked = b"\x1b]10;rgb:4c4c/4f4f/6969\x07";
        assert!(!should_forward_mixed_input(true, true, leaked));
        assert!(!should_forward_mixed_input(false, true, leaked));
        assert!(should_forward_mixed_input(false, true, b"ls\n"));
    }

    #[test]
    fn history_replay_is_noop_on_alternate_screen() {
        assert!(history_replay_allowed(false));
        assert!(!history_replay_allowed(true));
    }

    #[test]
    fn history_replay_scrolls_rows_without_reset_and_restores_visible_grid() {
        let mut current = TerminalState::new(20, 3);
        current.feed(b"TAIL_VISIBLE");
        let before = current.snapshot();
        let overlay = current.visible_overlay_ansi();
        let lines = vec!["HIST_OFFSCREEN".into(), String::new(), "pad-01".into()];

        let replay = history_replay_ansi(&lines, current.rows(), &overlay);
        assert!(!replay.windows(2).any(|bytes| bytes == b"\x1bc"));
        assert!(replay.starts_with(b"\x1b[H\x1b[2JHIST_OFFSCREEN\r\n\r\npad-01\r\n"));

        current.feed(&replay);
        assert_eq!(current.snapshot(), before, "当前可见网格必须原样恢复");
        assert!(
            current
                .search("HIST_OFFSCREEN")
                .iter()
                .any(|(_, line)| line.contains("HIST_OFFSCREEN")),
            "历史 token 必须进入 native VT scrollback"
        );
    }
}
