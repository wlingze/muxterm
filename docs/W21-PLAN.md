# W21-PLAN.md — 滚轮：shell 看历史，agent 交给应用

> 日期：2026-08-17（`2026-08-17T19:21:51+08:00`）
> 工作目录：`/home/wlz/Developer/self/muxterm`
> 分支：`feature/runtime/support_herdr`
> 先读：[`W19-PLAN.md`](W19-PLAN.md)（必须先绿）→ 本文件 → [`SURFACE.md`](SURFACE.md) §5.3 滚轮 → `renderer.rs` `apply_mirror_policy` → `pane_view.rs`
> 对照（只读）：iTerm2 `KEY_ALLOW_ALTERNATE_MOUSE_SCROLL` / `alternateMouseScroll`（alt-screen 滚轮转方向键）；VTE `enable-fallback-scrolling` 默认 **true**。
>
> **你是实现 agent。W19 门禁未绿不要开始。W21 在 W20 之前。先红测试再实现。禁止改断言 / widget_name。禁止 `#[ignore]`。禁止 `git add -A`。禁止 Co-authored-by。禁止 push。禁止 `visible_ansi` → `vte.reset`。禁止用 replica dump 冒充滚动。`d1181679` 必须仍是祖先。**

用户 2026-08-17 在 Mini 上 dogfood：tmux attach 后，**shell 里上下滚、agent（Codex TUI）里上下滚都有问题**。现有 e2e 绿是因为测的是 `vadjustment.set_value(0)`，**从来没有模拟滚轮**。

---

## 0. 根因（不要猜成「没有 scrollback 行数」）

`PaneView::new(..., is_tmux_mirror=true, ...)` 会 `apply_mirror_policy`：

```rust
self.terminal.set_enable_fallback_scrolling(false); // ← 滚轮不滚 VTE
self.terminal.set_scroll_on_output(false);          // 这个要留（scroll lock / TUI）
self.terminal.set_scroll_on_insert(false);
```

镜像 VTE **没有 PTY**。同时又在每次 feed 后灌 `DISABLE_MOUSE_TRACKING`（为了能划词复制）。于是：

| 场景 | 用户期望 | 现在 |
|---|---|---|
| tmux 里的 shell（主屏） | 滚轮走 VTE scrollback，看见历史 | fallback=false → 滚轮谁也不收 |
| tmux 里的 agent / htop / less（alt-screen） | 滚轮给应用（方向键或鼠标） | 鼠标报告被关掉，fallback 也关 → 滚轮谁也不收 |

`linux_scroll_lock_e2e` / `linux_render_e2e` 的 `scroll_up_reveals_vte_scrollback` 直接改 adjustment，所以一直绿。

`scroll_on_output=false` **不要打开**：打开会破坏 W17b scroll lock，也会把 htop 表头卷走。

---

## 1. 产品行为（锁死，对齐 iTerm2 alternate-mouse-scroll）

纯函数（无 GTK），放 `src/platform/linux/scroll_policy.rs`（或 `src/core/protocol/terminal/scroll_policy.rs`）：

```rust
pub enum WheelAction {
    /// 主屏：动 VTE 视口，不 send-keys。
    ScrollHistory { lines: i32 },
    /// alt-screen：发给 pane 的 CSI 方向键。
    SendToApp { bytes: Vec<u8> },
}

/// `delta_y < 0` = 用户想看上面（历史更早 / Up）。
/// 每「格」3 行。`delta_y == 0` → None。
pub fn wheel_action(alternate_screen: bool, delta_y: f64) -> Option<WheelAction>
```

- `alternate_screen == false` → `ScrollHistory { lines: ±3 * notches }`
- `alternate_screen == true` → `SendToApp`：向上 `ESC [ A` × 行数，向下 `ESC [ B` × 行数（CSI CUU/CUD，不是 `ESC O A`）
- `alternate_screen` 读 `TerminalState.alternate_screen`（CSI `?1049h/l` 已经在 emulate 里）

GTK：`PaneView` 上挂 `EventControllerScroll`（垂直）。`connect_scroll` 调 `wheel_action`，然后：

- `ScrollHistory`：改 `vadjustment`（`value + lines * step`，clamp）。**不要** `vte.reset`，不要 dump replica。
- `SendToApp`：走现有 `input_cb`（和键盘同一条，最终 `WriteRaw` / `send-keys -H`）。

`apply_mirror_policy`：**删掉** `set_enable_fallback_scrolling(false)`。默认 true 留给非我们控制器的路径；我们自己的 controller 在 `ScrollHistory` 时 `Propagation::Stop`，避免和 VTE 双滚。alt-screen 也 Stop（我们已经 SendToApp）。

本地 shell（`is_tmux_mirror=false`）同样挂这个控制器：主屏滚历史，alt-screen 发给 PTY/input_cb。不要只修 tmux。

Herdr `AttachScroll` 本轮 **不做**（observe 还没发这条 client 消息）。Herdr 格现在 `uses_tmux()==false`，会走同一套 VTE 滚轮；够用就先这样。

---

## 2. 测试（先红）

### W21a 纯逻辑

`wheel_action(false, -1.0)` → `ScrollHistory { lines: -3 }`（或 notches*3，常量写死并断言）。
`wheel_action(true, -1.0)` → `SendToApp` 的 bytes **以** `\x1b[A` 开头，且出现次数 = 行数。
`wheel_action(true, 1.0)` → `\x1b[B`。
`wheel_action(_, 0.0)` → `None`。

### W21b `PaneView`（普通 gtk Window，不要 AppWindow）

`is_tmux_mirror=true`，`set_scrollback_lines(1000)`。

1. 主屏：feed `line-0` … `line-199` 各 `\r\n`。调用 **生产** `test_emit_scroll(-1.0)`（必须和 EventControllerScroll 同一函数，禁止测试里 `adj.set_value`）。可见文本含 `line-0`，且不含 `line-199`（或 adjustment 明显离开底部）。
2. 再 `test_emit_scroll` 滚回底部：含 `line-199`。

### W21c alt-screen 把滚轮送给应用

同一 PaneView，`connect_input` 收到的字节收集到 `Rc<RefCell<Vec<u8>>>`。

feed `\x1b[?1049h` 再 `test_emit_scroll(-1.0)`。callback 里必须有 `\x1b[A`。**禁止**因此 `vte.reset`。

### W21d e2e（一个 AppWindow）

`tests/linux_scroll_wheel_e2e.rs`：隔离 tmux `-L muxterm-test-*`，`/bin/cat` 涂 80 行离屏 token（可复用 `build_offscreen_history`）。attach 后 `test_emit_scroll` 到顶，VTE 含历史 token。再 `send-keys` 进 alt-screen（`printf '\033[?1049h'`），`test_emit_scroll(-1)` 之后 tmux `capture-pane` 或 input 路径能证明 CSI A 出去了（钩子 `test_last_raw_input` 即可，不必解析远端 TUI）。

无 tmux / 无 DISPLAY 才 skip。不许 `#[ignore]`。

---

## 3. 不要做

- 打开 `scroll_on_output`（会破 `linux_scroll_lock_e2e`）
- 为了滚历史去 `visible_ansi` dump
- 重新打开全部鼠标报告（划词会坏；W21 只处理滚轮）
- 改 W19 emulate lockstep 的测试名
- 做 W20 面板（W21 绿了再做）
- Herdr `AttachScroll` / `herdr --remote`

---

## 4. 门禁

```bash
cargo test --lib wheel_action -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_scroll_wheel_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_scroll_lock_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_render_e2e -- --test-threads=1
```

`linux_scroll_lock_e2e` 必须仍绿：滚到顶后新输出不拽回。
