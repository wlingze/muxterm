# MACOS-W19-PLAN.md — 主题 / SSH 列表 / 连接进度 / 注意力 UX / status-right / tab 手势（交给 Codex）

> 日期：2026-08-17（本机 `2026-08-17T16:54:25+08:00`）
> 工作目录：`/Users/wlz/Developer/self/muxterm`
> 分支：`feature/quickconnect-attach-ui`（**不 push**）
> Linux 对齐：`origin/feat/linux-quickconnect-ui` tip `bd01a39`（`docs: close W18 Linux tmux GUI gates`）
> 证据：`test-2026-0817-1626.log` + 用户 dogfood（主题无效、SSH session 列表不刷新、连接进度、后台 sleep 不通知、注意力 peek、status-right 被 tab 挤没）
> 先读：`docs/MACOS-LINUX-PARITY-PLAN.md`、`AGENTS.md`、本文件列出的新测试
>
> **Grok 写测试和本计划。你实现，直到下列命令真实退出码 0。**
>
> 禁止改断言、identifier、token、亮度方向、40px、400 event cap 来「绿」。
> 禁止 `tmux kill-server` / `kill-session` 打默认 server。测试只用 `-L muxterm-test-*`；SSH 夹具清理只 `kill-session -t muxterm-test-*` 或隔离 socket 的 `kill-server`。
> 不要动已修的 SSH attach `sshAlias`、i18n `Bundle.module`、Cmd-P 三 tab、pane-cmd、深色 OSC 10/11，除非测试证明你碰坏了。
> **不 push。** commit 英文 `type(scope):`，无 Co-authored-by，不 `git add -A`。

---

## 0. 先 rebase（必须第一步）

当前分支相对 `origin/feature/quickconnect-attach-ui` 是 ahead/behind 状态。用户要求 **rebase onto** `origin/feat/linux-quickconnect-ui`（merge-base `c9a7bde`，Linux tip `bd01a39`）。

```bash
git fetch origin feat/linux-quickconnect-ui
git rebase origin/feat/linux-quickconnect-ui
```

Linux W18 带了 OSC 133 command marks / status / search。冲突按「两边都留」处理：不要丢掉 Linux W18，也不要丢掉 macOS parity commits（`720c4fd` / `8e144f2` / `bdd6393` 及本轮测试）。rebase 后再跑旧测试，坏了先修编译。

本轮 **不要** 把 Linux command marks 扩成新功能，除非 rebase 后现有测试红。

---

## 1. 用户看到的问题

1. **主题切换失败。** `applyTheme` 写了 UserDefaults 并 `reportAllPaneColours`，但窗口/chrome `effectiveAppearance` 不变。tmux status 模式下条也不跟着变。终端 OSC 必须继续固定深色（`cdd6f4` / `1e1e2e`），主题只改 chrome。
2. **Cmd-Shift-P → SSH → host 后 session 列表停在 New session。** 日志 `08:28:03Z` 已 spawn `ssh ryzen -- tmux list-sessions`，UI 没刷。`showSessions` 先画 placeholder，异步 `listRemoteSessions` 失败会 `dismiss`。Local 路径 `listLocalSessions` **没把 attached `bridge.socket` 传进 discover**，隔离 socket 的 session 也刷不出来。
3. **连接进度应盖住主内容**（resolving / ssh / list-sessions / attach / capture），identifier `muxterm.connectProgress`。不是小 dialog。Linux 是 `pending_connects`，没有这层 overlay——这是 macOS 产品要求。
4. **通知 + 注意力 UX**
   - 前台 `sleep 3 && echo aa` 不通知（对）；cursor agent OSC 133 D 会通知。后台 sleep 结束也要通知：优先 OSC 133 D；pane-cmd `sleep` → `zsh`/`cat` 也算 Done。
   - 列表行 = **进程名 + transport + path**，不要 `last_line` 片段。
   - ↑↓ 选行；**Enter = 跳转**（关面板、切 tab/pane）。
   - **删掉 peek**（`muxterm.attention.peek`）。`FeatureE2ETests` 已改成 `XCTAssertNil(testPeekView())`，不要把断言改回 NotNil，不要 skip。
   - 注意力面板 **Cmd-Enter** = 独立 zoom overlay `muxterm.replyOverlay`，用 replica/snapshot 渲染该 pane，I/O 走 overlay，**不改主布局 SwiftTerm / leaf 数**。再按一次关掉。主窗口 Cmd-Enter 仍是 tmux `resize-pane -Z`（`CmdEnterKeyE2ETests` 必须绿）。
5. **status-right 被 tab 挤没。** Linux `status_bar.rs`：`right.set_hexpand(true)`，chrome 永远可见。macOS tab 未限宽。要固定 tab 宽 + 溢出滚动；status-right 与 ●/🔔/+ 始终可见。
6. **iTerm2 手势（已核实 2026-08-17）**
   - 拖 tab 排序：`PseudoTerminal moveTabAtIndex:toIndex:` → Muxterm **`move-window`**。
   - pane 拖成新 tab：`MoveSessionToNewTabBuiltInFunction` / `TmuxController breakOutWindowPane:` → **`break-pane`**。
   - pane 挪到另一个 split：`movePane:intoPane:` → **`move-pane`**。
   - Muxterm：**tmux window = tab，tmux pane = pane**。不要第二套窗口层次。

证据链接：
- https://github.com/gnachman/iTerm2/blob/f243568d/sources/TerminalView/MovePaneController.m
- https://github.com/gnachman/iTerm2/blob/ea21d790/sources/TmuxController.h

---

## 2. 顺序（不要跳）

1. rebase 绿到能编译。
2. 新测试能编译、能跑红：`swift test --disable-swift-testing --package-path src/platform/macos`。
3. **W19-A** 主题：`ThemeToggleE2ETests`。
4. **W19-B** session 列表：`PaletteSessionListE2ETests`（SSH 无 loopback 可 skip，不算绿；Local 必须红→绿）。
5. **W19-C** 连接进度：`ConnectProgressE2ETests`。
6. **W19-D** 通知 + 注意力行标题：`NotifyBackgroundCommandE2ETests`。
7. **W19-E** 注意力导航 / 去 peek / overlay：`AttentionNavE2ETests` + `FeatureE2ETests`（peek=nil）。
8. **W19-F** status-right：`StatusBarOverflowE2ETests`（Chrome `StatusBarTabOverflowTests` 已绿，接 UI）。
9. **W19-G** `move-window` / `break-pane`：`TabReorderE2ETests` / `BreakPaneE2ETests`（需 core `Task` + FFI；`muxterm.h` 现在 TASK 只到 10）。
10. 回归：`MuxtermAppE2ETests` 全套、`MuxtermChromeTests`、`CmdEnterKeyE2ETests`、`cargo test --no-default-features --features ffi --test macos_e2e -- --test-threads=1`。

每步一逻辑一英文 commit。

---

## 3. 已落地的测试（RED → 你实现）

| 文件 | 硬断言 |
|---|---|
| `Chrome/AttentionRowLabel` + `AttentionRowLabelTests` | `process  transport  path`，不含 last_line |
| `Chrome/ConnectProgress` + tests | identifier `muxterm.connectProgress`；阶段 resolving/ssh/list-sessions/attach/capture |
| `Chrome/StatusBarTabOverflow` + tests | 固定 tab 宽 96；right min 64；20 tab @ 720pt overflow>0 |
| `Chrome/TmuxWindowCommands` + tests | `move-window` / `break-pane` / `move-pane` |
| `Chrome/CmdEnterRouting` + tests | 主窗口 zoom；注意力 overlay；overlay 再按关闭 |
| `ThemeToggleE2ETests` | theme 翻转；UserDefaults；**chrome appearance 翻转**；OSC 仍 `cdd6f4`/`1e1e2e` |
| `PaletteSessionListE2ETests` | Local：隔离 socket 的 extra session 出现；面板不关 |
| `ConnectProgressE2ETests` | 不可达 SSH 时全窗口 overlay 可见，value 含阶段名 |
| `NotifyBackgroundCommandE2ETests` | 后台 sleep 有 done 通知；行标题无 AA_BG、有进程/transport/path |
| `AttentionNavE2ETests` | peek=nil；Cmd-Enter overlay 含 bg token、leaf 数不变、输入进该 pane；再按关闭；Enter 跳转关面板 |
| `FeatureE2ETests` | **peek 必须 nil**（替换旧 NotNil 契约） |
| `StatusBarOverflowE2ETests` | 12+ 长名 tab @ 720pt：status-right 宽 ≥ 64，tab 宽 ≤ 96，chrome 仍在 |
| `TabReorderE2ETests` | `testReorderTab` 后 GUI tab id 顺序 = tmux window 列表 |
| `BreakPaneE2ETests` | `testBreakActivePaneToNewTab` 后 2 tab / 当前 1 leaf / tmux 2 window |

钩子已在 `MainWindow+Testing.swift`：`testToggleTheme` / `testShowLocalSessions` / `testConnectProgressVisible` / `testReplyOverlayVisible` / `testReorderTab` / `testBreakActivePaneToNewTab`（后两个现在是空操作，接上才会绿）。

---

## 4. 实现要点

### 4.1 主题

`applyTheme` 必须设 `window` / `content` / `NSApp` 的 `NSAppearance`（light=`aqua`，dark=`darkAqua`）。status bar 在 `.theme` 模式跟 chrome；`.tmux` 模式 left/right 仍用 tmux 样式，但窗口 chrome 仍要变。不要把 OSC 10/11 绑回 light 的 `000000`/`ffffff`。

### 4.2 session 列表

- `ConnectionDiscovery.listLocalSessions` / `showSessions(.local)` 传入 **attached `bridge.socket`**。
- SSH：`listRemoteSessions` 用 host alias；已 attach 的 SSH 还要把 remote socket 传进去（不要再把 alias 塞进 `-L`）。
- 异步失败不要默默 `dismiss` 成只剩 New session；错误可显示但仍留列表。
- 回调已经 `DispatchQueue.main`；查 table `reloadData` / 面板是否被第二次 `present` 冲掉。

### 4.3 连接进度

`ContentView` 上盖一层全内容 overlay（可参考 `disconnectOverlay`），identifier `muxterm.connectProgress`。阶段写 AX value。成功 attach 后隐藏。

### 4.4 注意力

- 删 `UnifiedPanelController` / `AttentionPanelController` 的 peek 容器与 `muxterm.attention.peek`。
- 行文案走 `AttentionRowLabel.display`。transport/path 从 workspace id / pane cwd / QuickConnect path 拼。
- 后台 CommandDone：OSC 133 D **或** pane-cmd 从非 shell 回到 shell。前台 Idle，不 notify。
- `handleKey`：注意力面板是 key 且 **无** Command 的 Return → 跳转；**Cmd-Enter** → overlay。主窗口 Cmd-Enter 仍 `togglePaneFullscreen`。
- overlay：独立 `MuxTerminalView`，`feedOutput` snapshot/replica，`sendInput` 到该 paneId，`syncSizeToPty(notifyResize: false)` 以免改主布局 PTY。

### 4.5 status bar

tab 放 `NSScrollView`，每个按钮宽 = `StatusBarTabOverflow.fixedTabWidth`。`rightLabel` 与 chrome `compressionResistance` 提高，不能被 tab 压到 0。可参考 Linux 顺序 `[left][tabs][right][chrome]`，但 **right+chrome 可见** 是硬条件。

### 4.6 tmux 手势

在 `src/core/model/task.rs` + FFI `muxterm.h` 增加 `MoveWindow` / `BreakPane`（下一个 TASK id 从 11 起，两边 header 同步）。macOS `MuxTask` + `testReorderTab` / `testBreakActivePaneToNewTab` 走 `bridge.execute`。GUI 拖 tab / 拖 pane 接到同一 Task。

---

## 5. 验证命令（全部退出码 0）

```bash
swift test --disable-swift-testing --package-path src/platform/macos --filter MuxtermChromeTests
swift test --disable-swift-testing --package-path src/platform/macos --filter MuxtermAppE2ETests
cargo fmt --all -- --check
cargo clippy --no-default-features --features ffi --all-targets -- -D warnings
cargo test --no-default-features --features ffi --test macos_e2e -- --test-threads=1
```

`CmdEnterKeyE2ETests`、`UnifiedPanelE2ETests`、`PaneCmdE2ETests`、`AgentRenderE2ETests` 必须仍绿。
