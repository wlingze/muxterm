# RUNTIME.md — Runtime 是什么

> 契约冻结：2026-09-08（`2026-09-08T15:02:22+08:00`，Asia/Shanghai）
> 代码现状：trait 仍在 `src/core/model/backend.rs`，`RuntimeMode` 2×2 facade 仍在，
> `DaemonRuntime` 仍是独立 adapter。新代码按本文写。
> 产品树：[`WORKSPACE.md`](WORKSPACE.md)。Catalog：[`CATALOG.md`](CATALOG.md)。
> tmux 适配：[`LAYER-MAPPING.md`](LAYER-MAPPING.md)。
> 像素：[`SURFACE.md`](SURFACE.md)。Herdr 流/身份：[`HERDR-RUNTIME-STABILITY.md`](HERDR-RUNTIME-STABILITY.md)。
> 专项门禁：[`HERDR-TESTING.md`](HERDR-TESTING.md)。
>
> 核对：本机 `herdr` 与官方 [Concepts](https://herdr.dev/docs/concepts/)、
> [Socket API](https://herdr.dev/docs/socket-api/)、[CLI](https://herdr.dev/docs/cli-reference/)
> 仍是 adapter 实现的权威来源；产品层不出现 Herdr wire 名。

**一句话：** Runtime 是给一个 Muxterm Workspace **填** Tab/Pane、收字节、执行 Task 的接口。
tmux、shell、Herdr 都是实现。SSH 不是 Runtime，是 Transport。GUI 不许按实现名字写
`if herdr`，只许问 `support()`。

---

## 1. 词

| 词 | 是什么 | 不是什么 |
|---|---|---|
| **Runtime** | `trait Runtime`。connect / execute / drain_events / shutdown，外加 `support()` | 池；Catalog；GUI Window |
| **RuntimeProvider** | 自注册插件。`new_instance(Arc<dyn TargetConnection>, spec)` | 已经 attach 的实例 |
| **Transport** | 字节怎么到对端：local / `ssh <alias>` | 会不会 split、有没有 worktree |
| **TargetConnection** | 可复用的 target 级连接；`open_channel` 产出 `ByteChannel` | Workspace |
| **Muxterm Workspace** | `Runtime(Transport) + path`。用户切换的单位 | tmux `$N`；Herdr named session；git worktree |
| **Tab / Pane** | Workspace 内部结构。所有 Runtime 都必须给出 | tmux window/pane 本体；Herdr `w2:t1` 本体 |
| **Herdr session** | Herdr **server 命名空间**（默认 socket，或 `--session work`） | 产品层 Session（已删） |
| **Herdr workspace** | Herdr 里一个项目容器（`w2`） | git worktree |
| **git worktree** | 同一仓库的第二份 checkout | Muxterm Workspace |
| **Herdr worktree** | 一份 git checkout **打开成** Herdr workspace，并带 provenance | 产品新层级 |

Worktree 就叫 worktree，不要再发明「工作树」当类型名。

### 1.1 和 tmux 怎么对齐

Herdr 的 socket 就是 tmux 的 `-L`：一条 server，上面挂很多可切换的格子。格子里是 tab → pane。

**Herdr 没有 window 这一层。** tmux 的 window 直接等于 Herdr 的 tab，等于 Muxterm 的 Tab。

| 口头 | Herdr 官方 | tmux | Muxterm |
|---|---|---|---|
| socket | named session / `herdr.sock` | `-L` 那个 server | `TargetConnection`（herdr 通道） |
| 能 `ls` 出来的 space | **workspace** `w2` | `tmux ls` 的 session | 池里一格 Workspace |
| tab | tab `w2:t1` | window `@N` | Tab |
| pane | pane `w2:p1` | pane `%N` | Pane |
| workspace / 缩进 / new worktree | **worktree** | 没有原生 | Projects 层；Herdr 走 native 能力 |

**Herdr 的 workspace 就是那个 space，不是 worktree 功能。** `worktree.create` 之后仍是单独一格 space。

---

## 2. Runtime × Transport

删除复合 mode：

```text
不要：LocalShell / LocalTmux / SshShell / SshTmux / RuntimeMode
要：  RuntimeId × TransportId + target data
```

兼容矩阵自动计算，不手写：

```text
compatible(runtime, transport) :=
    runtime.channel_requirements() ⊆ transport.supported_channels()
```

| Runtime | 需要的通道 | 数量 / 时机 |
|---|---|---|
| tmux | `Exec { argv: tmux -L <sock> -CC attach|new, pty: true }` | 1 条，connect 时 |
| shell | `Exec { argv: $SHELL, cwd: path, pty: true }` | **每个 pane 1 条**，NewTab / Split 时按需 |
| herdr | `UnixSocket { path: herdr.sock }`（SSH 侧由 transport 转发） | 1 条，同一 server 的多个实例共享 |

`new_instance` 只构造、不连接；`connect()` 才开通道。Runtime 看不见 local/ssh。

local 支持 `Exec` + `UnixSocket`；ssh 支持 `Exec`（远端命令）+ `UnixSocket`（转发）。
新增 Transport 只要声明 `supported_channels`，所有 Runtime 自动可组合。

具体类型不对外导出。允许：

```text
runtime::tmux::register(registry)
runtime::shell::register(registry)
runtime::herdr::register(registry)
```

禁止 `pub use tmux::TmuxRuntime`。Catalog 只读 registry，不实现 `TmuxRuntime::new()`。

---

## 3. `trait Runtime`

需要 `Box<dyn Runtime>`，因此 async 方法用 `#[async_trait]`。
（Rust 1.75 起 trait 内 `async fn` 脱糖为 `impl Future`，不可 dyn 分发：
[Rust 1.75.0 blog](https://blog.rust-lang.org/2023/12/28/Rust-1.75.0/)，核对 2026-09-08。）

```rust
#[async_trait]
pub trait Runtime: Send {
    fn id(&self) -> RuntimeId;
    fn name(&self) -> &'static str;
    fn support(&self) -> RuntimeCapabilitySet;
    async fn connect(&mut self) -> Result<(), RuntimeError>;
    fn execute(&mut self, task: &Task) -> Result<TaskOutcome, RuntimeError>;
    fn drain_events(&mut self, out: &mut RuntimeBatch);
    async fn shutdown(&mut self) -> Result<(), RuntimeError>;
}

pub struct RuntimeBatch {
    pub control: Vec<ControlEvent>,
    pub render:  Vec<RenderEvent>,
    pub signals: Vec<RuntimeSignal>,
}
```

`RuntimeSignal` 不是 `ActivityRecord`：Runtime 只报告 wire 上看到的事实
（`PaneAgentChanged`、进程名、退出码），不知道 Workspace 名 / Project / Template。

错误类型是 `thiserror` 枚举（`RuntimeError`），不是 `anyhow`。

Runtime 不负责：枚举所有 target、管理其他 Workspace、决定 Project 列表、创建另一个 Workspace、
向 frontend 暴露 concrete type。

`execute` 碰到没有的能力：返回 `TaskOutcome::Rejected`，不要 panic，不要悄悄 no-op。

异步 mutation：有界入队返回 `Accepted { operation_id }`，最终通过 `MutationSettled` 恰好一次
报告 Completed 或 Failed。同步操作仍返回 `Done`。frontend 不得把 Accepted 当完成。

Surface 增量丢失时，产品 Task 是 `RequestPaneSnapshot`。Tmux 产出 `PaneSnapshot`；Herdr 换
generation 等新的 full `PaneFrame`；Shell 无 Runtime 侧网格则 `Rejected`，frontend 保留已有
baseline，不能重放任意 byte suffix。

### 3.1 `RuntimeProvider`

```rust
pub trait RuntimeProvider: Send + Sync {
    fn id(&self) -> RuntimeId;
    fn name(&self) -> &'static str;
    fn support(&self) -> RuntimeCapabilitySet;
    fn channel_requirements(&self) -> &'static [ChannelKind];
    fn discover(&self, conn: &dyn TargetConnection) -> Result<Vec<ExistingCandidate>, RuntimeError>;
    fn new_instance(
        &self,
        conn: Arc<dyn TargetConnection>,
        spec: &WorkspaceSpec,
    ) -> Result<Box<dyn Runtime>, RuntimeError>;
}
```

`discover` 只在有 `Discover` 能力时有意义（tmux list-sessions / herdr workspace.list）。

### 3.2 `RuntimeCapability`

枚举标 `#[non_exhaustive]`，以后只加变体。GUI / Pool / CLI **只根据这个切片**决定画不画入口。

| 变体 | 意思 | Tmux | Shell | Herdr |
|---|---|---|---|---|
| `PersistDetach` | shutdown/关窗后远端还在，能再 attach | 是 | 否 | 是 |
| `Discover` | 连接前能列出可 open 的候选 | 是 | 否 | 是 |
| `MultiTab` | `NewTab` / `SwitchTab` 有意义 | 是 | 是 | 是 |
| `SplitPane` | `SplitPane` 有意义 | 是 | 是 | 是 |
| `SharedClientResize` | 一条控制通道上的 client 尺寸 | 是 | 否 | 视协议 |
| `WorktreeList` | **runtime 原生**列出 checkout | 否 | 否 | 是 |
| `WorktreeCreate` | **runtime 原生**建 checkout 并打开 | 否 | 否 | 是 |
| `WorktreeOpen` | **runtime 原生**打开已有 checkout | 否 | 否 | 是 |
| `WorktreeRemove` | **runtime 原生** `git worktree remove`（不删分支） | 否 | 否 | 是 |

`Worktree*` 收窄为「runtime 原生」。tmux / shell 不报这些能力，**不等于**用户没有 worktree：
Projects service 用 generic 策略（git over TargetConnection）补齐，见 [`WORKSPACE.md`](WORKSPACE.md) §3。
`TmuxRuntime::support()` 仍然不含 `Worktree*`。

禁止：

```text
if spec.runtime == "herdr" { show_worktree_ui() }
```

### 3.3 Runtime 权威 agent 状态

Agent lifecycle 不是 Herdr 专属产品类型，也不新增 `RuntimeCapability`。能提供结构化状态的
Runtime 产出 `RuntimeSignal` / 产品事件 `PaneAgentChanged { pane, agent, initial }`。

`PaneAgentInfo` 保存通用 status、显示名、标题、tokens、cwd、revision 等；不得含 Herdr
public pane id 或 event 名。`initial=true` 只播种现状，不制造 blocked/done 通知；
`agent=None` 释放权威来源，Workspace 恢复 OSC/BEL/正则启发式。Activity 领域消费通用模型。

---

## 4. Transport 三层

| 层 | 职责 |
|---|---|
| **TransportProvider** | `list_targets`、`supported_channels`、`connect(target) → Arc<dyn TargetConnection>`、探活 |
| **TargetConnection** | 可复用。`open_channel(ChannelRequest) → ByteChannel`、`probe` |
| **ByteChannel** | read / write / resize / shutdown / EOF |

`ConnectionRegistry`（`TargetId → Arc<dyn TargetConnection>`）属 transport 领域，由 Muxterm 持有。
不叫 pool，不拥有 Workspace。同一 SSH host / 同一 Herdr socket 一份 `Arc`。

Runtime 不判断 SSH，不直接拼接 `tmux -CC` 或 Herdr socket 路径以外的 wire（路径来自 spec）。

列出 SSH 不是 attach SSH：列出 / 探活用 `ssh -o BatchMode=yes -o ConnectTimeout=2`，
**不要 `-tt`**。`-tt` 会灌 MOTD，把 `list-sessions` 解析成空。

---

## 5. daemon

daemon **不是**独立 Runtime target，也**不**进入 `RuntimeId × TransportId` 矩阵。

它是 **shell runtime 的 daemon 化执行形态**：PTY/进程组放进独立进程，frontend 经 unix socket
连回来。产品语义不变：从池里扔掉 = 会话结束，没有 PersistDetach。

IPC 必须传输三条 lane 的事件流（原始 render 字节 + control + signal），不能再是
`DumpState` JSON 快照 + 本地 diff。wire contract 落在 `runtime/shell/`，CLI 只负责启动宿主。
Core 不得 `use crate::frontend::cli`。

---

## 6. 对照表

### 6.1 tmux（细节 [`LAYER-MAPPING.md`](LAYER-MAPPING.md)）

| tmux | Muxterm |
|---|---|
| 一条 session（按**名字**） | 一个 Workspace + 一个 TmuxRuntime 实例 |
| window `@N` | Tab |
| pane `%N` | Pane；字节 → `PaneOutput` |
| 控制 client detach | `Task::Detach`；session 还在 |

`$N` / `%output` / `send-keys` **只允许出现在** `runtime/tmux`。

### 6.2 Herdr

| Herdr | Muxterm |
|---|---|
| named session / 默认 `herdr.sock` | TargetConnection（UnixSocket 通道） |
| workspace `w2` | 一个 Workspace + 一个 HerdrRuntime 实例 |
| tab `w2:t1` | Tab |
| pane `w2:p1` | Pane |
| `terminal.frame` full/diff | `PaneFrame` / `PaneOutput` |
| `worktree.*` | Projects native 策略；provenance 同步回 Project |
| `pane.agent_status_changed` | `RuntimeSignal` → Activity |

同一 `TargetConnection` 可被多个 HerdrRuntime 共享。不要每个 Workspace 再开一条 socket。

协议 19 的 headless workspace 可能给出零尺寸 layout rect。这是 Herdr wire sentinel：
adapter 必须保留上一份有效 geometry；`PaneInfo` 不得看到 0×0。细节见
[`HERDR-RUNTIME-STABILITY.md`](HERDR-RUNTIME-STABILITY.md)。

**pane 上 tmux 与 Herdr 互斥。** 接 Herdr 不是把身份租出去。tmux 路径上的 OSC/BEL 必须继续自己活。

远程 Herdr：Transport `ssh` + Runtime `herdr`。打开不要 `herdr --remote`（会在远端装/启 server）：
把远端 `herdr.sock` 转发到本机。没在跑就跳过，不要替用户启动。

Herdr workspace id 不再复用项目 `path`。生成五段 `WorkspaceId` 时第五段优先 `workspace_id`。

### 6.3 Shell

自己分配 Tab/Pane id。支持 local/SSH Transport，但没有 Discover、没有 PersistDetach、没有
原生 Worktree。从池里扔掉 = 进程结束。

---

## 7. 字节与 Surface

Live 路径不因 Runtime 而改：

- 进入 `(WorkspaceId, PaneId)` 常驻 Surface 的是 **原始字节**
- 禁止 live `visible_ansi` → `vte.reset`
- Index 消费经过 generation 过滤的 frame/output；Surface 只消费原始 `PaneFrame`/`PaneOutput`
- Herdr `pane.read` 只生成 Index 快照，永不 feed Surface
- gap 后 live 必须先 fenced；只有权威 `PaneSnapshot` / full `PaneFrame` 后才能恢复

输入：tmux `send-keys`（字节通道用 `-H`）；Herdr 原样写字节走 `pane.send_text`，语义按键走
`pane.send_keys`。禁止逐键 `pane.send_input`（会自动加 bracketed-paste）。

Herdr 同一终端同时只有一个 controller 的约束是 **adapter 内部**的 wire 事实，不是 frontend
的「前台校准」或 `set_foreground` 产品 API。frontend 不再维护 foreground/background slot。
旧 generation 的 Frame/Closed/Error 不能改变当前流或像素。

---

## 8. Discovery 与打开

列出发生在 **RuntimeProvider::discover**，不在活 Runtime 实例上。门面是 Catalog，见
[`CATALOG.md`](CATALOG.md)。产品打开入口是 `OpenRequest`，不是 `WorkspaceSpec`。

| Runtime | 连接前能列出什么 | 打开 |
|---|---|---|
| tmux | 本机或 ssh 上的 session **名** | resolver → Pool.open |
| herdr | Connect 上 `workspace.list` | 同一 TargetConnection 上新实例 |
| shell | 无 Discover；目录来自 Project / 文件系统 | create 语义 |

`discover_targets(transport)` = 怎么到那儿。Existing 行 = 该 target 上各 provider 的可 attach 格子。

Project / Recent / Existing 不能把 `TargetConfig` 直接压成裸 `WorkspaceSpec`。
`AttachOnly` 是普通重连；只有用户本次明确新建才允许 `CreateIfMissing`。

同一 target 的异步列表请求带单调 request generation，旧 completion 不能覆盖新 target。

---

## 9. Attach 后生命周期

Create、Attach、Reattach 只是 bootstrap 来源；完成后都必须进入同一个
`Connected/task-capable` 状态。不得依据 bootstrap 来源改变 `NewTab` / split / `WriteRaw` /
`PaneOutput` 语义。

```text
Create | Attach | Reattach
    -> Connected + initial Surface ready
    -> Accepted(operation_id)
    -> authoritative topology/focus/layout + pane baseline
    -> MutationSettled(Completed | Failed)
    -> Connected（或 Detached）
```

contract 测试矩阵由 registry 的 runtime×transport 枚举生成。tmux 用 `-L muxterm-test-*`；
Herdr 用 named session `muxterm-test-*`。

---

## 10. 明确不做

- 产品层 Session、虚拟 Window、把 GUI 窗一对一映射成 tmux window 或 Herdr workspace
- frontend 里 git worktree / herdr CLI 拼命令
- 为 Herdr 单独做侧边栏、agent 列表、插件市场
- 用 Herdr 的 agent 状态当 Muxterm 的身份
- 把 `DaemonRuntime` 注册成第四种用户可选 Runtime
- 复合 `RuntimeMode`；`WorkspaceSpec::build_runtime()` 字符串工厂作为产品路径
- 改 live 像素契约；`visible_ansi` 进 Surface
- 对用户默认 tmux `kill-server`；对 Herdr `herdr server stop`
