# WORKSPACE.md — Muxterm 产品结构与 Core 边界

> 定名：2026-08-15 23:41 CST（`2026-08-15T23:41:41+08:00`）
> 施工：[`WORKSPACE-PLAN.md`](WORKSPACE-PLAN.md)。像素：[`SURFACE.md`](SURFACE.md)（F 已交）。
> 适配表：[`LAYER-MAPPING.md`](LAYER-MAPPING.md)（只给 `runtime/tmux` 看）。

**一句话：** Muxterm 自己的结构是 **WorkspacePool → Workspace → Tab → Pane**。GUI **Window 只是某个 Workspace 的体现**。tmux 只是 Runtime 的一种实现，全部关在 `runtime/tmux/`。前端只渲染，不养池、不养连接。

---

## 1. 谁拥有结构

```
Muxterm 产品（Core Protocol / FFI / CLI / GUI 都用这一套）
─────────────────────────────────────────────────────────
WorkspacePool                 ← 以前叫连接池；只在 core
  └── Workspace*              ← 池里一格；一个 Runtime
        ├── Tab*              ← 工作区内部结构（标准）
        │     └── Pane*       ← 最小格子：buffer + 终端画面
        └── PaneBuf / layout  ← 无头状态，给搜索/提醒/快切

platform
─────────────────────────────────────────────────────────
Window                        ← 一个 Workspace 的体现（画出来）
  快捷键、面板、把当前 Tab 的 Pane 画成 VTE/SwiftTerm
  不持有 Pool，不 ssh，不拼 tmux 命令

runtime/tmux（外人看不见）
─────────────────────────────────────────────────────────
把 tmux 的 session/window/pane 填进上面的 Workspace/Tab/Pane
$N、@N、%output、send-keys、-CC 全部停在这里
```

**Tab 和 Pane 不是「tmux 的东西被前端借用」。** 它们是 Workspace 的标准内部结构。ShellRuntime 没有 tmux，也必须给出同一套 Tab/Pane。TmuxRuntime 只是把 tmux 树 **适配** 进来。

### 1.1 定名

| 词 | 是什么 | 不是什么 |
|---|---|---|
| **Workspace** | 池里一格。Muxterm 的工作区。内含 Tab → Pane | tmux session（那是适配器内部） |
| **Tab** | Workspace 里的一页 | GUI 窗口；tmux window 本体 |
| **Pane** | 最小格子：有 PaneBuf，前端在这里画终端 | |
| **WorkspacePool** | 以前的连接池。打开/激活/后台保活/淘汰 | platform 里的 `ConnectionPool` |
| **Runtime** | 给 Workspace **填** Tab/Pane 的接口。实现：Tmux / Shell / 以后 Herdr | 池；用户切换器 |
| **Window** | GUI 窗口 = **一个 Workspace 的体现** | 产品树节点；tmux window；旧虚拟 `w1` |
| **Session** | **产品层没有。** 只在 `runtime/tmux` 叫 `TmuxSessionId`（`$N`） | FFI/CLI/GUI 类型 |

中文：工作区 / 标签 / 格子 / 窗口。面板「工作区」= WorkspacePool 里的格子。

### 1.2 Window 是体现，不是一层

产品树 **没有** Window 这一层。

- 一个 GUI Window **绑定** 池里当前（或指定的）一个 Workspace，把它的 Tab/Pane 画出来。
- 切工作区 = 同一扇窗改绑另一个 Workspace，不是新建产品层。
- 以后可以多扇窗绑池里不同 Workspace；那仍是「多个体现」，不是 Core 里长出 `WindowId`。
- 关 GUI 窗：TmuxRuntime **detach**（远端还在）；ShellRuntime **shutdown**（进程没了）。

### 1.3 为什么砍 Session / 虚拟 w1

旧协议硬造 `Session → w1 → Tab → Pane`，和 Workspace 重复，又和 GUI Window 撞名。现在：结构是我们的；tmux 是接口。

---

## 2. 分层职责

```
┌──────────────────────────────────────────────────────────┐
│ platform = 渲染                                           │
│   Window：体现某个 Workspace                              │
│   画：tab 栏、分割、当前 Pane 的终端（VTE / SwiftTerm）     │
│   收：快捷键 → 调用 Core（Task / pool.activate）           │
│   禁止：ConnectionPool、ssh、tmux 命令、%output 解析       │
└──────────────────────────▲───────────────────────────────┘
                           │ Core Protocol（Workspace / Tab / Pane）
┌──────────────────────────┴───────────────────────────────┐
│ core                                                      │
│   WorkspacePool     打开 / 列表 / 激活 / 后台吃字节 / 淘汰  │
│   Workspace         Tab+Pane 拓扑 + PaneBuf + 当前焦点     │
│   Runtime trait     connect / execute(Task) / events       │
│   transport         local / ssh（Runtime 用，platform 不用）│
│   discovery         能 attach 的候选（名字），不是产品 Session│
│   attention/search  读 PaneBuf                             │
│   protocol/ffi + CLI  见 §6                                 │
└──────────────────────────▲───────────────────────────────┘
                           │ 只有 TmuxRuntime 走这条
┌──────────────────────────┴───────────────────────────────┐
│ runtime/tmux          唯一允许出现 tmux 词的地方            │
│   protocol / command / client / pty                        │
│   tmux session → 填 Workspace                              │
│   tmux window  → 填 Tab                                    │
│   tmux pane    → 填 Pane + 原始字节                        │
└──────────────────────────────────────────────────────────┘
```

| 层 | 做 | 不做 |
|---|---|---|
| **platform** | 画 Window；画 Tab/Pane 控件；快捷键；面板外观 | 池、连接生命周期、ssh、tmux |
| **core** | Pool、Workspace 结构、搜索、提醒、FFI/CLI | 像素；tmux 协议 |
| **runtime/tmux** | `-CC`、解析、send-keys、把 tmux 树填进 Workspace | 产品类型；GUI |

前端功能清单（快捷键、命令面板、搜索 UI、attention 点）**不变**。变的是：这些功能读的是 Core 已经维护好的 WorkspacePool / PaneBuf，前端不再自己做连接池。

Live 显示仍走 Surface：原始字节进当前可见 Pane 的 VTE。PaneBuf 给搜索/提醒，禁止 `visible_ansi` dump。

---

## 3. 标准 Workspace 结构（Core 拥有）

每个 Workspace：

```
Workspace
  id: WorkspaceId          # 稳定；不是 $N
  name: String             # 用户看见的名字（tmux 时常用 session 名）
  runtime: Tmux | Shell | …
  transport: Local | Ssh { alias }
  tabs: [Tab]
    Tab
      id: TabId
      name, active
      layout: 嵌套分割树（叶子 = PaneId）
      panes: [Pane]
        Pane
          id: PaneId
          title, cols, rows, active
          buf: PaneBuf     # 有界网格 + 有界字节环 + viewport
```

- **一个 Runtime 填一个 Workspace。** 换另一条 tmux session = 池里另一个 Workspace，不是 Workspace 内部切 Session。
- ShellRuntime 自己造 Tab/Pane（可以一直只有一个 Tab 一个 Pane）。不要求持久化：从池里扔掉 = 进程结束。
- TmuxRuntime detach 后远端 session 还在，可以再 open 进池。

`WorkspaceId` 由连接身份构成（transport / alias / name / runtime / path），与今天 `ConnectionKey` 同构。不是 `$N`。

---

## 4. WorkspacePool（旧连接池）

以前：`platform/*/quickconnect/pool.rs` + `WarmConnectionSlot` 养连接。  
现在：**只有** `src/core/workspace/pool.rs`。

池负责：

- `open`：建 Workspace + Runtime.connect，放进池
- `list`：池里有哪些（含后台）
- `activate(id)`：前台至多一个（单窗时）；其余后台继续 `take_events` 喂 PaneBuf
- 淘汰：超容量 / TTL → Tmux detach，Shell shutdown
- 搜索/提醒跨池里所有 Workspace

platform **不得**再实现第二套淘汰/复用。macOS/Linux 的 slot 若还在，只表示「这个 WorkspaceId 的 **像素控件缓存**」（VTE 别扔掉），连接本身在 core 池里。

---

## 5. Runtime 契约（产品侧）

Platform 与 Core Protocol **只看见**：

```
connect / shutdown / detach
execute(Task)           # NewTab / SwitchTab / SplitPane / SendKeys / …
take_events() → StateChange   # PaneOutput(原始字节) / TabAdded / LayoutChanged / …
snapshot: tabs, panes, layout
```

看不见：`list-sessions`、`%output`、`$N`、`send-keys -H`。那些是 `TmuxRuntime` 内部把事件翻译成上面的 `StateChange`。

`Capability`：`can_attach` / `can_discover`（能否列出可 open 的候选）/ `can_display_message`。不要 `can_list_sessions`。

---

## 6. 对外接口（锁死）

原则：CLI、FFI、JSON **只说 Workspace / Tab / Pane**。发现层列出的也是「可 open 的工作区候选」，即使 TmuxRuntime 内部跑的是 `tmux list-sessions`。

### 6.1 Core 快照 / 事件 / 任务

**快照**

| 类型 | 字段（产品） |
|---|---|
| `WorkspaceInfo` | `id`, `name`, `runtime`, `transport`, `active_tab` |
| `TabInfo` | `id`, `name`, `workspace`, `active`（**无** `window`） |
| `PaneInfo` | `id`, `tab`, `title`, `cols`, `rows`, `active` |

**`StateChange`（对前端）**

留下：`PaneOutput`、`TabAdded/Closed/Renamed`、`ActiveTabChanged { tab }`、`LayoutChanged`、`PaneAdded/Closed/Title/Resized`、`ActivePaneChanged`、`StatusBarSubscription`、`WorkspaceRenamed`、`PoolChanged`（列表变了）、Runtime 连接状态。

删掉：`SessionChanged`、`SessionsChanged`、`WindowAdded/Closed/…`、`ActiveWindowChanged`。

**`Task`**

留下：Tab（`NewTab` 不再带 `WindowId`）、Pane 分割/焦点/尺寸/输入、`RenameWorkspace`、`Detach`、`Shutdown`。

删掉：`SwitchSession`、`RenameSession`、`NewWindow`/`CloseWindow`/`SwitchWindow`/`RenameWindow`。换工作区走 **Pool.activate / open**，不是 Task。

### 6.2 FFI（W7 已落地）

**一个 handle = 整个 `WorkspacePool`**（进程里 GUI 通常只拿一个）。

| 现在 | 说明 |
|---|---|
| `muxterm_new()` → 空池 | 不再一个 handle 一条连接 |
| `muxterm_workspace_open(h, spec)` | spec：runtime / transport / name / socket / ssh / dir |
| `muxterm_workspace_list(h, out)` | **池里的**工作区（旧 session list 的在线部分） |
| `muxterm_workspace_activate(h, id)` | 当前体现到 GUI Window 上的那一个 |
| `muxterm_workspace_close(h, id)` | tmux=detach；shell=shutdown |
| `muxterm_discover_workspaces_json(spec)` | 候选（未进池）。内部才 `list-sessions` |
| `muxterm_workspace_create(spec)` | 名字是工作区；runtime=tmux 时 adapter 去 new-session |
| `muxterm_get_tabs` / `get_panes` / `get_layout` | 相对 **当前** workspace | 结构不变 |
| `muxterm_poll_events` | 池事件：后台工作区照样出字节，GUI 可只画当前 |
| `CTask` 无切 session | 切工作区用 `workspace_activate`，不是 Task |
| `muxterm_status_snapshot_json(..., session)` | 保留（status 查询独立于连接） |

`CTab` / `CPane` / `CLayoutNode` 保留。`CStateChange.window_id` 字段保留为 0（W7 未删，避免破坏 macOS ABI）。

兼容：`muxterm_new(backend, socket, session)` / `muxterm_new_connect(...)` 是 deprecated 薄封装 = `new` + `workspace_open`。macOS 未改交互前靠这层。**新 Linux 只走池 API。**

Discovery 的 JSON 形状（产品）：

```json
{ "workspaces": [
    { "id": "local/tmux/yaklang-workspace",
      "name": "yaklang-workspace",
      "runtime": "tmux",
      "transport": "local",
      "in_pool": false }
]}
```

不要 `{ "sessions": [ { "id": "$4" } ] }`。

### 6.3 CLI（W3/W7 已落地）

用户语言与 FFI 同一套。`-s` = 工作区名。

| 现在 | 旧名 |
|---|---|
| `list-workspaces` | `ls` 仍可用 |
| `new-workspace` | `new-session` / `new` 暂留 alias |
| `attach-workspace` | `attach-session` / `attach` 暂留 |
| `close-workspace` | `kill-session`（tmux 路径 = detach，不杀默认 server） |
| `rename-workspace` | `rename-session` |
| `new-tab` / `list-tabs` / `list-panes` / `split-pane` / … | 保留，作用域是 **当前或 `-s` 指定的 Workspace**；`list-tabs` 不再要 `-w` |
| `muxterm tmux session …` | 仅调试；不要第三套 session API |
| 全局 `-s` | 工作区名，不是 `$N` |

`list-windows` 作为产品命令删除（那是虚拟 w1）。若有人当 tmux 别名用，文档写明：请用 `list-tabs`。

CLI 实现放 `src/platform/cli/` 可以，但 **只调 Core**（池 / Task）。禁止 CLI 自己 `tmux list-sessions`（`discovery.rs` / `tmux_dialog.rs` 的直调要收到 TmuxRuntime / discovery 的产品 API 后面）。

---

## 7. 目标目录

```
src/core/workspace/     pool.rs + workspace.rs + pane_buf.rs + id.rs
src/core/runtime/       trait Runtime
src/core/runtime/tmux/  全部 tmux（含 list-sessions 实现）
src/core/runtime/shell/ ShellRuntime
src/core/protocol/ffi/  handle = WorkspacePool
src/core/discovery/     返回 Workspace 候选，不返回 SessionInfo
src/platform/linux|macos|tui
  window.rs             体现：bind 当前 WorkspaceId，画 Tab/Pane
  keymap / 面板         只调 FFI
  （无 ConnectionPool）
```

`ReplicaStore` 并进 `Workspace.panes`。platform 的 `quickconnect/pool.rs` 删除或变成对 FFI list 的 UI 模型（不含生命周期）。

---

## 8. 明确不做什么

- 产品层再引入 Session / 虚拟 Window。
- platform 实现连接池、ssh、tmux。
- 把 GUI Window 一对一映射成 tmux window（iTerm2）。
- 把 WezTerm 的 workspace 标签当产品层。
- 本轮 Herdr。
- 破坏 Surface：live 仍只 `vte.feed` 原始字节。
