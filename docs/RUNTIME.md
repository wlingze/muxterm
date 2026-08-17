# RUNTIME.md — Runtime 是什么

> 日期：2026-08-17（`2026-08-17T15:26:26+08:00`）
> 修订：2026-08-17 Catalog（`2026-08-17T22:45:39+08:00`）。
> 分支：`feature/runtime/support_herdr`
> 产品树：[`WORKSPACE.md`](WORKSPACE.md)。Catalog：[`CATALOG.md`](CATALOG.md) / [`CATALOG-PLAN.md`](CATALOG-PLAN.md)。
> tmux 适配：[`LAYER-MAPPING.md`](LAYER-MAPPING.md)。
> Herdr 接入施工：[`HERDR-PLAN.md`](HERDR-PLAN.md)。已有的连接：[`W20-PLAN.md`](W20-PLAN.md)。像素：[`SURFACE.md`](SURFACE.md)。
> 愿景里的阶段 D：`docs/PRODUCT-VISION-STRATEGIC-REVIEW.md` §0.3 / §6 阶段 D。
>
> 核对：本机 `herdr 0.8.0`，socket 协议 **19**；官方 [Concepts](https://herdr.dev/docs/concepts/)、[Socket API](https://herdr.dev/docs/socket-api/)、[CLI](https://herdr.dev/docs/cli-reference/)。

**一句话：** Runtime 是给一个 Muxterm Workspace **填** Tab/Pane、收字节、执行 Task 的接口。tmux、本地 shell、Herdr 都是实现。SSH 不是 Runtime，是 Transport。GUI 不许按实现名字写 `if herdr`，只许问 `support()`。

本文是契约。H0–H4 已在 `feature/runtime/support_herdr`。QuickConnect 已有的连接见 [`W20-PLAN.md`](W20-PLAN.md)。

---

## 1. 词

| 词 | 是什么 | 不是什么 |
|---|---|---|
| **Runtime** | `trait Runtime`。connect / execute / take_events / shutdown，外加 **`support()`** | 池；QuickConnect；GUI Window |
| **Transport** | 字节怎么到对端：本地进程 / `ssh <alias>` | 会不会 split、有没有 worktree |
| **Muxterm Workspace** | 池里一格。用户切换的单位 | tmux `$N`；Herdr 的 named session |
| **Tab / Pane** | Workspace 内部结构。所有 Runtime 都必须给出 | tmux window/pane 本体；Herdr `w2:t1` 本体 |
| **Herdr session** | Herdr **server 命名空间**（默认 socket，或 `--session work`） | 产品层 Session（已删） |
| **Herdr workspace** | Herdr 里一个项目容器（`w2`） | git worktree |
| **git worktree** | 同一仓库的第二份 checkout | Muxterm Workspace |
| **Herdr worktree** | 一份 git checkout **打开成** Herdr workspace，并带 provenance | 产品新层级 |

中文：运行时 / 传输 / 工作区 / 标签 / 格子。Worktree 就叫 worktree，不要再发明「工作树」当类型名。

### 1.1 和 tmux 怎么对齐（口头说法）

Herdr 的 socket 就是 tmux 的 `-L`：一条 server，上面挂很多可切换的格子。格子里是 tab → pane，和 Muxterm、和 tmux window → pane 同一套。

**Herdr 没有 window 这一层。** 你口头说的「一个 window 里多个 tab」在 Herdr 里不存在：tmux 的 window 直接等于 Herdr 的 tab，等于 Muxterm 的 Tab。

| 你刚才说的 | Herdr 官方 | tmux | Muxterm |
|---|---|---|---|
| socket | named session / `herdr.sock` | `-L` 那个 server | `HerdrSession`（连接，不是产品类型） |
| 能 `ls` 出来的多个 space | **workspace** `w2` / `w4` | `tmux ls` 的 session | 池里一格 Workspace |
| tab | tab `w2:t1` | window `@N` | Tab |
| pane | pane `w2:p1` | pane `%N` | Pane |
| workspace / 缩进 / new worktree | **worktree**（git checkout 打开成另一格 workspace） | 没有 | `support()` 里的 `Worktree*` |

容易撞的名字：**Herdr 的 workspace 就是那个 space，不是 worktree 功能。** worktree 只是「这个 space 和某个 repo 的哪份 checkout 有亲缘」，侧栏缩进。`worktree.create` / `open` 之后仍是**单独一格 space**（新的 `workspace_id`），不是当前格子里多一个 tab。

Herdr 相对 tmux 多出来的，产品上就两样：worktree 亲缘 + agent 状态/通知。tab/pane 操作同一套 Task。agent 以后喂现有 attention，不新造侧边栏。

---

## 2. 分层（和代码对齐）

```
platform（Linux GTK / macOS / TUI）
  只画当前 Workspace 的 Tab/Pane
  快捷键 → Task / Pool.activate
  卡和列表来自 Catalog.runtime_list / discover_sessions
  禁止：ssh、tmux 命令、herdr socket 帧、git worktree add、if runtime == "herdr"

core
  Catalog           Driver/Transport 有序表 + Connect + Inventory + Pool
  WorkspacePool     已打开的格子
  Workspace         Tab+Pane + PaneBuf
  trait Runtime     已 attach，见 §4（没有 list）
  catalog Transport local / ssh 插件
  byte transport    spawn_exec / read / write

runtime/tmux        tmux -CC。唯一能出现 $N / %output 的地方
runtime/shell       自管 PTY。没有远端可 attach
runtime/herdr       Herdr socket。唯一能出现 w2:p1 / terminal.frame 的地方
```

今天代码：

- trait 在 `src/core/model/backend.rs`（名字仍叫 `Runtime`）
- 实现：`TmuxRuntime`、`ShellRuntime`、`HerdrRuntime`、`DaemonRuntime`
- `src/core/catalog/` 类型表面已在；`with_builtins()` 还空（施工 [`CATALOG-PLAN.md`](CATALOG-PLAN.md)）
- `WorkspaceSpec.runtime` 是 `"tmux"` / `"shell"` / `"herdr"` / `"daemon"` 字符串
- `WorkspaceSpec.build_runtime()` 仍是字符串 `match`；Catalog::open **禁止**再走这条
- `WorkspacePool.herdr_sessions` 仍是 Herdr 旁路表，要迁进 `Catalog.connects`
- `RuntimeMode` 仍是 2×2 facade；真正打开走 spec / Catalog

---

## 3. 谁填哪一层

```
一条 tmux session          → 一个 TmuxRuntime → 一个 Muxterm Workspace
一个本地 shell 进程组      → 一个 ShellRuntime → 一个 Muxterm Workspace
一个 Herdr server 会话     → 一个 HerdrSession（连接）
  └── 每个 Herdr workspace → 一个 HerdrRuntime 视图 → 一个 Muxterm Workspace
```

**tmux / shell：一个 Runtime 实例填一个 Workspace。** 换另一条 tmux session = 池里另一格。这条已经锁在 [`WORKSPACE.md`](WORKSPACE.md) §3。

**Herdr：必须放松「一个 socket 一个 Workspace」。** Herdr 一个 server 里同时有很多 workspace（本机 dogfood：`w2` muxterm、`w4` yaklang-workspace、`w8` legion 挂在同一 `herdr.sock`）。worktree create 还会在**同一个** server 上再开一格。

做法：

1. `HerdrSession`：连一个 socket（默认或 named session，或以后 SSH 远端的 Herdr）。负责 `ping` / `session.snapshot` / `events.subscribe` / 写请求。
2. `HerdrRuntime`：绑定 `HerdrSession` + 一个 Herdr `workspace_id`（如 `w2`）。对 Pool 来说仍是「一个 Runtime 填一个 Workspace」。
3. 同一 `HerdrSession` 可被多个 `HerdrRuntime` 共享（`Arc`）。不要每个 Workspace 再开一条 socket。

GUI 切换 Herdr 项目 = `Pool.activate`，和切 tmux 工作区同一条路。不要把 Herdr 的全部 workspace 压进一个 Muxterm Tab 栏。

关 GUI 窗：Tmux / Herdr **detach**（server 还在）；Shell **shutdown**（进程没了）。

---

## 4. `trait Runtime`

现有方法保留：`connect` / `execute(Task)` / `take_events` / `shutdown`，以及 `as_any`、默认的 `status_subscriptions_active` / `traffic_bytes`。

**新增（本规划要写进 trait 的核心）：**

```text
fn support(&self) -> &'static [RuntimeCapability]
```

GUI、Pool、CLI **只根据这个切片**决定：要不要画「新建 worktree」、点了会不会 `Rejected`、发现层能不能列出候选。禁止：

```text
if spec.runtime == "herdr" { show_worktree_ui() }
```

`execute` 碰到当前 Runtime 没有的能力：返回 `TaskOutcome::Rejected`，不要 panic，不要悄悄 no-op。

### 4.1 `RuntimeCapability`

枚举。一个实现返回它**真会做**的子集。以后只加变体，不加 `can_*` 散装 bool（今天 `status_subscriptions_active()` 这种默认方法，新能力不要再抄）。

| 变体 | 意思 | Tmux | Shell | Herdr |
|---|---|---|---|---|
| `PersistDetach` | shutdown/关窗后远端还在，能再 attach | 是 | 否 | 是 |
| `Discover` | 连接前能列出可 open 的候选 | 是（list-sessions） | 否 | 是（连上 socket 后 `workspace.list`） |
| `MultiTab` | `NewTab` / `SwitchTab` 有意义 | 是 | 是 | 是 |
| `SplitPane` | `SplitPane` 有意义 | 是 | 是 | 是 |
| `WorktreeList` | 能列出当前仓库的 checkout | **否（v1）** | 否 | 是 |
| `WorktreeCreate` | 能建 checkout 并打开成新 Workspace | **否（v1）** | 否 | 是 |
| `WorktreeOpen` | 能打开已有 checkout | **否（v1）** | 否 | 是 |
| `WorktreeRemove` | 能 `git worktree remove`（不删分支） | 否 | 否 | 是（可后做） |

v1 **不要**把这些放进枚举（有需求再加，避免 GUI 为假能力画入口）：

- Herdr agent 检测 / `pane.agent_status_changed`（Muxterm 已有 OSC/BEL；Herdr 只是以后更高保真的源）
- Herdr 插件、graphics、notification.show
- tmux `display-message`、`refresh-client -B`（继续用现有方法，不必为了迁移而改）

`DaemonRuntime`：IPC 客户端，能力等于它背后那个 Runtime。自己不要谎报 worktree。

### 4.2 产品侧仍看不见的东西

和 [`WORKSPACE.md`](WORKSPACE.md) §5 一样。另外：看不见 `worktree.create`、`terminal.frame`、`w2:p1`、`$N`、`%output`。那些停在对应 `runtime/*` 里，翻译成 Task / StateChange / Pool 上的 worktree 操作。

---

## 5. Worktree（产品能力，不是第三棵树）

Muxterm **不**在 Workspace → Tab → Pane 上面再加一层 Worktree。

Worktree 是 **某些 Runtime 会的事**：认出同一仓库的其它 checkout，并能新建一份，打开成**另一个** Muxterm Workspace。

用户要的最低限度：

1. **识别**：当前格子所在仓库有哪些 checkout、哪个已经在池里打开、分支名、路径。
2. **建立**：指定 branch / base / 可选 path，创建 checkout，并作为新工作区出现在池里。

**Herdr 已有一等 API，Muxterm 要接。** 本机实测（muxterm 主 checkout，Herdr `w2`）：

```json
{
  "source": {
    "repo_key": "/home/wlz/Developer/self/muxterm/.git",
    "repo_root": "/home/wlz/Developer/self/muxterm",
    "source_workspace_id": "w2"
  },
  "worktrees": [
    {
      "branch": "feat/linux-quickconnect-ui",
      "is_linked_worktree": false,
      "open_workspace_id": "w2",
      "path": "/home/wlz/Developer/self/muxterm"
    }
  ]
}
```

`yaklang-workspace`（`w4`）的 `workspace.list` 带 `worktree` provenance（bare repo）。主工作区 `w2` 的 `workspace.get` **没有** `worktree` 字段。识别要以 `worktree.list` 为准，不要只看 workspace 记录。

官方语义（[Socket API](https://herdr.dev/docs/socket-api/) / [CLI](https://herdr.dev/docs/cli-reference/)）：

| Herdr | 做什么 |
|---|---|
| `worktree.list` | 列出该 repo 的 checkout；`workspace_id` 或 `cwd` 二选一，都省则用当前 |
| `worktree.create` | `git worktree add` + 打开成 Herdr workspace，和源 workspace 分组。已有本地分支就 checkout，否则从 `--base` 或 HEAD 建分支 |
| `worktree.open` | 打开已有 checkout；已经打开就返回那格 |
| `worktree.remove` | `git worktree remove`，**不删分支**；脏树要 `--force` |
| `workspace.close` | 只关 Herdr 状态，不动磁盘 checkout |

Muxterm 对应（core，不进 platform）：

```text
WorktreeInfo { path, branch, repo_root, open_workspace: Option<WorkspaceId>, linked: bool }

Pool.list_worktrees(ws) -> Vec<WorktreeInfo>     // 需 WorktreeList
Pool.create_worktree(ws, spec) -> WorkspaceId    // 需 WorktreeCreate；成功 = 池里新开一格
Pool.open_worktree(ws, path|branch) -> WorkspaceId
Pool.remove_worktree(ws)                         // 可后做
```

`create` / `open` 之后走现有 `PoolChanged` + 可选 `activate`。不要为 worktree 再发明一套切换器。

### 5.1 tmux：v1 不做

tmux 没有 worktree。硬做只能是：在 pane cwd 跑 `git worktree`，再 `new-session -c <path>`。没有分组、没有 provenance、session 名规则要自造、还容易误碰用户默认 server。

`TmuxRuntime::support()` **不**包含任何 `Worktree*`。QuickConnect 在 tmux 工作区上不画「新建 worktree」。以后真要做，另开文档，且必须隔离 `-L`。

Shell 同理：进程组不是 git 宿主，也不报 `Worktree*`。

---

## 6. 对照表

### 6.1 tmux（已有，细节 [`LAYER-MAPPING.md`](LAYER-MAPPING.md)）

| tmux | Muxterm |
|---|---|
| 一条 session（按**名字**） | 一个 Workspace + 一个 TmuxRuntime |
| window `@N` | Tab |
| pane `%N` | Pane；字节 → `PaneOutput` |
| 控制 client detach | `Task::Detach`；session 还在 |

### 6.2 Herdr（要接）

| Herdr | Muxterm |
|---|---|
| named session / 默认 `herdr.sock` | `HerdrSession`（连接身份，进 `WorkspaceId` 的 session/path，**不是**产品 Session 类型） |
| workspace `w2` | 一个 Workspace + 一个 `HerdrRuntime` |
| tab `w2:t1` | Tab |
| pane `w2:p1` | Pane |
| `session.snapshot` + `events.subscribe` | 拓扑：Tab/Pane/layout |
| `terminal session observe\|control` 的 `terminal.frame`（base64 ANSI） | `PaneOutput` 原始字节 → VTE `feed` |
| `workspace.*` / `tab.*` / `pane.*` / `layout.updated` | `StateChange` |
| `worktree.*` | §5 Pool API；workspace 记录上的 provenance 只是提示 |
| `pane.agent_status_changed` | **以后**喂现有 attention，不新造 UI。v1 可以不订 |

Herdr 文档原话：`session.snapshot` 是给「自己缓存 runtime 状态的客户端」的一次性 bootstrap；之后靠事件。第三方只要终端字节：`terminal session observe`。这和 Muxterm「自己是客户端，不养 session」对齐。

**pane 上 tmux 与 Herdr 互斥。** Herdr 明说不检测套在 Herdr pane 里的 tmux session。Muxterm 的 OSC/BEL 感知必须继续在 tmux 路径上自己活。接 Herdr 不是把身份租出去。

### 6.3 Shell

自己分配 Tab/Pane id。没有 Discover、没有 PersistDetach、没有 Worktree。从池里扔掉 = 进程结束。

---

## 7. 字节与 Surface

Live 路径不因 Runtime 而改：

- 进当前可见 Pane 的是 **原始字节**（tmux `%output` 解转义后的 payload，或 Herdr `terminal.frame` 解开的 ANSI）
- 禁止 live `visible_ansi` → `vte.reset`
- PaneBuf / 搜索 / 提醒吃同一份 feed
- Herdr `pane.read` 可以当 attach 快照（类似 `capture-pane`），不要当直播

输入：tmux `send-keys` / Herdr `terminal.input` 或 `pane.send_input`。控制权：Herdr control 同一终端同时只有一个 controller；Muxterm 前台 pane 用 control，后台用 observe。

---

## 8. Discovery 与打开

列出发生在 **Driver** 上，不在活 Runtime 上。`RuntimeCapability::Discover` 只是旗标。门面是 Catalog，见 [`CATALOG.md`](CATALOG.md)。

| Runtime | 连接前能列出什么 | 打开 |
|---|---|---|
| tmux | 本机或 ssh 上的 session **名**（内部 `list-sessions`） | `Catalog::open` ← `TmuxDriver` + Connect |
| herdr | Connect 上 `workspace.list`（id + label + 可选 worktree） | `Catalog::open` ← `HerdrDriver`；同一 socket 共用 Connect |
| shell | 目录，不是 session | `Catalog::open` ← `ShellDriver`；只接受 `local` |

`discover_targets(transport)` = 怎么到那儿（SSH hosts / Local 单例）。  
`discover_sessions(transport, target)` = 该 target 上各 Driver 的可 attach 格子。不要叫 `discover-connection`。

QuickConnect 一级按**预设项目**索引，不要按「tmux / Herdr」做两个顶栏。最上固定「已有的连接」（施工 [`W20-PLAN.md`](W20-PLAN.md)）：进去按 **Transport** 分目录（本地 / SSH），目录里 tmux session 和 Herdr workspace 用同一套项目行。Runtime 只是徽章和 `support()` 决定的次级动作（worktree）。卡的来源改 `runtime_list()`，不要硬编码三张卡的存在性（widget_name 保持 W20 的）。

远程 Herdr：Transport `ssh` + Runtime `herdr`。列出用 `ssh … herdr session list` / `workspace list`（和 `ssh … tmux list-sessions` 同类）。打开不要 `herdr --remote`（会在远端装/启 server）：把远端 `herdr.sock` Unix 转发到本机，再走现有 `HerdrSession`。没在跑就跳过，不要替用户启动。探活进 Inventory，不要写在 `window.rs`。

---

## 9. 明确不做

- 产品层 Session、虚拟 Window、把 GUI 窗一对一映射成 tmux window 或 Herdr workspace
- platform 里 git worktree / herdr CLI 拼命令
- 为 Herdr 单独做侧边栏、agent 列表、插件市场
- 用 Herdr 的 agent 状态当 Muxterm 的身份（tmux 侧 OSC/BEL 必须留下）
- v1 在 TmuxRuntime 上假装 worktree
- 改 live 像素契约
- 对用户默认 tmux `kill-server`；对 Herdr `herdr server stop`（测试用隔离 socket / 隔离 Herdr session 名）

---

## 10. 文件落点（实现时）

```
src/core/catalog/             Catalog 门面 + Driver/Transport 表 + Connect + Inventory
src/core/model/backend.rs     trait Runtime + RuntimeCapability + support()
src/core/runtime/mod.rs       导出 Herdr
src/core/runtime/herdr/       仅此处出现 herdr.sock / w2:p1 / terminal.frame
src/core/workspace/spec.rs    WorkspaceSpec（Catalog::open 的入参）
src/core/workspace/pool.rs    槽位表；不再持有 herdr_sessions
```

测试：能力表用单测（假 Runtime 只报 `WorktreeList` 时 Pool 不得调用 create）。Herdr e2e 用**独立 named session**（`herdr --session muxterm-test-<unique>`），不要打用户默认 `herdr.sock`。
