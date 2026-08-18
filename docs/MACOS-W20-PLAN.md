# MACOS-W20-PLAN.md — agent 半截画面 / emulate panic / 主题跟终端色（交给 Codex）

> 日期：2026-08-18（本机 `2026-08-18T11:22:15+08:00`）
> 工作目录：`/Users/wlz/Developer/self/muxterm`
> 分支：`feature/quickconnect-attach-ui`（**不 push**）
> 证据：`test-2026-0818-1114.log` + 用户 dogfood
> 先读：`docs/MACOS-W19-PLAN.md`、`src/core/protocol/terminal/emulate.rs`（`resize` / `linefeed_inner`）、`src/core/workspace/pane_buf.rs`、`TerminalView.setThemeColors`
>
> **Grok 写测试和本计划。你实现，直到下列命令真实退出码 0。**
>
> 禁止改断言、identifier、token 来「绿」。禁止默认 tmux `kill-server`。测试只用 `-L muxterm-test-*`。
> **不 push。** commit 英文 `type(scope):`，无 Co-authored-by。

---

## 0. 用户看到的问题

1. **Codex agent 画面没加载完**：只有一段话 + 输入框，正文/状态行缺失。
2. **主题切换不影响终端配色**，还是黑底。用户要终端跟着 light/dark 走，不是只改 chrome。
3. **stderr panic**（`./build/macos/muxterm gui --debug`）：

   ```
   panicked at src/core/protocol/terminal/emulate.rs:718:40:
   insertion index (is 26) should be <= len (is 15)
   ...
   removal index (is 11) should be < len (is 11)
   DEBUG new-window cmd="new-window -t $0"
   ```

`muxterm_poll_events` 整函数 `catch_unwind`。emulate panic → 返回 -1 → **整批 `%output` 不发给 SwiftTerm**。所以 agent 只剩半截。这不是「渲染慢」，是事件被吞。

---

## 1. 根因（不要猜别的）

`TerminalState::resize` **只改 `grid`，不改 `grid_soft_wrapped`**。

`PaneBuf::feed` 每次输出都 `resize(tmux_cols, tmux_rows)` 再 `feed`。窗口变高（15→27）后：

- `grid.len() == 27` = `rows()`
- `grid_soft_wrapped.len()` 仍是 **15**
- agent 设部分 DECSTBM（不是整屏）再在底行 LF
- `linefeed_inner`：`grid.remove(top)` 成功，`grid_soft_wrapped.insert(bottom=26)` 在 len=15 上 **panic**

同一文件 `insert_blank_lines` / `delete_lines` / `scroll_up_n` 也只动 `grid`。

主题：`setThemeColors(fgHex, bgHex)` **丢掉入参**，永远写 `MuxtermTerminalColors.foregroundHex/backgroundHex`（`cdd6f4`/`1e1e2e`）。`applyTheme` 也把 OSC 报成这两色。所以终端永远黑。

**产品改口（覆盖 W19「OSC 固定深色」）：** MainWindow 主题路径上，SwiftTerm 默认色 **和** OSC 10/11 都用 `theme.palette`。light = `000000`/`ffffff`，dark = `cdd6f4`/`1e1e2e`。

裸 `MuxTerminalView` 无主题时仍深色（`AgentRenderE2ETests.testReportedOscColors…` 保持绿，不要改那条亮度方向）。

---

## 2. 顺序

1. 新测试能编译、能跑红。
2. **W20-A** `emulate.rs`：`resize_*` / `insert_and_delete_lines_*` 绿（同步 `grid_soft_wrapped`；LF/IL/DL 下标 clamp 到 `min(grid.len(), soft.len())`，禁止 panic）。
3. **W20-B** `AgentRenderE2ETests.testDecstbmFrameAfterWindowGrowReachesSwiftTerm` 绿。
4. **W20-C** `ThemeToggleE2ETests` 绿（`testToggleThemeChangesChromeAndTerminalPalette` + `testSetThemeColorsUsesProvidedHex`）。`setThemeColors` 必须用传入 hex；`applyTheme` 必须 `theme.palette` 而不是写死 dark。
5. 回归：`cargo test --lib emulate` 相关、`MuxtermAppE2ETests`、`AgentRenderE2ETests` 旧用例、`CmdEnterKeyE2ETests`。

每步一英文 commit。

---

## 3. 已落地的测试（RED → 你实现）

| 文件 | 硬断言 |
|---|---|
| `emulate.rs` `resize_grow_then_partial_decstbm_lf_does_not_panic` | 15→27 后 `soft_wrap_row_count()==rows()`；DECSTBM `2;27` + LF 不 panic，snap 含 `PROMPT` |
| `resize_shrink_then_decstbm_lf_does_not_panic` | 27→11 后仍同步；含 `FOOT` |
| `insert_and_delete_lines_keep_soft_wrap_len` | CSI L/M 后仍同步 |
| `resize_expands_extends_scroll_region_to_new_height` | 现有测试加了 soft-wrap 同行数 |
| `tests/scripts/agent_decstbm_frame.py` + `testDecstbmFrameAfterWindowGrowReachesSwiftTerm` | 先矮后高窗口，tmux capture **和** SwiftTerm 都有 `FULL_AGENT_FRAME` + `AGENT_TOP` |
| `ThemeToggleE2ETests` | 切换后 `testThemeHexColors() == currentTheme().palette`；再切一次背景必须变；`setThemeColors` 浅色/深色 hex 真写进 native |

不要把 ThemeToggle 改回「OSC 永远深色」。那是上一轮契约，用户已经否掉。

---

## 4. 实现要点

### 4.1 emulate.rs

`resize` 里对 `grid_soft_wrapped` 做与 `grid` 相同的 grow/truncate/drain。任何 `grid.remove/insert` 的滚动路径（`linefeed_inner`、`scroll_up_n`、`scroll_down_n`、`insert_blank_lines`、`delete_lines`）必须同时改 `grid_soft_wrapped`，且 **index 先 clamp**：

```rust
let rows = self.grid.len();
if self.grid_soft_wrapped.len() != rows {
    self.grid_soft_wrapped.resize(rows, false);
}
let top = self.scroll_top.min(rows.saturating_sub(1));
let bottom = self.scroll_bottom.min(rows.saturating_sub(1)).max(top);
```

禁止 unwrap/panic。`soft_wrap_row_count` 测试钩子保留。

### 4.2 主题

- `MuxTerminalView.setThemeColors`：用 `fgHex`/`bgHex` 设 `nativeForegroundColor` / `nativeBackgroundColor`，不要 `_ = fgHex`。
- `MainWindow.applyTheme`：`terminalManager.applyTheme` 和 `reportAllPaneColours` 都传 `theme.palette`（即 `MuxtermTerminalColors.activePalette`，已经在函数开头赋过）。
- 不要改坏 `AgentRenderE2ETests` 里**未** `setThemeColors` 的默认深色断言。

### 4.3 不要做的

- 不要为了绿去 skip DECSTBM 测试。
- 不要 `kill-server` 默认 socket。
- 不要动 W19 已绿的 session 列表 / connect progress / attention overlay，除非回归红了再修。

---

## 5. 验证命令（全部退出码 0）

```bash
cargo test --no-default-features --features tui --lib resize_grow_then_partial_decstbm_lf_does_not_panic -- --nocapture
cargo test --no-default-features --features tui --lib resize_shrink_then_decstbm_lf_does_not_panic
cargo test --no-default-features --features tui --lib insert_and_delete_lines_keep_soft_wrap_len
cd src/platform/macos && swift test --disable-swift-testing --filter ThemeToggleE2ETests
cd src/platform/macos && swift test --disable-swift-testing --filter testDecstbmFrameAfterWindowGrowReachesSwiftTerm
cd src/platform/macos && swift test --disable-swift-testing --filter AgentRenderE2ETests
cargo fmt --all -- --check
```
