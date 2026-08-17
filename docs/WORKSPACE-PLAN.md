# WORKSPACE-PLAN.md — Codex 实施合同（工作区架构）

> **Linux tmux GUI 测试门禁已锁。** W1–W18 已落地（[`W18-PLAN.md`](W18-PLAN.md)）。后续：macOS 开发与人手修 bug；以后再改分支名、清理 log/test/文档。审计：[`VISION-AUDIT.md`](VISION-AUDIT.md)。
> Herdr Runtime 施工单：[`HERDR-PLAN.md`](HERDR-PLAN.md)（分支 `feature/runtime/support_herdr`）。测试写法与运行命令以该文件 §4–§5 为准。
> 架构：[`WORKSPACE.md`](WORKSPACE.md)（先读完再写代码，尤其 §6 接口）
> 像素契约：[`SURFACE.md`](SURFACE.md)（F 已交，live 路径禁止 dump）
> 测试契约：[`TESTING.md`](TESTING.md) §5.4 attach + §5.5 功能 e2e + §5.6 W15 + §5.7 W16 + §5.8 W17 + §5.9 W18
> 功能测试规格：[`FEATURE-E2E-PLAN.md`](FEATURE-E2E-PLAN.md)
> 分支：本地 `feat/linux-quickconnect-ui`，跟踪 `origin/feature/quickconnect-attach-ui`（macOS 同线，以后再改名）
> 修订：2026-08-17 15:44 CST（`2026-08-17T15:44:12+08:00`）

---

## 任务目标（本轮要交什么）

把 Muxterm 从「GUI 自己养连接 + 产品层假装有 Session/虚拟 Window」收成：

```
Core:   WorkspacePool → Workspace → Tab → Pane
View:   Window 只是某个 Workspace 的体现（画 Tab/Pane + 终端）
Adapt:  tmux 全部在 runtime/tmux，把 tmux 树填进我们的结构
API:    FFI/CLI 只说 workspace / tab / pane（list-sessions → list-workspaces）
```

**成功长这样：**

1. Tab/Pane 是 Workspace 的标准内部结构；Shell 没有 tmux 也是这套；前端画的就是这套。
2. `WorkspacePool` 在 **core**（旧连接池）。platform **没有** ConnectionPool 生命周期。
3. GUI Window `bind` 当前 Workspace；切 Tab 不扔 VTE；切工作区画面还在。
4. FFI：一个 handle = 整个池（`workspace_list` / `open` / `activate` / `close`）。
5. CLI：`list-workspaces` / `new-workspace` / `-s` 工作区名。没有产品 Session / `list-windows`。
6. `linux_render_e2e` + `linux_live_e2e` 仍绿（F 契约：原地打字、不 dump、不乱 reset）。

**本轮不是：** 重做渲染、做 Herdr、抄 iTerm2 一 tmux window 一 OS 窗口、push、杀用户默认 tmux。

**开工顺序：** 严格 W1 → W8。**一次做一个 W**（先 RED 再 GREEN，一个英文 commit），**做完立刻做下一个，不要等用户说继续。** 做到 W8 停下来汇报。当前 HEAD 已含 W1（`3f19923`），从 **W2** 接着做。

F1–F6 已把 Linux live 显示改成原始字节进 VTE。接口细节见 [`WORKSPACE.md`](WORKSPACE.md) §6。

### W1 复盘（已做完，后续别再犯）

W1 本体合格：`3f19923` 只加 `src/core/workspace/`，没改 GTK，单测隔离 pane，门禁绿。另有 `fbc77e4`：本机 12pt 字体让 VTE 网格变成 79×21，F 的 80×24 fixture 头滚出可见区，测试改 10pt，**断言没改**。不要回滚这个 commit。

后续禁止：

1. **不要等用户。** 做完一个 W 立刻下一个，直到 W8。不要 `update_goal complete` 停住。
2. **不要提交工作区里已有的脏 docs。** `AGENTS.md` / `docs/WORKSPACE*.md` / dogfood 摘录是架构文档，**W8 才收口**。W2–W7 每个 commit 只含该 W 的代码+测试。
3. **不要** `git add -A` / `git add docs/`。
4. **不要** 为了跑测试再建 `/tmp` worktree；就在当前仓库跑。不要 `git worktree remove --force` 别人的 worktree。
5. 测 workspace 模块用 `cargo test --lib workspace::`（注意 `::`）。不要 `cargo test --lib workspace`，会把 `replica` 等名字里带 workspace 的测试也算进来。
6. `WorkspaceId` 里的 `session` 字段是连接身份字符串（和旧 `ConnectionKey` 同构），**不是**产品 Session。W7 可改名叫 `name`，W2 不必改。
7. **不要回滚** `fbc77e4`。`linux_render_e2e` 字体必须让网格 ≥80×24。
8. **不要** 在 platform 再写一套 ConnectionPool。W2 池进 core；Linux 只 bind。
9. W2–W4 **不要改** `pane_view.rs` live 路径。W5 只改 layout 挂载/池绑定，禁止 `visible_ansi` dump。

---

## 0. 执行合同

1. 先读：[`WORKSPACE.md`](WORKSPACE.md)、本文、[`SURFACE.md`](SURFACE.md)、[`bugfix-log.md`](bugfix-log.md)、四份 dogfood 摘录。对照 `/home/wlz/Developer/terminal/{ivyterm,iterm2,wezterm,ghostty,cmux,tmux,psmux}` 与本仓库 macOS `WarmConnectionSlot.swift`。**不要抄 iTerm2「一 tmux window 一 OS 窗口」。**
2. **产品层没有 Session。** FFI/CLI/GUI 不得出现 `SessionId` / `list-sessions` 产品语义。tmux `$N` 只留在 `src/core/runtime/tmux/`（`TmuxSessionId`）。
3. **Tab / Pane 是 Workspace 的结构**，不是 tmux 的结构被前端借用。TmuxRuntime 只适配。不要再引入产品 `WindowId`。GUI **Window = Workspace 的体现**。
4. **WorkspacePool 只在 core。** 禁止 platform 再实现连接池 / 养 `CoreBridge` 列表。Platform 只渲染 + 快捷键。禁止在 `src/platform/*` 拼 ssh/tmux、解析 `%output`。
5. **对外接口以 [`WORKSPACE.md`](WORKSPACE.md) §6 为准。** `list-sessions` → `list-workspaces`；FFI handle = 整个池；`discover_tmux_sessions` → `discover_workspaces`。
6. **一 W 一 commit。** 先 RED 再 GREEN。`type(scope): English`，无 Co-authored-by，不 push。
7. **禁止** live 路径 `visible_ansi` → `vte.reset`。`linux_live_e2e` / `linux_render_e2e` 的 Surface 断言必须保持绿。
8. **禁止** 默认 tmux `kill-server` / `kill-session` / `kill-pane`。测试只用 `tmux -L muxterm-test-*`。前台命令用 `/bin/cat`。
9. **禁止** 重做 F 的像素路径、C7 session-id 显示、C8 ASCII 几何。**禁止** 本轮实现 Herdr。
10. **禁止** `include_str!` 原 34MB `test_2026-0815-*.log`。
11. macOS Swift：只保证 FFI 能编（可 deprecated 薄封装）。Linux 不要再长 platform `ConnectionPool`。
12. **W2–W7 禁止**把仓库里已有的未提交 docs 加进 commit。W8 才收口文档。
13. **禁止** `git add -A`。每个 commit 只 stage 本 W 文件。
14. **禁止**等用户说「继续」。W8 之前不要把 thread goal 标 complete。

验证门（每 W 汇报）：

```bash
cargo fmt
cargo check --features gtk
cargo test
cargo clippy -- -D warnings
xvfb-run -a cargo test --features gtk --test linux_render_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_live_e2e -- --test-threads=1
```

W5 起额外：

```bash
xvfb-run -a cargo test --features gtk --test linux_quickconnect_e2e -- --test-threads=1
```

W6 起额外：

```bash
xvfb-run -a cargo test --features gtk --test linux_search_e2e -- --test-threads=1
```

---

## 1. 定名（锁死，写代码时对照）

| 词 | 含义 |
|---|---|
| **Workspace** | Muxterm 工作区。标准内部结构 = Tab → Pane。一个 Runtime 填它 |
| **Tab / Pane** | **我们的**结构。前端画这个。tmux window/pane 只是 TmuxRuntime 的填法 |
| **WorkspacePool** | 旧连接池。只在 core：open / list / activate / 后台 / 淘汰 |
| **Window** | GUI 体现某一个 Workspace。不是产品树节点 |
| **Runtime** | 填 Workspace 的接口（今天 `Backend`）。Tmux / Shell |
| **TmuxSessionId** | 仅 `runtime/tmux` 的 `$N` |
| **Session** | 不是产品类型 |

切工作区 = `pool.activate`。不要 `Task::SwitchSession`。

---

## 2. 现状 → 目标

| 现在 | 目标 |
|---|---|
| `TerminalModel` 一个 `Backend`；GUI 堆多个 handle | `WorkspacePool`（core）里多个 `Workspace`，每个一个 Runtime |
| `SessionInfo` + 虚拟 `w1` + Tab + Pane | **我们的** Workspace → Tab → Pane；tmux 只在 adapter 里 |
| Linux/macOS `ConnectionPool` | **删除** platform 池；只留「按 WorkspaceId 缓存 VTE」 |
| `muxterm_new` = 一条连接；`discover_tmux_sessions` | handle = 池；`workspace_list` / `discover_workspaces` |
| CLI `list-sessions` / `new-session` / `-s` session | `list-workspaces` / `new-workspace` / `-s` 工作区名 |
| `LayoutHost` retain 掉其它 tab 的 VTE | 切 Tab 只摘挂 widget；连接仍在 core 池 |
| `SearchHit` 无 `tab_id` | `(workspace, tab, pane, seq, line)` |
| `tmux_dialog` / CLI 直调 `tmux list-sessions` | 只调 Core discovery（内部才 tmux） |

F 路径不动：当前可见 Pane 只 `vte.feed` 原始字节。

---

## 3. 对照谁（只抄机制，不抄铺窗口）

时间：`2026-08-15T22:50:10+08:00`。源码在本机 `/home/wlz/Developer/terminal/`；文档以官方页为准。

| 来源 | 抄什么 | 不抄什么 |
|---|---|---|
| **本仓库 macOS** `src/platform/macos/App/WarmConnectionSlot.swift` | slot = bridge + `TerminalManager`；后台 `handleOutput`；切走不 `displayIfNeeded` | Swift UI |
| **ivyTerm** `ivyterm/` `feed_output` / capture 一次 / `send-keys -H` | VTE 进 notebook 后活着；seed 门 | 一 GTK 窗 = 一条 tmux |
| **iTerm2** `TmuxGateway.m` `%pause`、hidden/buried window | 后台可 pause；懒建隐藏窗 | [官方 tmux integration](https://iterm2.com/documentation-tmux-integration.html)：**每个 tmux window 一个 OS 窗口** |
| **WezTerm** `wezterm/mux/` Domain；[workspaces](https://wezterm.org/recipes/workspaces.html) | Domain ≈ Runtime；切 workspace 换 GUI 内容 | workspace 只是 mux 窗标签，≠ 一条 tmux |
| **Ghostty** `src/terminal/tmux/viewer.zig`、`src/Surface.zig` | 每 pane 一台 `Terminal` | 像素 Surface 名字（我们已用在 F） |
| **cmux** `RemoteTmuxPaneSeed.swift` | discard + snapshot + catch-up | |
| **tmux** `control.c` / `tmux.1` | `%output` 按 client offset；`send-keys -H` | |
| **psmux** `tests-rs/test_send_keys_literal_byte.rs` | `-H` 单测写法 | |

本仓库已有、**必须保持绿**的测试（改名可以，断言不能松）：

| 测试 | 守什么 |
|---|---|
| `tests/linux_render_e2e.rs` `surface_typing_overwrites_in_place` / `surface_live_feed_does_not_reset` / `surface_codex_fixture_raw_feed` | F：原地打字、不 reset、Codex raw feed |
| `tests/linux_live_e2e.rs` `isolated_tmux_typing_token_appears_once` / `isolated_tmux_cup_script_lands_on_last_frame` / `isolated_tmux_switch_tab_resets_bounded` / `live_attach_vte_nonempty_and_prompt_not_collapsed` | F：真 tmux 不闪、切 tab reset 有界 |
| `src/core/replica.rs` `search_all_finds_hits_across_workspaces_and_panes` | 跨工作区搜索（W6 迁到 pool 后断言仍在） |
| `src/platform/linux/quickconnect/pool.rs` 单测 | 池容量 / TTL / 复用（W2 升 core 后迁过去） |
| `capture_pane_strips_trailing_blank_rows_so_cursor_stays_at_prompt` | bugfix-log §1 光标 |
| `tests/tmux_backend_integration.rs` / `sendkeys_regression.rs` | 隔离 tmux 后端 |
| macOS `testBackgroundTabEventsDoNotReloadCurrentUI` | bugfix-log §13；Linux W5 对等 |

改模型时 **不要回归**（见 `docs/bugfix-log.md`）：capture 光标、IME Backspace、PATH/ENOENT、SSH statusbar 转义、OSC 10/11 所有 pane、`%output` coalesce、模型宽度=pane 宽度、后台 tab 不重绘前台、tmux `<3.2` 跳过 `-r`、status bar 溢出。W 不修这些，但相关测试必须仍绿。

---

## 4. 代码落点（按层）

```
src/core/workspace/          # W1–W2：结构 + 池（旧连接池）
src/core/runtime/tmux/       # 全部 tmux 词；list-sessions 只在这里
src/core/model/{task,state}.rs
src/core/protocol/ffi/       # W7：handle = WorkspacePool；§6 的 C 名
src/platform/cli/            # W7：list-workspaces；禁止自己 spawn tmux
src/platform/linux/window.rs # 体现：bind 当前 WorkspaceId
src/platform/linux/layout_host.rs  # 只缓存控件，不养连接
```

`platform/linux/quickconnect/pool.rs` 的 **生命周期**在 W2 迁走、W5 删掉。不要两套池。

---

## 5. 分步（一 W 一 commit）

### W1 — `Workspace` 包装一个 Runtime

**Commit：** `feat(core): wrap one runtime in Workspace`

**做：**

- 新建 `src/core/workspace/{mod.rs,id.rs,workspace.rs}`。
- `WorkspaceId`：稳定字符串（可用今天 `ConnectionKey` 的显示形式：`transport/alias/session/runtime/path`）。不是 `$N`。
- `Workspace` 持有 `Box<dyn Backend>`（W4 再改名）+ 本工作区 pane 映射（可先委托现有 `ReplicaStore` 的单 key 切片，或内嵌 `HashMap<PaneId, TerminalState>`）。
- `TerminalModel` 仍可存在，作为 Workspace 内部；或 Workspace 直接调 Backend。**不要**这一步改 GTK。
- 从 `src/lib.rs` / `src/core/mod.rs` 导出。

**测试（先红）：**

- mock Runtime 推一段 `%output` 等价事件 → `workspace.pane_text(pane)` 含 token。
- 两个 Workspace、同一 `PaneId` 数字 → 文本互不污染。
- `WorkspaceId` 相等性：相同 key 相等，不同 session 名不相等。

**验收：** `cargo test --lib workspace` 绿；现有 `linux_*e2e` 无改动仍绿。

---

### W2 — `WorkspacePool`（旧连接池进 core）

**Commit：** `feat(core): add WorkspacePool`

**做：**

- `src/core/workspace/pool.rs`：**这就是连接池。** open / list / activate / 后台 `take_events` 喂 PaneBuf / 淘汰（tmux Detach、shell Shutdown）。容量、TTL、按 `WorkspaceId` 复用，从 `platform/linux/quickconnect/pool.rs` 搬语义。
- Linux GUI **改为调 core 池**（本 W 可通过现有 `CoreBridge` 包一层，但淘汰/复用逻辑不得再写在 platform）。不要两套互相打架的池。
- `activate` / `list` 先可用；搜索到 W6。

**测试（先红）：**

- 两个 mock workspace：A 前台、B 后台仍吃字节；`activate(B)` 后 A 仍能读到已索引文本。
- 超容量：tmux mock 计数 detach，shell 计数 shutdown。
- 同一 `WorkspaceId` 再 open 复用，不新建 Runtime。
- 现有 `quickconnect/pool.rs` 单测迁到 `workspace/pool.rs`（或双跑后删 platform 生命周期测试）。

**验收：** 池的单测在 `src/core/workspace/`。platform 不再决定 evict。

---

### W3 — 产品模型改为 Workspace → Tab → Pane

**Commit：** `refactor(core): drop Session and virtual Window from product model`

这是词汇提交。协议解析器里的 tmux 消息名（`WindowAdd`、`%session-changed`）可以留在 `runtime/tmux/protocol.rs`，那是 **tmux 的词**。

**做：**

| 删 / 改 | 变成 |
|---|---|
| `State` 的 `SessionInfo`、`WindowInfo` | `Workspace` 元数据（名字、id、runtime 种类）+ `TabInfo` + `PaneInfo` |
| `StateChange::SessionChanged` / `SessionsChanged` / `ActiveWindowChanged` / `WindowAdded`… | `WorkspaceRenamed` / `PoolChanged` / `ActiveTabChanged { tab }` |
| `Task::SwitchSession` / `RenameSession` | 无 / `RenameWorkspace`。换 tmux session = 池 `activate` |
| `Task::NewWindow` / `CloseWindow` / `SwitchWindow` / `RenameWindow` | 无。新建页用已有 `NewTab`（去掉 `window: WindowId`） |
| `Task::NewTab { window, ... }` | `NewTab { name, command, workdir }` |
| `src/core/types.rs` 的 `SessionId`、`WindowId` | 从产品 types 移除。`SessionId` → `runtime/tmux` 的 `TmuxSessionId`（`$N`）。tmux window id 只在 adapter 里映射到 `TabId` |
| 虚拟 `w1` 的构造 | 删除 |

`TabId` / `PaneId` 是产品 ID。TmuxRuntime 内部再映射 `@N` / `%N`。旧 `tN` Display 可暂留到 W7。**不要**再生成 `w1`。

TmuxRuntime 按 **名字** bind 一条 tmux session，填进这一个 Workspace。`%session-changed` 只更新内部 `TmuxSessionId` 与 `Workspace.name`。

**测试（先红再改实现）：**

- 快照：attach 假布局「tmux 2 window / 4 pane」→ **0 个** product Window、**2 个** Tab、pane 挂在对应 tab。无 `SessionId` 字段。
- `Task::NewTab` 不再需要 `WindowId`；执行后 tab 数 +1。
- 编译期：`src/platform/linux` 与 FFI 若仍写 `WindowId`/`SessionId`，本 W **一并改完**（否则 check 红）。允许 FFI 旧符号暂留 `#[deprecated]` 到 W7，但 Linux GUI 不得再传 `w1`。
- 现有 `tmux_backend_integration` / layout 单测改断言后仍绿。

**验收：** `rg 'SessionId|WindowId|SwitchSession|w1' src/core/model src/platform/linux` 无产品用法（protocol/tmux 与 TmuxSessionId 除外）。Surface e2e 绿。

---

### W4 — `Backend` 改名为 `Runtime`

**Commit：** `refactor(core): rename Backend trait to Runtime`

**做：**

- `trait Runtime`；`TmuxRuntime` / `ShellRuntime`（今天 `TmuxBackend` / `LocalBackend`）。
- 可先 `pub type Backend = Runtime` 过渡，但新代码只写 Runtime。
- 注释、日志、GUI 用户可见字符串不要出现「Backend」；内部文件名 `backend.rs` 可改 `runtime.rs` 或留 type alias 文件。
- ShellRuntime：**简单**，关 Workspace = 进程结束，**不要求**持久化。TmuxRuntime detach 保远端。

**测试：** 现有 mock backend 测试改名后全绿；无行为变化。

**验收：** `cargo test` 绿。不要顺手改协议。

---

### W5 — Linux：Window 只体现 Workspace；控件缓存不是池

**Commit：** `feat(linux): bind the window to a core workspace`

**做：**

- GUI Window `bind(WorkspaceId)`：向 **core 池** 取当前 Workspace 的 tabs/panes/字节。
- **删除** platform `ConnectionPool` 的生命周期（若 W2 还留了壳）。可以留 `HashMap<WorkspaceId, LayoutHost>` 当 **像素缓存**（切走不扔 VTE），但 Runtime 不在这里。
- `LayoutHost::apply_layout`：禁止因换 Tab 而 `retain` 掉其它 pane 控件。
- 切 Workspace = 改绑体现，core `activate`；后台仍由 core 池 poll。
- 每个 pane 记住滚动位置。后台 tab 的 layout 事件不要重建前台（bugfix-log §13）。

**测试（先红）：** 同前：两 tab 切回 token 还在；两 **工作区** 切回 VTE 非空；viewport；Surface e2e 绿。

**验收：** `rg ConnectionPool src/platform/linux` 无生命周期实现。连接开/关只在 core。

---

### W6 — PaneBuf：有界环、viewport、带 Tab 的搜索

**Commit：** `feat(core): pane buffers and workspace search`

**做：**

- 把 `ReplicaStore` 收进 `Workspace.panes`（`pane_buf.rs`）。键不再是松散 `String`。
- 每个 PaneBuf：有界 scrollback（已有）+ **有界 byte ring**（`buffer_cap`，不是上一截 `last_raw_bytes`）+ `viewport`。
- `SearchHit { workspace_id, tab_id, pane_id, seq, line }`。
- API：`search_pane` / `search_workspace` / `search_all`。跳转：`pool.activate` → `SwitchTab` → 恢复 viewport。
- Live GUI **仍然**只 `vte.feed` 原始字节。PaneBuf 只给搜索/提醒/peek。禁止 dump `visible_ansi` 当显示。

**测试（先红）：**

- 迁走并加强 `search_all_finds_hits_across_workspaces_and_panes`：命中带 `tab_id`；同 pane 不同 tab 不串。
- byte ring 超过 cap 丢最旧，搜索仍能命中最近 token。
- `linux_search_e2e`：后台工作区的 token 可搜到；跳转后 VTE 可见（若跳转 UI 本 W 来不及，至少 core 命中坐标对，GUI 跳转可 W6b，但计划默认本 W 做完 Linux Search tab 已有的打 replica 路径）。

**验收：** 搜索三个范围都有单测；F 显示路径无 dump。

---

### W7 — FFI / CLI 改成 Workspace 接口

**Commit：** `refactor(ffi): expose WorkspacePool instead of sessions`

接口以 [`WORKSPACE.md`](WORKSPACE.md) **§6 为准**，不要另发明一套。

**FFI**

- `MuxtermHandle` = `WorkspacePool`。`muxterm_new()` 空池。
- `muxterm_workspace_open` / `list` / `activate` / `close`。
- `muxterm_discover_workspaces_json` 替换 `muxterm_discover_tmux_sessions_json`。
- `muxterm_workspace_create` 替换 `muxterm_create_tmux_session_json`。
- `get_tabs` / `get_panes` / `get_layout` / `poll_events` 相对当前 workspace（或带 id）。去掉 `window_id`。
- 旧 `muxterm_new(backend, socket, session)` 可 deprecated 转发，供 macOS 暂用。

**CLI**（`src/main.rs` + `platform/cli/command.rs`）

| 现在 | 变成 |
|---|---|
| `list-sessions` / `ls` | `list-workspaces`（`ls` alias） |
| `new-session` | `new-workspace`（`new-session` alias） |
| `attach-session` | `attach-workspace` |
| `kill-session` | `close-workspace` |
| `rename-session` | `rename-workspace` |
| `new-window` / `list-windows` / … | 删除；用 `new-tab` / `list-tabs` |
| `-s` | 工作区名 |
| `CliCommand::ListSessions` 等 | `ListWorkspaces` … |

禁止 CLI / `tmux_dialog` 直接 spawn `tmux list-sessions`。走 Core discovery；tmux 命令只在 `runtime/tmux`。

`Capability.can_list_sessions` → `can_discover`。

**测试：** FFI 单测走 list/open/activate；`cli_integration` 用新子命令 attach 隔离 tmux；旧 `ls` alias 仍能列出。`muxterm.h` 与 `api.rs` 同步。

**验收：** `rg 'ListSessions|discover_tmux_sessions|SessionId' src/core/protocol/ffi src/platform/cli src/main.rs` 无产品语义（deprecated 转发除外，须标注释）。

---

### W8 — 文档与死代码收口

**Commit：** `docs: make Workspace the canonical product model`

**做：**

- [`LAYER-MAPPING.md`](LAYER-MAPPING.md) 已在本轮改写成新映射；本 W 核对代码与文档一致，删过期例子。
- [`ID-SYSTEM.md`](ID-SYSTEM.md)：路径 `workspace/{name}/tab/{n}/pane/{n}`。废弃 `s{name}/w1/t2`。
- 指针文档已在本轮改过；本 W 核对代码符号与 [`WORKSPACE.md`](WORKSPACE.md) §6 一致。
- 删除 deprecated 空壳（若 macOS 仍依赖，汇报留下哪些符号）。

**测试：** 无新逻辑则跑全门禁。文档-only 也要 `cargo test` 证明没误删代码。

**验收：** 新 agent 只读 `WORKSPACE.md` + 本计划不会再实现虚拟 `w1`。

---

### W9 — 禁止 platform 自己 spawn tmux list-sessions

**Commit：** `refactor(core): stop spawning tmux list-sessions from platform`

**完成定义：**

- `src/platform/cli/routing.rs` / `src/platform/cli/tmux_cli_exec.rs` /
  `src/platform/tui/app.rs` / `src/platform/cli/daemon.rs` 的
  `Command::new("tmux") list-sessions` 全部改为调 `core::discovery`（工作区候选）。
- `src/platform/linux/tmux_dialog.rs` 不再 spawn tmux；改走 core discovery，
  文案是工作区候选（name / created / tabs），不再叫 session list。
- 遗留的 raw `tmux capture-pane` 直调（CLI debug 命令）改成走 Runtime 的
  `pane_output`；`title_watch.rs` 的 `tmux display-message` 直调删除
  （无人调用，且属于 platform 碰 tmux）。

**验收：** `rg 'list-sessions' src/platform` 只剩 CLI alias 注释
（`list-sessions` → `list-workspaces`）；`Command::new("tmux")` 在
`src/platform` 为 0。

---

### W10 — Linux GUI 不要直接 new TmuxRuntime / 不要调 tmux 名字的 FFI

**Commit：** `refactor(linux): open workspaces through core pool spec`

**完成定义：**

- `src/core/workspace/spec.rs` 新增 `WorkspaceSpec`（transport / alias /
  session / runtime / path / socket / create），Runtime 构造只在 core。
- `WorkspacePool::open_spec(spec)` 是 platform 打开工作区的唯一入口。
- `src/platform/linux/window.rs` 不再 import `TmuxRuntime` / `ShellRuntime`；
  启动、本地 shell、tmux attach 全部走 `WorkspaceSpec`。
- `CoreBridge::discover_tmux_sessions` / `create_tmux_session` 改名为
  `discover_workspaces` / `create_workspace`；`TmuxSessionEntry` 改名为
  `WorkspaceCandidate`。macOS Swift 仍走 deprecated C 符号，本 W 不动。

**验收：** `rg 'TmuxRuntime|ShellRuntime|DaemonRuntime|discover_tmux|create_tmux|TmuxSessionEntry' src/platform/linux` 为空。

---

### W11 — 文档与验收

**Commit：** `docs: record remaining tmux-in-platform cleanup`

- 本文记录 W9–W10 完成定义。
- `rg 'list-sessions' src/platform` 只有 CLI alias 注释；`rg 'Command::new\("tmux"\)' src/platform` 为空。
- 门禁：`cargo fmt` / `cargo check --features gtk` / `cargo test` /
  `cargo clippy -- -D warnings` / `linux_render_e2e` / `linux_live_e2e` /
  `linux_quickconnect_e2e` 全绿。

---

### W12 — W10 收口（分层与命名，不修像素）

W9–W11 的 `rg` 验收过了，但规格层没做完。真机 attach 白屏/卡顿是 **W13**，不要把两件事混进同一个 commit。

**完成定义：**

1. `WorkspacePool::open_spec` 必须把 `build_runtime()` 放进 `open` 的 `create` 闭包。复用已有 slot 时 **零构造**（对得上 `reopen_same_id_reuses_without_new_runtime`）。
2. `src/core/workspace/spec.rs` 补 `#[cfg(test)]`：`id()` / `name()` / local attach vs `create` / ssh 空 session / 未知 runtime → shell。禁止 core 再 `use crate::platform::cli`（daemon socket 路径要么 spec 必填 `path`，要么挪到 runtime 构造处）。
3. CLI（`routing.rs` / `daemon.rs` / `tmux_cli_exec.rs`）打开工作区走 `WorkspaceSpec` / `open_spec`，或删掉 spec 注释里「CLI 不再 new TmuxRuntime」这句话，别假装已经统一。
4. 产品命名收口（可同一 commit 或紧随）：`TmuxAction::NewSession` / `SessionInfo` / `ChooseTmuxSession` / FFI JSON `"sessions"` → workspace 词。`window.rs` 文件头别再写 `muxterm_new`。`ARCHITECTURE.md` 删掉已删除的 `title_watch.rs`。

**验收：** `cargo test --lib workspace::`；`rg 'platform::cli' src/core/workspace` 为空。

---

### W13 — attach 保真（1820.log：白屏 / 错布局 / 高 CPU）

**证据（`test_2026-0816-1820.log`，2026-08-16T10:20:51Z–10:22:17Z，SSH `ryzen` attach `yaklang-workspace`）：**

- 118 879 行日志，**118 608** 条 `实时 %output 交付`；pane `%39` 独占 84 446 条（2730/1365 半帧，Codex TUI）。
- **0** 次 `%pause` / `refresh-client -A`。SURFACE.md 定律 7 没做。
- capture-pane 只在 attach 时发了 13 次；之后客户端把洪水当直播喂 GTK。
- 现有 `linux_live_e2e` **测不到**：它是 `new-session` 空壳再 `echo`，不是「先有 2 tab / 3 pane / 已有画面，再 attach」。

**测试已写（先红，禁止改断言迁就）：**

| 层 | 文件 | 必须抓住 |
|---|---|---|
| 夹具 | `tests/support/tmux_test_support.rs` + `workspace_attach_contract.rs` | 隔离 `-L muxterm-test-*`；**先**建 2tab/3pane + `/bin/cat` 涂 token，**再**让 Muxterm attach |
| A core | `tests/tmux_attach_contract.rs` | attach 后 core 有 2 tab、当前 tab 3 leaf、每个 pane 的 `pane_output` 含 token；CUP 洪水下 1s 内 `PaneOutput` 事件有上界，否则必须发出 pause |
| C+D GTK | `tests/linux_workspace_attach_e2e.rs` | VTE 非空；每个可见 pane 宽高 ≥ 40px；GTK Paned 数匹配 3 pane；切 tab token 还在；CUP 洪水后 VTE 仍可读、resets 有界 |
| 门禁 | `TESTING.md` §3.6 / §5.4 | `cargo test --test tmux_attach_contract`；`xvfb-run -a cargo test --features gtk --test linux_workspace_attach_e2e -- --test-threads=1` |

跨平台：契约在 `workspace_attach_contract.rs`（无 GTK）。macOS / Windows 复刻时实现同一套断言（SwiftTerm / DirectWrite 当 Surface），不要另写一套更弱的 echo 测试。

**修的方向（实现自由，断言不自由）：**

1. 已存在 session：capture 快照必须进 Surface（VTE 非空），禁止只 `feed` 洪水把播种冲掉。
2. 布局：3 pane 的 `LayoutNode` 必须变成 3 个面积非零的控件，不能整窗一块白。
3. 流控：忙 pane 必须 `%pause`（`refresh-client -A '%N:pause'`）或等价合并，禁止每条 `%output` 都进 GTK。iTerm2 `pausePanes` / tmux `control.c`。
4. 禁止 live 路径 `visible_ansi` → `vte.reset`。禁止回滚 `fbc77e4`。禁止杀默认 tmux。

**若大 e2e 红：** 拆小，但最终大 e2e 必须绿。建议顺序：

1. 单测：attach 模式 capture 响应把快照推进 `PaneOutput`，洪水不得丢掉快照事件。
2. 单测：输出速率超过阈值时 `dispatch` 出 pause 命令。
3. core 集成：`tmux_attach_contract`。
4. GTK：`linux_workspace_attach_e2e`。

**Commit 建议：** 一逻辑一英文 commit（pause / seed / layout / 测试绿）。不 push。

---

### W14 — 功能保真 e2e（搜索 / 通知 / SSH / mock-codex / tail-f / 诊断日志）

人工 dogfood 会反复回归，因为这些路径以前没有**真 tmux attach** 的测试：

| 功能 | 今天的测试 | 为什么不够 |
|---|---|---|
| 搜索 | `linux_search_e2e` 注入 Mock PaneBuf | 不 attach、不跳 VTE |
| Blocked 通知 | `linux_attention_e2e` `test_feed_replica(BEL)` | 几乎不走 `%output` |
| 任务完成 Done | engine 有 Done | **没有** `notify_done` / 桌面通知 |
| SSH tmux | `linux_ssh_e2e` `#[ignore]` 把 echo 喂进 **另一个 Mock Workspace** | 不算 SSH attach |
| Codex TUI | `linux_render_e2e` 喂静态 sample | 没有活进程画 CUP |
| `tail -f` / `/bin/cat` | attach 夹具只用 cat 涂一次 | 没有「后来追加的行」 |
| DEBUG | 每条 `%output` 打 `实时 %output 交付` | 1820.log 13MB 仍看不出白屏原因 |

**测试已写（先红，禁止改断言迁就）。规格见 [`FEATURE-E2E-PLAN.md`](FEATURE-E2E-PLAN.md)。**

顺序：先绿 W13（播种/pause/布局），再绿 W14（功能依赖非空 PaneBuf/VTE），再 W12 分层。

**修的方向（实现自由，断言不自由）：**

1. `NotificationSink::notify_done` + `AttentionEngine::take_new_done_notifications`；16ms poll 与 `test_poll_once` 都要 drain。Done 只对**非前台** pane（E6 前台会 Idle）。
2. Search tab 必须走 `WorkspacePool::search_all`（已接线）；命中后 `SwitchPane`，VTE 含 token。
3. `tests/scripts/mock_codex.py` 末帧（`TOKEN_HEADER`/`TOKEN_PROMPT`）进 PaneBuf 和 VTE。
4. `/usr/bin/tail -f` 追加行进缓冲。
5. SSH：`TmuxRuntime::new_ssh_attach` + 远端 `-L`；禁止 MockRuntime。
6. tracing target：`muxterm::tmux::seed` / `pause` / `layout` / `surface` / `search` / `notify`。`实时 %output 交付` 不得再 `debug!`。

---

### W15 — dogfood UX + 通知 peek/回复

用户 2026-08-17：attach/切 tab 可用。剩下流量永远 `B/s`、搜索跳转不能用、面板撑破窗口、SSH 冻整窗、SSH 无红绿灯。另外通知要能 **选中 → 渲染该 pane → 快速回复**。

规格与测试清单：[`W15-PLAN.md`](W15-PLAN.md)。顺序 **a → b → e → c → d**，不要和 Herdr/像素混 commit。

| 项 | 抓住 |
|---|---|
| a 流量 | `format_bytes` 1024 一位小数；popover 速率 + 累计；禁止把累计标成 `B/s` |
| b 搜索 | 跨 tab `SwitchTab`+`SwitchPane`；关面板；长行 ellipsize，面板宽 ≤ 窗口 |
| e 通知 | 真 `%output` BEL → `notify_blocked`；Attention 小 VTE 是该 pane；peek 输入进 tmux `capture-pane` |
| c 超时 | `open_spec` 离开 GTK 线程；失败写 `notification_log`；`test_connect_target` 500ms 内把控制权交还 |
| d SSH 灯 | `ssh_probe_args` BatchMode + ConnectTimeout=2；QC 行 `muxterm-ssh-dot-ok/err` |

**Commit 建议：** 一逻辑一英文 commit。不 push。

---

## 6. 明确不做（本轮）

- Herdr Runtime
- 重做 F 像素 / C7 / C8 / Phase E 搜索抛光
- 把 GUI Window 一对一映射成 tmux window（iTerm2 模型）
- 在 Muxterm 里实现可 attach 的 session 服务器
- 把 WezTerm workspace 标签模型当产品层
- 提交 `/home/wlz/Developer/terminal`
- push、改 git config、杀默认 tmux
- 顺手大重构 `emulate.rs` / 主题 / attention UX
- 回滚 `fbc77e4`（render e2e 10pt）
- 把未提交的架构 docs 混进 W2–W7
- 做完一个 W 就停下来等用户

---

## 7. 完成定义（整轮）

- [ ] W1–W8 各一英文 commit，无 Co-authored-by，未 push
- [ ] 产品层无 Session、无虚拟 Window；Window 只是 Workspace 的体现
- [ ] Tab/Pane 是 Workspace 结构；tmux 不出 `runtime/tmux/`
- [ ] WorkspacePool 在 core；platform 无连接池
- [ ] FFI handle = 池；CLI `list-workspaces`；无产品 `list-sessions`
- [ ] Linux Window bind 工作区；切 Tab 不销毁 VTE
- [ ] 搜索三层；Surface e2e 仍绿
- [ ] `fmt` / `check --features gtk` / `test` / `clippy -D warnings`

人工 dogfood（用户）：两个真 tmux 工作区来回切，画面还在、滚动位置还在、搜索能跳到后台格子；SSH Codex 仍不闪（F 契约）。

---

## 8. 每 W 汇报（必须）

```
W?: <commit subject>
hash: <git rev-parse --short HEAD>
tests: fmt / check --features gtk / test / clippy / linux_render_e2e / linux_live_e2e
      （W5+ 加 linux_quickconnect_e2e；W6+ 加 linux_search_e2e）
files: <主要改动路径>
blocked: 无 | <原因>
next: W?
```

每 W 汇报写进该 commit body 或你的回复里，**然后立刻开始下一个 W**。不要停。

---

## 9. 给 Codex 的一段话（可贴；当前从 W2 跑到 W8）

继续。不要等我确认。不要把 thread goal 标 complete，直到 W8 门禁全绿。

工作目录 `/home/wlz/Developer/self/muxterm`，分支 `feat/linux-quickconnect-ui`。HEAD 已有：
- `fbc77e4` test(linux): fit render e2e 80x24 fixture with 10pt font（**不要回滚**）
- `3f19923` feat(core): wrap one runtime in Workspace（W1 完成）
**不要 push。**

先再读：`docs/WORKSPACE.md` §6、`docs/WORKSPACE-PLAN.md`（含「W1 复盘」）、`docs/SURFACE.md`、`AGENTS.md`。

从 **W2** 做到 **W8**。一次一个 W，先 RED 再 GREEN，一 W 一英文 commit，无 Co-authored-by。做完立刻下一个。W8 全绿后再按 §8 汇总停下来。

W2–W7 **不要** `git add` 这些已有脏文件：`AGENTS.md`、`ARCHITECTURE.md`、`PRODUCT.md`、`TASKS.md`、`docs/*`、`tests/samples/dogfood-*`。它们 W8 再收。禁止 `git add -A`。不要建 `/tmp` worktree。测模块用 `cargo test --lib workspace::`。

不要重做 F/C7/C8/E，不要改 live dump，不要 Herdr，不要杀用户默认 tmux，不要在 platform 做 ConnectionPool。

现在开始 **W2**：`feat(core): add WorkspacePool`。


