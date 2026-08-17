# MACOS-E2E-PLAN.md — 把 Linux 功能保真测试复刻到 macOS（交给 Codex）

> 日期：2026-08-17（本机 `2026-08-17T13:08:54+08:00`）
> 续工（Cmd-P 三 tab / pane-cmd / agent 色与光标）：[`MACOS-LINUX-PARITY-PLAN.md`](MACOS-LINUX-PARITY-PLAN.md)
> 工作目录：`/Users/wlz/Developer/self/muxterm`
> 分支：`feature/quickconnect-attach-ui`（**不 push**）
> 先读：`docs/FEATURE-E2E-PLAN.md`、`docs/W16-PLAN.md`、`AGENTS.md`
>
> **测试已经写好且必须先红。禁止改断言、阈值、token、identifier 来「绿」。**
>
> Grok 写测试和本计划。你实现，直到下列命令真实退出码 0。

---

## 0. 为什么要做

Linux 用 in-process GTK `AppWindow::test_*` 锁死了 W13–W16 用户路径。macOS 之前只有 3 个 FFI 测试 + 弱 XCUITest：搜得到但看不出跳转、断线只查 buffer、Exited 仍关窗。

本轮把 Linux 契约搬到 **in-process AppKit**（`MuxtermAppLib` + `AppE2ETests`），对标 GTK e2e，不是再写一层更弱的 FFI。

已知生产 bug（测试已覆盖，必须修）：

1. `MainWindow` 对 `BackendStatus::Exited` (pane_id=4) 调 `closeSessionWindow()`。Linux W16b：tmux 死后留最后一帧 + 水印，不关窗。
2. 状态点 popover 文案不是 `type=ssh` / `1.5 KB/s` / `1.5 KB` + `56 B`。
3. 注意力面板没有 `muxterm.attention.peek` 小终端。
4. FFI `AttentionEngine` 用 `AttentionConfig::default()`，不读 `config.toml` 的 `blocked_regex`（W16c 正则点亮会红）。
5. Alt+Enter zoom：core 可能已 zoom，GUI leaf 必须变成 1。

---

## 1. 顺序（不要跳）

1. **先让测试能编译、能跑红**：`swift test --disable-swift-testing --package-path src/platform/macos`。`MuxtermAppLib` / `AppE2ETests` / `MainWindow+Testing.swift` 不要拆掉。
2. **W13** `AttachE2ETests` 绿（2tab/3pane、≥40px、切 tab、CUP 1s ≤ 400）。
3. **W14** `FeatureE2ETests` 绿（搜索跳转关面板、BEL 通知、peek、Done 通知、mock-codex 末帧、tail -f）。
4. **W16b** `DisconnectE2ETests` 绿（**禁止关窗**）。这是现成红测试。
5. **W16a** `HistoryE2ETests` 绿（离屏 token、回底按钮）。
6. **W16c** `AttentionSemanticsE2ETests` 绿（看见不熄、输入才熄、NEED_INPUT 正则）。
7. **SearchJump / Zoom / Chrome / Live / Panel / Prefs / QuickConnect / ConnectTimeout / AttentionBadge / Render** 绿。
8. 回归：`cargo test --test tmux_attach_contract -- --test-threads=1`、`cargo test --test tmux_feature_contract -- --test-threads=1`、`cargo test --no-default-features --features ffi --test macos_e2e -- --test-threads=1`、`swift test --disable-swift-testing --package-path src/platform/macos --filter MuxtermChromeTests`。

每步一逻辑一英文 commit，`type(scope):`，无 Co-authored-by，不 `git add -A`，不 push。

---

## 2. 夹具纪律

- tmux **只** `-L muxterm-test-*`。清理同一 `-L` 的 `kill-server`。禁止默认 server。
- pane 命令用 `/bin/cat`、`/usr/bin/tail`、`python3 -u tests/scripts/mock_codex.py`。
- **先**在 tmux 里画 token，**再** `MainWindowController` attach。空 session echo 不算 attach。
- in-process：`NSApplication.shared` + `orderFront`，每个测试一个 `MainWindowController`，`defer { testShutdown() }`。
- `--test-threads=1`（cargo）。Swift 这边不要并行开多个真实 tmux attach。
- 禁止加长裸 sleep 当修复。硬超时 + `AppE2E.wait`。
- 无 tmux：**失败**，禁止 `return` / `XCTSkip` 冒充绿（`AppE2E.requireTmux()`）。
- 不要把 Search / Done / flood / disconnect 标 `#[ignore]` 或 `XCTSkip`。

---

## 3. 已落地的测试（RED → 你实现）

| 文件 | 对标 Linux | 硬断言 |
|---|---|---|
| `src/platform/macos/AppE2ETests/AttachE2ETests.swift` | `linux_workspace_attach_e2e` | 2tab/3pane；leaf ≥ 40px；tab1 token 可见；点 `muxterm.tab.*` 切到 tab2；切回 token 还在；CUP 1s PaneOutput ≤ 400；洪水后非空 |
| `FeatureE2ETests.swift` | `linux_feature_e2e` | search_all 命中；搜索面板 `muxterm.search.hit-*`；activate 关面板；BEL → recorded 含 blocked；`muxterm.attention.peek` 含 bg token；`W15_REPLY` 进 tmux；OSC 133 D → done 通知；TOKEN_HEADER+TOKEN_PROMPT；TAIL_FOLLOW_TOKEN |
| `SearchJumpE2ETests.swift` | `linux_search_jump_e2e` | 搜 tab2 token → 当前 tab 变成 tab2 + 面板关掉 |
| `DisconnectE2ETests.swift` | `linux_disconnect_e2e` | kill-server 后 `testWindowVisible()`；SwiftTerm 仍含 token；`muxterm.disconnectOverlay` 可见；无 sheet |
| `HistoryE2ETests.swift` | `linux_attach_history_e2e` | 离屏 token 可搜；viewport>0 后可见离屏 token；`muxterm.jumpLatest` 可见；点完回到 tail、按钮隐藏 |
| `AttentionSemanticsE2ETests.swift` | `linux_attention_semantics_e2e` | BEL blocked≥1；`on_became_visible` 仍 ≥1；输入后 0；`NEED_INPUT` 无 BEL 再 ≥1 |
| `ChromeE2ETests.swift` | `linux_chrome_e2e` | 一条 bar；tab 标题 `1  code` 不含 `#[`；点 tab-21 回调 21；状态点 18×18；**performClick** 后 popover 含 `type=ssh` / `host=127.0.0.1` / `status=connected` / `1.5 KB/s` / `1.5 KB` / `56 B`；禁止 `1536B/s` |
| `ZoomE2ETests.swift` | macOS 旧 bug | Alt+Enter 路径：tmux zoomed_flag=1 **且** GUI leaf==1；再按恢复 3 leaf |
| `LiveE2ETests.swift` | `linux_live_e2e` | echo 到 SwiftTerm；CUP 停在 frame-19 不含 frame-0；点 status tab 切 window |
| `QuickConnectE2ETests.swift` | `linux_quickconnect_e2e` zoom 段 | status snapshot 有 current；toggle fullscreen → zoomed_flag=1 |
| `PrefsE2ETests.swift` | `linux_prefs_e2e` | Cmd+= 写 `muxterm.terminalFontSize` |
| `ConnectTimeoutE2ETests.swift` | `linux_connect_timeout_e2e` | `testConnectTarget(192.0.2.1)` < 500ms；窗口还在 |
| `PanelE2ETests.swift` | `linux_panel_e2e` | search/attention/quickConnect identifier |
| `AttentionBadgeE2ETests.swift` | `linux_attention_e2e` | BEL 后红点不是 `"0"` |
| `RenderE2ETests.swift` | `linux_render_e2e` CUP | MuxTerminalView 20 帧后 frame-19、无 frame-0 |
| `tests/macos_e2e.rs` | core FFI | 无 tmux 失败；disconnect 必须有 status 0 或 4；zoom 后 `LAYOUT_LEAF` |
| `MuxtermAppUITests.swift` | 键盘 GUI | 搜索必须切到 tab2；kill-server 窗口+水印；Alt+Enter 只剩 1 pane host；点 `muxterm.statusDot` 出 popover；历史回底必须 isHittable |

App 钩子（已加，不要删）：`MainWindow+Testing.swift` 的 `testPollOnce` / `testPaneTerminalText` / `testLayoutLeafIDs` / `testSearchAll` / `testDisconnectOverlayVisible` / `testClickJumpLatest` 等。

---

## 4. 实现要点

### 4.1 W16b 断线（先做，现成红）

对照 `src/platform/linux/window.rs`：tmux 的 `Disconnected` **和** `Exited` 都只显示 overlay，不 `pending_close`。

`MainWindow` 里这段是错的：

```swift
if ev.paneId == 4 {
    closeSessionWindow()
    return
}
```

改成：tmux/ssh 控制模式（`terminalManager.usesClientResize`）下 0 和 4 都 `content.setDisconnected(true)`，**不关窗**。本地 shell 的 Exited 仍可关窗。

### 4.2 搜索跳转

`jumpToPane` 不要写 `if tabId != 0`。tmux window id 可以是 0。命中行 activate 必须 `requestSwitchTab` + `switchPane` + 关搜索面板。

### 4.3 peek

注意力面板选中行后必须有 identifier `muxterm.attention.peek` 的小 `MuxTerminalView`，喂该 pane 的 `getPaneOutput`。快速回复走和 VTE 同一条 `sendInput` / `on_user_input`。

### 4.4 正则 blocked

`muxterm_new` 里 `AttentionEngine::new(AttentionConfig::default())` 必须改成 `Config::load()` 的 attention（测试会设 `XDG_CONFIG_HOME`）。非法正则跳过，不要 panic。

### 4.5 状态点 popover

必须 `performClick` 才打开（已有）。文案锁死 Linux 同款字段：`type=ssh`、`host=`、`status=`、人类可读 `1.5 KB/s` 与累计 `1.5 KB` / `56 B`。`upRate` / `upBytes` 已经在 `StatusBarView.updateConnectionStatus` 里传入。

### 4.6 zoom

core `%layout-change` flags 含 `Z` 时 layout 单叶。`PaneLayoutProjection.accepts` 已允许 1 leaf vs 多 pane。GUI `apply(layout:panes:)` 必须真的只挂一个 `PaneHostView`。

### 4.7 历史回底

`setPaneViewport` 之后 SwiftTerm 可见文本必须跟着变（不能只改 core offset）。`muxterm.jumpLatest` 在 viewport>0 时显示。

---

## 5. 怎么跑（汇报真实退出码）

先确保 `src/platform/macos/Vendor/libmuxterm.a` 指向带 `ffi` 的 debug staticlib。

```bash
cargo fmt --all -- --check
cargo test --test tmux_attach_contract -- --test-threads=1
cargo test --test tmux_feature_contract -- --test-threads=1
cargo test --no-default-features --features ffi --test macos_e2e -- --test-threads=1
cd src/platform/macos
swift test --disable-swift-testing --filter MuxtermAppE2ETests
swift test --disable-swift-testing --filter MuxtermChromeTests
```

XCUITest 有 Aqua 会话再跑（SSH/Background 可 skip，**不要把 in-process e2e 也 skip**）：

```bash
cd src/platform/macos && xcodegen generate
xcodebuild test -project Muxterm.xcodeproj -scheme MuxtermApp -destination 'platform=macOS'
```

---

## 6. 完成定义

- [ ] 上面 Swift e2e 全绿，断言没被改弱
- [ ] `DisconnectE2ETests`：kill-server 后窗口仍在
- [ ] `tmux_attach_contract` / `tmux_feature_contract` / `macos_e2e` 绿
- [ ] MuxtermChromeTests 仍绿
- [ ] 英文 commit，未 push

不要做：改 CI yaml、改 TESTING.md、Herdr、杀默认 tmux、把 Window 映射成 tmux window、为了绿把 token/40px/400 改掉。
