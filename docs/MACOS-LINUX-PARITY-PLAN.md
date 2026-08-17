# MACOS-LINUX-PARITY-PLAN.md — 把 Linux 前端保真到 macOS（交给 Codex）

> 日期：2026-08-17（本机 `2026-08-17T15:06:53+08:00`）
> 工作目录：`/Users/wlz/Developer/self/muxterm`
> 分支：`feature/quickconnect-attach-ui`（**不 push**）
> 证据：`test-2026-0817-1457.log`（SSH `ryzen` / `yaklang-workspace`，14:57 CST）
> 先读：`docs/MACOS-E2E-PLAN.md`、`src/platform/linux/panel_model.rs`、`src/platform/linux/quickconnect_panel.rs`、`src/platform/linux/window.rs`（`StatusBarSubscription` / `pane-cmd`）、`AGENTS.md`
>
> **测试已经写好且必须先红（Cmd-Enter 键路径除外，用户确认能用，必须保持绿）。禁止改断言、identifier、亮度阈值来「绿」。**
>
> Grok 写测试和本计划。你实现，直到下列命令真实退出码 0。
>
> **不要做大重构之外的事。** 不要动 SSH attach 已有修复、不要动 i18n `Bundle.module` 修复，除非测试证明你碰坏了。

---

## 0. 用户看到的问题

整体能用（attach / 切 pane / **Cmd-Enter 全屏**）。缺口是 Linux 已经有、macOS 还是旧样子：

1. **Cmd-P 不是三 tab 面板。** Linux：一个面板，Workspaces / Attention / Search，**Tab / Shift+Tab 循环**，共享 query。macOS：Cmd-P 只有 QuickConnect；搜索是 Cmd-Shift-F 另一个窗口；注意力是点铃铛第三个窗口。
2. **运行中的命令没有进注意力。** Linux 订阅 `muxterm.pane-cmd:%*:#{pane_current_command}`，写 `AttentionEngine.set_process_name`。macOS `STATE_STATUS_SUBSCRIPTION` 只把 `status-left/right` 填进 status bar，**丢掉 pane-cmd**。
3. **Agent 画面有时缺一块，输入光标几乎看不见。** 1457 日志在 attach 后上报：

   ```
   refresh-client -r "%0:\e]10;rgb:0000/0000/0000\e\\"
   refresh-client -r "%0:\e]11;rgb:ffff/ffff/ffff\e\\"
   ```

   这是浅色默认（黑字白底）。cursor/codex 按 OSC 10/11 画**深色输入框**，正文和 `▌` 用「默认前景」→ 黑字画在深色框上，所以**有时有字、有时没有、光标像不存在**。`MuxTerminalView` 注释写了要深色终端，但 `MuxtermTerminalColors.activePalette` 默认仍是 light。

Cmd-Enter 用户确认可用：已有 `ZoomE2ETests`（直接调 `testTogglePaneFullscreen`）+ 新的 `CmdEnterKeyE2ETests`（走 `handleKey`）。**不要改坏。**

---

## 1. 顺序（不要跳）

1. 让新测试能编译、能跑红：`swift test --disable-swift-testing --package-path src/platform/macos`。
2. **W-A 颜色/光标**：`AgentRenderE2ETests` 绿。
3. **W-B pane-cmd**：`PaneCmdE2ETests` 绿。
4. **W-C 统一面板**：`PanelModelTests` 已绿；`UnifiedPanelE2ETests` 必须绿。旧 `PanelE2ETests` 仍绿（identifier 做别名，不要删旧测试）。
5. **W-D Cmd-Enter**：`CmdEnterKeyE2ETests` + `ZoomE2ETests` 绿。
6. 回归：`MuxtermAppE2ETests` 全套、`MuxtermChromeTests`、`cargo test --no-default-features --features ffi --test macos_e2e -- --test-threads=1`。

每步一逻辑一英文 commit，`type(scope):`，无 Co-authored-by，不 `git add -A`，不 push。

---

## 2. 已落地的测试（RED → 你实现）

| 文件 | 对标 | 硬断言 |
|---|---|---|
| `Chrome/PanelModel.swift` + `ChromeTests/PanelModelTests.swift` | `panel_model.rs` | Tab 循环；query 跨 tab 保留。**模型已写好，不要改语义。** |
| `AppE2ETests/UnifiedPanelE2ETests.swift` | `linux_panel_e2e` Tab | `openQuickConnect()` 后**同一窗口**有 `muxterm.panel.tab.workspaces` / `attention` / `search`（Linux 的 `muxterm-panel-tab-*` 也算）；Tab → Attention on；Shift+Tab → Workspaces |
| `AppE2ETests/PaneCmdE2ETests.swift` | `window.rs` pane-cmd | attach `/bin/cat` 后 `testAttentionProcessNames()` 含 `"cat"` |
| `AppE2ETests/AgentRenderE2ETests.swift` | 1457.log OSC | `themeHexColors` 前景亮度 > 背景；**禁止** `000000`/`ffffff`；caret frame > 1pt 且在 bounds；erase-up 末帧 `STATUS-C`/`FOOTER-C` 无 `STATUS-A`；OnePaneCat attach 后 caret 可见 |
| `AppE2ETests/CmdEnterKeyE2ETests.swift` | 用户路径 | `testDispatchKeyEvent(Cmd-Enter)` → tmux `window_zoomed_flag=1` **且** GUI leaf==1 |

不要改这些 identifier / token / 亮度方向。

---

## 3. 实现要点

### 3.1 Agent 颜色 + 光标（先做，用户天天撞）

对照 `src/platform/macos/Chrome/FlatChrome.swift` 注释：codex/cursor 输入框是深色，OSC 10 必须是浅字（`cdd6f4`），OSC 11 深底（`1e1e2e`）。

- `MuxTerminalView` 初始化 / `themeHexColors()` / `reportPaneColoursIfNeeded` 必须上报**深色终端色板**，不能跟 AppKit 浅色 chrome 绑死。
- **不要削弱** `testDefaultTerminalColorsAreDarkOnLight`：那是 light **常量**的亮度方向，不是「终端默认也要浅色」。
- 光标：attach 后活动 `MuxTerminalView` 必须是 first responder，SwiftTerm `caretView` 留在 hierarchy（见 checkout 里 `AppleTerminalView.updateCursorPosition`：`cursorHidden` 或 y 滚出视口会 `removeFromSuperview`）。禁止为了绿去改 caret 断言成 0。
- erase-up：已有 `ResizeRedrawRegressionTests`（裸 `Terminal`）。新测试打在 **MuxTerminalView.feedOutput**。若堆叠，查 `TerminalManager` 合并增量、模型行列 vs tmux pane 列（滚动条已经隐藏，不要再打开）。

### 3.2 pane-cmd → process_name

`MainWindow.pollOnce` 里 `STATE_STATUS_SUBSCRIPTION`：

```swift
if ev.name.hasPrefix("muxterm.pane-cmd") {
    _ = bridge.attentionSetProcessName(paneId: ev.paneId, name: value)
}
```

空 pane id 不要写到 0 误伤。Linux：`pane.map(|p| p.0).unwrap_or(0)` 只在真有 pane 时有意义；FFI 已把 pane 放进 `out.pane_id`。

### 3.3 Cmd-P 三 tab 统一面板

对照 `quickconnect_panel.rs` + `panel_model.rs`：

- **一个** NSPanel。顶部共享 input（identifier 建议 `muxterm.panel.input`，并给旧的 `muxterm.quickConnect.input` / `muxterm.search.input` / `muxterm.attention.input` 做别名，让 `PanelE2ETests` 继续绿）。
- 三个 tab 按钮：`muxterm.panel.tab.workspaces|attention|search`。
- 状态用已有 `PanelModel`。Tab / Shift+Tab 在面板是 key window 时 `cycleTab`；Esc 关。
- Workspaces = 现在 QuickConnect 列表（含 SSH 灯如果 Linux 有）。
- Attention = 现在 AttentionPanel（跳转、mute）。**W19 起不要 peek**（`muxterm.attention.peek` 必须消失；Cmd-Enter 走 `muxterm.replyOverlay`）。
- Search = 现在 SearchPanel（`muxterm.search.hit-*`，activate 关面板 + 切 tab/pane）。
- query 跨 tab 保留（`PanelModel.query`）。
- 红点点击：有 blocked 则打开面板并停在 Attention，否则 Workspaces（Linux `window.rs` 同一套）。

不要维持三个独立 NSWindow 再「假装」有 tab。

### 3.4 Cmd-Enter

`handleKey` 已按 keyCode 36/76 认 Enter。`CmdEnterKeyE2ETests` 必须继续走这条，不要改回只调 `toggleActivePaneFullscreen()`。

---

## 4. 夹具纪律

- tmux **只** `-L muxterm-test-*`。清理同一 `-L` 的 `kill-server`。禁止默认 server。
- 无 tmux：**失败**，禁止 `XCTSkip`。
- 禁止加长裸 sleep 当修复。硬超时 + `AppE2E.wait`。
- 不要把 UnifiedPanel / PaneCmd / AgentRender 标 skip。

---

## 5. 怎么跑（汇报真实退出码）

```bash
cd src/platform/macos
swift test --disable-swift-testing --filter PanelModelTests
swift test --disable-swift-testing --filter UnifiedPanelE2ETests
swift test --disable-swift-testing --filter PaneCmdE2ETests
swift test --disable-swift-testing --filter AgentRenderE2ETests
swift test --disable-swift-testing --filter CmdEnterKeyE2ETests
swift test --disable-swift-testing --filter MuxtermAppE2ETests
swift test --disable-swift-testing --filter MuxtermChromeTests
```

仓库根：

```bash
cargo test --no-default-features --features ffi --test macos_e2e -- --test-threads=1
```

---

## 6. 完成定义

- [ ] 上表新测试全绿，断言没被改弱
- [ ] `PanelE2ETests` / `ZoomE2ETests` / `FeatureE2ETests` / `ChromeE2ETests` 仍绿
- [ ] Cmd-P 一个窗口能 Tab 到搜索和注意力
- [ ] `/bin/cat` 的 process_name 能在注意力快照里看到
- [ ] 新 attach 的 `refresh-client -r` 不再是 `000000`/`ffffff`（可用 `MUXTERM` debug 日志或复跑 1457 场景目测）
- [ ] 英文 commit，未 push

不要做：改 CI yaml、杀默认 tmux、为了绿把 tab identifier 改成现有 QuickConnect-only 控件、把 OSC 断言改回允许浅色。
