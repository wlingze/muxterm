# WORKSPACE.md — Muxterm 产品结构与 Core 边界

Catalog：[`CATALOG.md`](CATALOG.md)。Runtime：[`RUNTIME.md`](RUNTIME.md)。
像素：[`SURFACE.md`](SURFACE.md)。配置：[`CONFIG.md`](CONFIG.md)。
身份：[`ID-SYSTEM.md`](ID-SYSTEM.md)。Herdr 身份细节：[`HERDR-RUNTIME-STABILITY.md`](HERDR-RUNTIME-STABILITY.md) §7。
tmux 适配（只给 `runtime/tmux`）：[`LAYER-MAPPING.md`](LAYER-MAPPING.md)。

**一句话：** Muxterm 的结构是 **Muxterm → WorkspacePool → Workspace → Tab → Pane**。
**Workspace = Runtime(Transport) + path**。GUI Window 只是某个 Workspace 的体现。
tmux / Herdr / Shell 是 Runtime provider；local / SSH 是 Transport provider。
前端只渲染，不养池、不养连接、不构造 `WorkspaceSpec`。

---

## 1. 谁拥有结构

```text
Frontend（cli / tui / linux / macos / windows）
─────────────────────────────────────────────────────────
Window / TUI / CLI          体现某个 Workspace
快捷键、面板、常驻 Scene
禁止：ssh、tmux 命令、%output、Herdr 帧、连接池、WorkspaceSpec

C FFI + ffi_client
─────────────────────────────────────────────────────────
MuxtermHandle = Box<Muxterm>

Muxterm（组合根，core/muxterm.rs）
─────────────────────────────────────────────────────────
Config
Catalog                     能打开什么：providers 视图 + inventory + resolver
Projects                    Project / Worktree；引用 WorkspaceTemplate
Activity                    commands / agents / attention
transport::ConnectionRegistry
WorkspacePool
  └── Workspace*            = Runtime instance (Transport) + path
        ├── Tab* → Pane*
        ├── Index           搜索 / OSC 133 / BEL（不是显示缓存）
        └── provenance      { project_id?, worktree_id? }

runtime/<name>/             具体 Runtime 隐藏；自注册 RuntimeProvider
transport/<name>/           自注册 TransportProvider
```

**Tab 和 Pane 不是「tmux 的东西被前端借用」。** 它们是 Workspace 的标准内部结构。
ShellRuntime 没有 tmux，也必须给出同一套 Tab/Pane。TmuxRuntime 只是把 tmux 树适配进来。

### 1.1 定名

| 词 | 是什么 | 不是什么 |
|---|---|---|
| **Muxterm** | 一个产品会话。FFI handle 指向它 | Catalog；某一个 tmux 连接 |
| **Workspace** | 可连接实例：`Runtime(Transport) + path` | tmux session；Herdr named session；git worktree |
| **Tab** | Workspace 里的一页 | GUI 窗口；tmux window 本体 |
| **Pane** | 最小格子：拓扑节点 + Index + 前端 Surface | 显示网格的真相不在 Index |
| **Catalog** | 「能打开什么」的目录。见 [`CATALOG.md`](CATALOG.md) | 组合根；Pool；连接表 |
| **RuntimeProvider** | 注册表里的 Runtime 插件。`new_instance(conn, spec)` | 已经 attach 的 `trait Runtime` |
| **TransportProvider** | local / ssh。`list_targets` + `connect` → `TargetConnection` | 一条 pane 字节流 |
| **TargetConnection** | 到某个 target 的可复用连接 | Workspace；ByteChannel |
| **ByteChannel** | Runtime 拿到的有序字节管道 | Transport provider |
| **WorkspacePool** | 已打开的 Workspace。open / list / activate / close | 插件表；frontend 的 slot 表 |
| **Project** | 持久化项目管理记录 | Workspace；Tab |
| **Worktree** | Project 下的 git checkout 记录 | 产品树层级；Herdr workspace id |
| **WorkspaceTemplate** | 新建会话时的 Tab/Pane 初始结构 | attach 已有会话的真相 |
| **Window** | GUI 窗口 = 一个 Workspace 的体现 | 产品树节点；tmux window |
| **Session** | **产品层没有。** 只在 `runtime/tmux` 叫 `TmuxSessionId`（`$N`） | FFI/CLI/GUI 类型 |

中文：工作区 / 标签 / 格子 / 窗口。面板「工作区」= WorkspacePool 里的格子。

### 1.2 Window 是体现，不是一层

产品树 **没有** Window 这一层。

- 一个 GUI Window **绑定** 池里当前（或指定的）一个 Workspace，把它的 Scene 画出来。
- 切工作区 = 同一扇窗改可见 Scene，不是新建产品层，也不是点一下再校准 Core。
- 以后可以多扇窗绑池里不同 Workspace；那仍是「多个体现」，不是 Core 里长出 `WindowId`。
- 关 GUI 窗：有 `PersistDetach` 的 Runtime **detach**（远端还在）；shell **shutdown**（进程没了）。

### 1.3 为什么砍 Session / 虚拟 w1

旧协议硬造 `Session → w1 → Tab → Pane`，和 Workspace 重复，又和 GUI Window 撞名。现在：结构是我们的；tmux 是接口。

---

## 2. Workspace 定义

```text
Workspace = Runtime interface ( Transport interface ) + path
```

- **Runtime**：统一 `trait Runtime`。实现 herdr / tmux / shell；shell 是默认基础 runtime。
- **Transport**：Runtime 只经 `TargetConnection::open_channel` 拿 `ByteChannel`，看不见 local/ssh。
- **path**：工作目录锚点（cwd）。path 不携带 worktree 功能，也不在字符串上做 worktree 匹配。

Workspace 还携带 `provenance { project_id?, worktree_id? }`：侧栏按 Project 分组、worktree 缩进的唯一依据。

`WorkspaceId` 由连接身份构成，稳定字符串保持五段
`transport / alias / session / runtime / identity`，不是 `$N`。tmux/shell 没有独立 target
id 时第五段使用 path；Herdr 使用独立 `workspace_id`。第五段即使暂名 `path`，也不能反向当成
Project 目录。详见 [`HERDR-RUNTIME-STABILITY.md`](HERDR-RUNTIME-STABILITY.md) §7。

一个 Runtime 实例填一个 Workspace。换另一条 tmux session = 池里另一格，不是 Workspace 内部切 Session。
Herdr 一个 socket 上可以有很多 workspace：`TargetConnection` 共享，每个 Herdr workspace 仍是池里独立一格。

---

## 3. Project / Worktree / Template

```text
Projects（持久化在 config.toml）
  └── Project
        ├── main directory
        ├── Worktree A / B
        ├── template: Option<TemplateName>
        └── target / runtime / transport defaults
```

- **Project** 描述「在哪个项目、哪个目标环境工作」，不直接拥有 Tab/Pane。
- **Worktree** 位于 Projects 层。打开的结果是新 Workspace，带 provenance 归组。
- **WorkspaceTemplate**（`twork`）：类型在 `workspace/template.rs`，持久化 `[[templates]]`。
  只在 intent 结果为 **create** 时经普通 Task（`NewTab` / `SplitPane` / `RenameTab` / `SendKeys`）应用。
  attach 已有会话以远端拓扑为准，不套模板。

Worktree 联动是 Projects service 的唯一入口：

- **native**：Runtime `support()` 含 `Worktree*`（Herdr）→ `Task::WorktreeCreate`
- **generic**：在该 Project 的 TargetConnection 上 `open_channel(Exec git worktree add …)`

两种策略的完成条件相同：池里多一格 Workspace 并归到源 Project。UI 不出现 `if herdr`。
generic 路径**绝不**对用户默认 tmux server 执行破坏性命令。

`Worktree*` 能力收窄为「runtime 原生 worktree」。没有这些能力不等于用户没有 worktree 功能。

---

## 4. 打开路径

```text
Candidate（QuickConnect / 命令面板；frontend 可见）
  kind: Project | Worktree | Existing | Recent
  title / subtitle / badges
  in_pool: Option<WorkspaceId>
  ref: CandidateRef

        用户选择
        ▼
OpenRequest（frontend 可见）
  candidate, intent: AttachOnly | CreateIfMissing
  template?, activate

        Catalog::resolve（唯一能产出 WorkspaceSpec 的地方）
        ▼
WorkspaceSpec（Core 内部）
  runtime_id, transport_id, target, path, session/namespace, template, provenance, intent

        WorkspacePool::open
        ▼
live Workspace + 可选 activate
```

- `in_pool` 有值 → 选中即 activate，不再 open。
- 兼容期内 `muxterm_workspace_open(h, spec)` 只留给测试/迁移，不是产品路径。
- 解析失败必须返回结构化 `ResolveError`，禁止把 SSH 失败静默降级为本地 shell。

WorkspacePool：

- `open` / `list` / `activate` / `close`
- 不再因为容量静默淘汰。`max_slots` 默认 20 是软提醒阈值。
- 对前端的唯一事件出口：合并各 Workspace 批次，标 `WorkspaceId`。
- **没有** frontend 侧的第二套淘汰/复用/warm slot。连接复用是 `ConnectionRegistry`。

---

## 5. 三条 lane 与职责

| 事情 | Runtime instance | Workspace | Pool / Activity |
|---|---|---|---|
| wire 协议（`-CC`、Herdr JSON、PTY） | **是** | 否 | 否 |
| `%output` / base64 还原为原始字节 | **是**（只此一次） | 否 | 否 |
| Control lane | 产出 `ControlEvent` | 应用：Tab/Pane 树是唯一真相 | 打包批次 |
| Render lane | 产出原始字节 + generation/sequence | 按 PaneId 路由；同一份复制给 Index | 打包批次 |
| ANSI 内容（OSC 133、BEL、标题） | 否 | **是**：Index | 否 |
| `RuntimeSignal`（`PaneAgentChanged`、进程名） | **产出** | 与 Index 信号合并 | 否 |
| `ActivityRecord` | 否 | 本工作区归一化 | 跨工作区聚合 |
| Task 执行 | **是** | 转发；校验 pane 归属 | activate/open/close 不是 Task |
| 对前端发送 | 否 | 否 | **是** |

Runtime 不知道 Workspace 名、Project、Template。Workspace 不认识 `%output`、`terminal.frame`、`$N`。

Live 显示：原始字节按 `(WorkspaceId, PaneId)` 进入常驻 Surface。隐藏 Scene 继续 feed，只是不绘制。
禁止 `visible_ansi` dump。详见 [`SURFACE.md`](SURFACE.md)。

---

## 6. 对外接口

原则：CLI、FFI、JSON **只说 Workspace / Tab / Pane / Candidate / OpenRequest / Activity**。
发现层列出的是可 open 的 Candidate，即使 TmuxRuntime 内部跑的是 `tmux list-sessions`。

### 6.1 快照 / 事件 / 任务

**快照**

| 类型 | 字段（产品） |
|---|---|
| `WorkspaceInfo` | `id`, `name`, `runtime`, `transport`, `active_tab`, `provenance` |
| `TabInfo` | `id`, `name`, `workspace`, `active`（**无** `window`） |
| `PaneInfo` | `id`, `tab`, `title`, `cols`, `rows`, `active` |
| `Candidate` | `kind`, `title`, `subtitle`, `badges`, `in_pool`, `ref` |

**对前端的事件（三条 lane）**

Control：`TabAdded/Closed/Renamed`、`TabOrderChanged`、`ActiveTabChanged`、`LayoutChanged`、
`PaneAdded/Closed/Title/Resized`、`ActivePaneChanged`、`MutationSettled`、`WorkspaceRenamed`、
`PoolChanged`、Runtime 连接状态、data-gap barrier。

Render：`PaneOutput`、`PaneFrame`、`PaneHistory`。只给 Index 的 `PaneIndexSnapshot` 不是 Surface 输入。

Activity：`Upsert(ActivityRecord)` / `Remove(ActivityId)`，单调 revision。

删掉：`SessionChanged`、`WindowAdded`、`ActiveWindowChanged`。

**`Task`**

留下：Tab（`NewTab` 不再带 `WindowId`）、Pane 分割/焦点/尺寸/输入、`RenameWorkspace`、`Detach`、`Shutdown`、
有能力时的 `Worktree*`。

删掉：`SwitchSession`、`NewWindow` / `SwitchWindow`。换工作区走 **Pool.activate / open**，不是 Task。

`TaskOutcome::Done` 只表示同步操作已完成；异步 NewTab/SplitPane 入队返回
`Accepted { operation_id }`，最终 Completed/Failed 只由同 id 的 `MutationSettled` 表达。
frontend 不得把 Accepted 当完成并主动重拍 UI。

### 6.2 FFI

**一个 handle = 一个 `Muxterm`。** 不是某一个连接，也不是 Catalog。

产品入口（目标）：

| 调用 | 说明 |
|---|---|
| `muxterm_new()` | 空 Muxterm（Config + Catalog + Pool + …） |
| `muxterm_runtime_list_json` / `transport_list_json` | providers 只读视图 |
| `muxterm_candidates_json` / `muxterm_discover_*` | Candidate / Existing inventory |
| `muxterm_open_json(h, OpenRequest)` | **产品打开入口** |
| `muxterm_workspace_list` / `activate` / `close` | 池 |
| `muxterm_poll_workspace_events` | 三 lane 批次；带 `WorkspaceId` |
| `muxterm_execute_json` | Task；Accepted + MutationSettled |
| project / worktree / activity / config | 按领域拆文件，见 [`PROJECT-STRUCTURE.md`](PROJECT-STRUCTURE.md) |

`muxterm_workspace_open(h, spec)` 降为测试/迁移入口。frontend 永不构造 WorkspaceSpec。

`CTab` / `CPane` / `CLayoutNode` 保留。`CStateChange.window_id` 字段保留为 0（避免破坏现有 ABI），直到 Phase 7 清掉。

`pane_visible_ansi` / `pane_surface_seed_ansi` / `pane_scroll_ansi` **不是** live Surface API。

C 符号保留 `muxterm_` 前缀，按领域后缀规范化。两份 `muxterm.h` 最终合成一个来源。

Rust frontend 走统一 `frontend/ffi_client`（安全 wrapper：handle、DTO、borrowed bytes 复制、error envelope）。
禁止散装 `ffi_bridge`，禁止 `use crate::core::...`。

### 6.3 CLI

用户语言与 FFI 同一套。`-s` = 工作区名。无 subcommand 即 CLI；`tui` / `gui` 是 frontend 入口，不是另一套产品树。

| 现在 | 旧名 |
|---|---|
| `list-workspaces` | `ls` 仍可用 |
| `new-workspace` | `new-session` / `new` 暂留 alias |
| `attach-workspace` | `attach-session` / `attach` 暂留 |
| `close-workspace` | `kill-session`（tmux 路径 = detach，不杀默认 server） |
| `rename-workspace` | `rename-session` |
| `new-tab` / `list-tabs` / `list-panes` / `split-pane` / … | 作用域是当前或 `-s` 指定的 Workspace |
| `muxterm tmux session …` | 仅调试；不要第三套 session API |

`list-windows` 作为产品命令删除。CLI 只调 FFI/Core，禁止自己 `tmux list-sessions`。

---

## 7. 明确不做什么

- 产品层再引入 Session / 虚拟 Window。
- frontend 实现连接池、ssh、tmux、warm slot、`bridgeLock`、前台校准。
- 把 GUI Window 一对一映射成 tmux window。
- 让 Workspace/Index 当显示缓存再 dump 给前端。
- frontend 构造 `WorkspaceSpec` 或引用 `TmuxRuntime`。
- Catalog 回涨成 god facade（持有 Pool / 连接 / 打开编排）。
- 对用户默认 tmux / Herdr server 做破坏性操作。
