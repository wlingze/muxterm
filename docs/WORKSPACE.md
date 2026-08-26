# WORKSPACE.md — Muxterm 产品结构与 Core 边界

> 定名：2026-08-15 23:41 CST（`2026-08-15T23:41:41+08:00`）
> 修订：2026-08-17 Catalog（`2026-08-17T22:45:39+08:00`）；2026-08-22 Herdr
> WorkspaceId/foreground contract。
> 2026-08-24 纠偏：Runtime 交出布局事件 + PTY 字节；Workspace 不画像素。见 [`SURFACE.md`](SURFACE.md) §7（核查 `2026-08-24T16:55:34+08:00`）。
> Catalog：[`CATALOG.md`](CATALOG.md)。
> 像素：[`SURFACE.md`](SURFACE.md)（F 已交）。适配表：[`LAYER-MAPPING.md`](LAYER-MAPPING.md)（只给 `runtime/tmux` 看）。
> Runtime 契约：[`RUNTIME.md`](RUNTIME.md)。Herdr 稳定性与身份见
> [`HERDR-RUNTIME-STABILITY.md`](HERDR-RUNTIME-STABILITY.md)。

**一句话：** Muxterm 自己的结构是 **Catalog → WorkspacePool → Workspace → Tab → Pane**。GUI **Window 只是某个 Workspace 的体现**。tmux / Herdr / Shell 是 Driver；SSH 是 Transport。前端只渲染，不养池、不养连接。

---

## 1. 谁拥有结构

```
Muxterm 产品（Core Protocol / FFI / CLI / GUI 都用这一套）
─────────────────────────────────────────────────────────
Catalog                       ← FFI 持有；Driver/Transport 表 + Connect + Inventory + Pool
  └── WorkspacePool           ← 已打开的格子；只在 core
        └── Workspace*        ← 池里一格；一个已 attach 的 Runtime
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
| **Pane** | 最小格子：拓扑节点 + Index（PaneBuf）+ 前端 Surface | 显示网格的真相不在 PaneBuf |
| **Catalog** | backend 总状态。两张插件表 + Connect + Inventory + Pool。见 [`CATALOG.md`](CATALOG.md) | GUI Window |
| **Driver** | Runtime 插件（`TmuxDriver` / `HerdrDriver` / `ShellDriver`）。负责 list / open | 已经 attach 的 `trait Runtime` |
| **Connect** | 可复用管道。同一 SSH host / 同一 Herdr socket 一份 `Arc` | 一个 Workspace |
| **Inventory** | 尚未 attach 的 target/session 台账（探活、灯） | Pool 里已打开格子的 `BackendStatus` |
| **WorkspacePool** | 已打开的格子。打开/激活/后台保活/淘汰 | 插件表；platform 里的 `ConnectionPool` |
| **Runtime** | 给 Workspace **填** Tab/Pane 的接口。实现：Tmux / Shell / Herdr。问能力用 `support()`，见 [`RUNTIME.md`](RUNTIME.md) | 池；发现层；用户切换器 |
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
│   Catalog           Driver/Transport 表 + Connect + Inventory│
│   WorkspacePool     已打开：打开 / 列表 / 激活 / 后台吃字节 │
│   Workspace         Tab+Pane 拓扑 + PaneBuf + 当前焦点     │
│   Runtime trait     已 attach：connect / execute / events  │
│   catalog Transport local / ssh 插件（list_targets / connect）│
│   byte Transport    spawn_exec / read / write（现有模块）  │
│   discovery         被 Driver.list 调用，platform 不直接调 │
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
| **platform** | 每个已打开 pane 一台 VT；`feed` `PaneOutput`；按 `LayoutChanged` 改分割；快捷键 | 池、ssh、`-CC`、`capture-pane` |
| **core Workspace** | Pool、Tab/Pane 拓扑、Index（搜索/提醒）、把 Task 交给 Runtime | 像素；解析 `%output`；把 Index 灌回 Surface |
| **runtime/tmux** | 拆控制协议 vs PTY；翻译成 `StateChange`；send-keys | 产品类型；GUI |

产品层可以对 Runtime **种类** 说话（打开一个 tmux 工作区还是本地 shell），不对 tmux **协议** 特化。

Live 显示仍走 Surface：原始字节按 `(WorkspaceId, PaneId)` 进入产品拓扑中常驻的 VTE；
hidden tab/background workspace 继续 feed，只是不绘制。PaneBuf 给搜索/提醒，禁止
`visible_ansi` dump。

Workspace 不是无限的。`WorkspacePoolPolicy.max_slots` 默认 5，超出按 LRU detach。后台格子继续吃字节进 Index；像素 Surface 只保留已经建过的。

---

## 3. 标准 Workspace 结构（Core 拥有）

每个 Workspace：

```
Workspace
  id: WorkspaceId          # 稳定；不是 $N
  name: String             # 用户看见的名字（tmux 时常用 session 名）
  resolved_target: Option<ResolvedTarget>
                           # Catalog 打开时保存；Recent/重连的规范身份来源
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

`WorkspaceId` 由连接身份构成，现有稳定字符串保持五段
`transport / alias / session / runtime / identity`，不是 `$N`。tmux/shell 没有独立 target
id 时第五段使用 path；Herdr 使用独立 `workspace_id`，项目 `path` 仍作为
`WorkspaceSpec` 元数据保留。当前结构的第五段字段即使仍暂名 `path`，也不能反向当成
Project 目录。详见 [`HERDR-RUNTIME-STABILITY.md`](HERDR-RUNTIME-STABILITY.md) §7。

`ResolvedTarget` 同时保存规范化 `TargetConfig` 与实际打开用 `WorkspaceSpec`，类型固定在
`src/core/catalog/resolver.rs`。Catalog 的 Project/Recent/Existing 路径必须填它；测试 mock/
旧 CLI 直开可以为 None。`Workspace` 是 Pool 内唯一所有者，`PooledWorkspace` 与 platform
不得复制第二份。Recent、当前连接高亮和重连只能读这份 Core 元数据，禁止 platform 从
`WorkspaceId` 五段字符串反向猜 path/socket/workspace id。其
identity key 包含 transport target、runtime、session、target-side socket 与 workspace_id；
Project name/path 只是显示/项目元数据，不是 attach identity。

descriptor 作为完整 value 在 open 时注入；相同 identity 的 slot 复用只允许 Core 整值补全
缺失的 canonical name/path，不允许改变 attach identity。Workspace 用户可见名称来自非空
`ResolvedTarget.canonical.name`，不能把 Herdr named session 当成 Project 名。

---

## 4. WorkspacePool（旧连接池）

以前：`platform/*/quickconnect/pool.rs` + `WarmConnectionSlot` 养连接。
现在：**只有** `src/core/workspace/pool.rs`。

池负责：

- `open`：建 Workspace + Runtime.connect，放进池
- `list`：池里有哪些（含后台）
- `resolved target`：Workspace 保留 Catalog 规范化的连接描述，供 Recent/重连读取；Pool
  只通过 Workspace 查询，不另存 side table
- `activate(id)`：前台至多一个（单窗时）；其余后台继续 `take_events` 喂 PaneBuf
- background event routing：Pool 返回 `(WorkspaceId, events)`；platform 的常驻 Surface
  registry 仍按 `(WorkspaceId, PaneId)` raw feed，不能只处理 attention 后丢像素事件
- foreground 转换：只由 Pool 调用 Runtime 的通用 `set_foreground()`；platform 不按
  runtime 名字判断
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

看不见：`list-sessions`、`%output`、`$N`、`send-keys -H`、Herdr 的 `w2:p1` / `terminal.frame`。那些是对应 `runtime/*` 内部把事件翻译成上面的 `StateChange`。

能力用 `Runtime::support() -> &'static [RuntimeCapability]`，不要 `can_list_sessions`，也不要 GUI `if runtime == "herdr"`。完整枚举与 worktree：[`RUNTIME.md`](RUNTIME.md) §4–§5。

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

留下：`PaneOutput`、`PaneFrame`、`PaneSnapshot`、`PaneHistory`、只给 Index 的 `PaneIndexSnapshot`、
`TabAdded/Closed/Renamed`、`TabOrderChanged`、`ActiveTabChanged { tab }`、`LayoutChanged`、
`PaneAdded/Closed/Title/Resized`、`ActivePaneChanged`、异步创建最终结果
`MutationSettled`、`StatusBarSubscription`、`WorkspaceRenamed`、`PoolChanged`（列表变了）、
Runtime 连接状态。

删掉：`SessionChanged`、`SessionsChanged`、`WindowAdded/Closed/…`、`ActiveWindowChanged`。

**`Task`**

留下：Tab（`NewTab` 不再带 `WindowId`）、Pane 分割/焦点/尺寸/输入、`RenameWorkspace`、`Detach`、`Shutdown`。

删掉：`SwitchSession`、`RenameSession`、`NewWindow`/`CloseWindow`/`SwitchWindow`/`RenameWindow`。换工作区走 **Pool.activate / open**，不是 Task。

`TaskOutcome::Done` 只表示同步操作已完成；异步 NewTab/SplitPane 入队返回
`Accepted { operation_id }`，最终 Completed/Failed 只由同 id 的 `MutationSettled` 表达。
platform 不得把 Accepted 当完成并主动重拍 UI。

### 6.2 FFI（现有 W7 基线 + 本轮 additive 变更）

**一个 handle = 整个 `Catalog`**（Pool 在里面；进程里 GUI 通常只拿一个）。见
[`CATALOG.md`](CATALOG.md) C5。现有 FFI handle 已经持有 Catalog；本轮不重建 handle，
而是在现有裸 spec open 上追加 descriptor-aware target open，并补齐异步 mutation 的
Accepted/settlement 表达。

| 现在 | 说明 |
|---|---|
| `muxterm_new()` → 空 Catalog | 不再一个 handle 一条连接 |
| `muxterm_runtime_list_json` / `transport_list_json` | 新建项目卡 / Local·SSH（C5） |
| `muxterm_discover_targets_json` / `discover_sessions_json` | target = 怎么到那儿；session = 可 attach 格子 |
| `muxterm_workspace_open(h, spec)` | 保留的低层兼容入口；走裸 spec，descriptor=None，不供 Project/Recent/Existing 新路径使用 |
| `muxterm_workspace_open_target_json(h, target, intent)` | **新增产品入口**；Project/Recent/Existing 走 resolver，保存 resolved descriptor |
| `muxterm_workspace_list(h, out)` | **池里的**工作区（旧 session list 的在线部分） |
| `muxterm_workspace_activate(h, id)` | 当前体现到 GUI Window 上的那一个 |
| `muxterm_workspace_close(h, id)` | tmux=detach；shell=shutdown |
| `muxterm_discover_workspaces_json(spec)` | 候选（未进池）。内部才 `list-sessions` |
| `muxterm_workspace_create(spec)` | 名字是工作区；runtime=tmux 时 adapter 去 new-session |
| `muxterm_get_tabs` / `get_panes` / `get_layout` | 相对 **当前** workspace | 结构不变 |
| `muxterm_poll_events` | 旧 active-workspace-only 兼容入口；Core 仍可在内部 poll 后台供 Index/attention 使用，但不向旧 ABI 混入无 WorkspaceId 的后台 Surface event |
| `muxterm_poll_workspace_events` | **新增 additive 入口**；`CWorkspaceStateChange { workspace_id, event }` 返回 active/background 事件，platform 按 `(WorkspaceId, PaneId)` 路由 |
| `muxterm_execute_json` / `STATE_MUTATION_SETTLED=16` | additive ABI：返回 Accepted operation id；settlement JSON 放既有 event data buffer |
| `CTask` 无切 session | 切工作区用 `workspace_activate`，不是 Task |
| `muxterm_status_snapshot_json(..., session)` | 保留（status 查询独立于连接） |

`CTab` / `CPane` / `CLayoutNode` 保留。`CStateChange.window_id` 字段保留为 0（W7 未删，避免破坏 macOS ABI）。

兼容：`muxterm_new(backend, socket, session)` / `muxterm_new_connect(...)` 是 deprecated 薄封装 = `new` + `workspace_open`。macOS 未改交互前靠这层。**新 Linux 只走池 API。**

`muxterm_workspace_open(...)` 本轮也降为 descriptor=None 的低层兼容入口，签名不追加字段；
新产品路径用 `muxterm_workspace_open_target_json(...)`。`muxterm_workspace_list` JSON 可追加
optional `resolved_target`，旧消费者忽略未知字段。`CStateChange` struct 布局保持不变；新
settlement 事件的完整内容放进既有 `data/data_len`。workspace-aware poll 用新增 wrapper
struct 包住原 `CStateChange`，不复用 `window_id`；该字段继续为 0。一个 platform 实例不能
同时调用旧、新 poll 去竞争同一队列。

Discovery 的 JSON 形状（产品）：

```json
{ "workspaces": [
    { "id": "local/local/tmux/yaklang-workspace",
      "name": "yaklang-workspace",
      "runtime": "tmux",
      "transport": "local",
      "target": "local",
      "in_pool": false },
    { "id": "ssh/self/tmux/yaklang-workspace",
      "name": "yaklang-workspace",
      "runtime": "tmux",
      "transport": "ssh",
      "target": "self",
      "in_pool": false }
]}
```

`transport` = 插件 id（`local` / `ssh`）。`target` = **connect name**（本机 `local`，或 SSH Host alias）。禁止把 Host 名写进 `transport` 然后丢掉插件 id。

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
src/core/catalog/       Catalog + Driver + Transport 插件 + Connect + Inventory
src/core/workspace/     pool.rs + workspace.rs + pane_buf.rs + id.rs
src/core/runtime/       trait Runtime
src/core/runtime/tmux/  全部 tmux（含 list-sessions 实现）
src/core/runtime/shell/ ShellRuntime
src/core/runtime/herdr/ 全部 Herdr socket（H0–H4 已落地）
src/core/protocol/ffi/  handle = Catalog（现仍是 WorkspacePool，C5 换）
src/core/discovery/     被 Driver.list 调用；返回 Workspace 候选，不返回产品 Session
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
- 把 Herdr 做成身份；规划见 [`RUNTIME.md`](RUNTIME.md)。
- 破坏 Surface：live 只能 `feed` 原始 PTY 字节。
- 让 Workspace/PaneBuf 当显示缓存再 dump 给前端。
- 无限个 Workspace 同时养全套前端 VT。洪水 pane 的 `pause-after` 见 [`SURFACE.md`](SURFACE.md) §7.4 TODO。
