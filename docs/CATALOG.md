# CATALOG.md — Catalog 是 backend 总状态

> 日期：2026-08-19（`2026-08-19T01:41:31+08:00`）；C9 connect name / 扁平已有的连接。C7/C8 仍有效。
> 工作目录：`/home/wlz/Developer/self/muxterm`
> 分支：`feature/runtime/support_herdr`
> 产品树：[`WORKSPACE.md`](WORKSPACE.md)。Runtime 契约：[`RUNTIME.md`](RUNTIME.md)。施工：[`CATALOG-PLAN.md`](CATALOG-PLAN.md)。
> 像素：[`SURFACE.md`](SURFACE.md)。已有的连接 UI：[`W20-PLAN.md`](W20-PLAN.md)。

**一句话：** FFI 持有一份 **Catalog**。里面是两张插件表（Driver / Transport）、未打开对象的 **Inventory**、可复用管道 **Connect**、以及已经打开的 **WorkspacePool**。`trait Runtime` 只表示**已经 attach** 的那一格，不负责 `ls`。

本文是契约。QuickConnect / macOS / CLI 都读这一份，不要再在 platform 里长第二套发现或连接池。

---

## 1. 词

| 词 | 是什么 | 不是什么 |
|---|---|---|
| **Catalog** | 进程里一份 backend 总状态。FFI handle 持有它 | GUI Window；产品 Session |
| **Driver** | Runtime **插件**：`TmuxDriver` / `HerdrDriver` / `ShellDriver`。会 `list` / `open` | 已经 attach 的 `trait Runtime` 实例 |
| **Registry** | Driver 表、Transport 表。几乎静态 | 活连接（那是 Pool） |
| **Transport**（Catalog） | **插件**：`Local` / `Ssh`。`list_targets` + `connect()` → `Connect` | 一次 spawn 的字节流（见下） |
| **Connect** | 可复用管道。`Arc<Connect>`，按 `(transport, target)` 共享 | 一个 Workspace；一条 `-CC` |
| **Inventory** | **尚未 attach** 的 target / session 台账（探活、灯） | Pool 里已打开格子的 `BackendStatus` |
| **Pool** | 已打开的 Workspace。槽位表可以继续用现在的 `WorkspacePool` | 插件表；SSH 探活 |
| **target** | 怎么到对端：Local 单例，或 SSH Host alias | 可 attach 的格子 |
| **session**（发现层） | 可 attach 的格子：tmux session 名、Herdr workspace | 产品类型 Session（已删） |

中文：总台账 / 驱动 / 注册表 / 传输插件 / 管道 / 存货 / 池 / 目标 / 候选。

### 1.1 两个叫 Transport 的东西（不要混）

| 层 | 代码 | 职责 |
|---|---|---|
| Catalog 插件 | `core::catalog::transport::Transport` | Local / SSH：**列出 target**，**拿出可复用 Connect** |
| 字节流 | `core::transport::Transport` | 已经存在：一次 `spawn_exec` / read / write / resize。本轮 **不改名**（以后可叫 BytePipe，不在本施工单） |

Driver 打开 Runtime 时，用 `Connect` 再 spawn 字节流（本机 PTY 或 `ssh` 上的 `tmux -CC` / Herdr socket）。**不要**再写 `TmuxRuntime::new_ssh_attach` 把 SSH 烤进 Runtime。

**不要**发明 `TransportDriver`。插件就叫 Transport。

### 1.2 三个都叫 `local` 的东西（不要混）

| 词 | id / 名字 | 怎么列 |
|---|---|---|
| Local **Transport** | `"local"` | `discover_targets("local")` → 单例，target id 是 `""` |
| SSH **Host alias** | 用户 `~/.ssh/config` 的 `Host` 名，**可以就叫 `local`** | `discover_targets("ssh")` 含 `id == "local"`；格子走 `discover_sessions("ssh", "local")` |
| 可连接 Runtime **插件**表 | `"tmux"` / `"herdr"` / `"shell"` | `runtime_list()`，**不是** SSH host 表，也**不是**可 attach 行 |

`discover_sessions("local", "")` 是本机。`discover_sessions("ssh", "local")` 是 SSH Host 名叫 `local` 的那台。两者禁止串。

### 1.3 列出 SSH 不是 attach SSH

`tmux -CC` / 交互 shell 需要远端 pty：`ssh -tt -o ConnectTimeout=10`。那是 **attach**。

列出 session / 探活是短命令：`ssh -o BatchMode=yes -o ConnectTimeout=2`，**不要 `-tt`，不要 SshProcessTransport PTY**。和 W15 `ssh_probe_args`、W20 `ssh_run` 同一条。`-tt` 会灌 MOTD / `\r` / 提示符，`list-sessions` 解析成空，面板就把有 session 的 host 丢掉。

测试用隔离 tmux：`MUXTERM_TEST_LOCAL_TMUX_SOCKET`（本地 Driver.list）和 `MUXTERM_TEST_REMOTE_TMUX_SOCKET`（SSH Driver.list），对标 `HERDR_SOCKET_PATH`。双份测试把两个 env 指到**同一个** `-L muxterm-test-*`。生产不设 = 默认 server。禁止测用户默认 `tmux` / `herdr.sock`。不要求测 `archmini` / `cd`。

### 1.4 connect name（机器）和 runtime list（可 attach 行）

产品里并列的「机器」叫 **connect name**：本机 `local` + 每个 SSH Host alias（`self` / `archmini` / `cd`）。这是 `Connect` 的身份，**不是** `transport_list()` 的插件 id（插件只有 `local` | `ssh`）。

| 调用 | 含义 |
|---|---|
| `discover_sessions("local", "")` | connect name `local` 上的 tmux + herdr |
| `discover_sessions("ssh", "self")` | connect name `self`（SSH Host）上的 tmux + herdr |
| `discover_sessions("all", "")` | 扇出所有 connect name，拼成一张表。写法同搜索 scope 的 `all` |

同一台机器既是 local 又被 SSH 指回来（测试里 LoopbackSshd **Host `self`** → 127.0.0.1）时，**同一 session 出现两行**：`tmux @ local` 和 `tmux @ self`。这是要的，不是去重 bug。

SSH Host 也可以叫 `local`（C7）。connect name 表里本机永远是 `local`；Host `local` 的行用 `ssh:local` 或继续走 `discover_sessions("ssh","local")` 的 target 字段区分，禁止把 Host `local` 当成 Transport `"local"`。

用户说的「runtime list」= `discover_sessions` 的行。`runtime_list()` 仍是新建项目三张卡。

---

## 2. 为什么要这一层

今天的问题：

1. **`list` 挂在活 Runtime 上说不通。** 列出候选发生在实例存在之前。`RuntimeCapability::Discover` 只是旗标：「这种 Driver 能列出」。真正的 `list` 在 Driver 上。
2. **`WorkspaceSpec.build_runtime()` 是字符串 `match`。** 加 Herdr / 以后 Zellij 就要改 core。SSH 路径还走 `TmuxRuntime::new_ssh_attach`。
3. **共享管道只给 Herdr 做了旁路。** `WorkspacePool.herdr_sessions: HashMap<(session, socket), Arc<HerdrSession>>` 是 leak。SSH 每次仍 `ssh … tmux -CC`。同一 Host 上两条 tmux session、两个 Herdr workspace，应该共用一条 `Connect`。
4. **探活在错误的层。** W15 的 SSH 灯写在 `window.rs`（`SshReach`，超时 2s）。GTK 不该 `ssh`。那是 Inventory，不是 Pool 的断线重连。
5. **macOS 不能再为 Herdr 长协议。** 通用 FFI（`runtime_list` / `discover_targets` / `discover_sessions` / `open`）是硬条件。Swift 只渲染 Catalog 给的卡和行。

打开公式（Workspace = Runtime × Transport 是**构造**，不是 Workspace 上两个活字段）：

```
connect   = Transport.connect(target)     // local ≈ 空操作；ssh = 可复用管道
runtime   = Driver.open(connect, spec)    // tmux -CC / herdr socket 走这条管道
workspace = Workspace::new(..., runtime)  // + PaneBuf；放进 Pool
```

同一 `Arc<Connect>` 可以喂多个 Runtime：一条 SSH 上两个 tmux session；一个 named Herdr session 上两个 workspace。

非法组合（以后 `shell × ssh`）在 `open` 时拒绝。Driver 声明 `accepted_transports()`。

---

## 3. 三层寿命

| 层 | 活多久 | 例子 |
|---|---|---|
| Registry | 几乎静态（进程内插件表） | Driver `tmux`，Transport `ssh` |
| Inventory | 未打开的远端/格子，后台一直探 | `ryzen` 通，session `muxterm` 还在 |
| Pool | 已经 attach | 槽里的 `Box<dyn Runtime>` |

```
Catalog                         ← FFI 持有这一份
  registries
    runtimes:  Driver 表        ← 有序数组；with_builtins 按 tmux, herdr, shell 登记
    transports: Transport 表    ← 有序数组；with_builtins 按 local, ssh 登记
  inventory                     ← 未 attach 的 SSH / session 台账（灯）
  connects                      ← HashMap<(transport, target), Arc<Connect>>
  pool: WorkspacePool           ← 已打开
```

`runtime_list()` / `transport_list()` 按数组原样返回，不要再排序。要改卡片顺序就改 `with_builtins` 的登记顺序。

`WorkspaceId` 仍然是产品键：`transport/alias/session/runtime/path`。不要为 Catalog 再发明一套 id。

---

## 4. Driver

`trait RuntimeDriver`（`src/core/catalog/driver.rs`）。**不是** `trait Runtime`。

```text
id() / name() / support() / accepted_transports()
namespaces(connect) -> Vec<String>          # Herdr named session；tmux 可空
list(connect, namespace?) -> Vec<SessionCandidate>
open(connect, spec) -> Box<dyn Runtime>
```

规则：

- **禁止**把 `list` / `namespaces` 加到活 `trait Runtime` 上。
- `support()` 与活 Runtime 的 `support()` 同一组 `RuntimeCapability`。GUI 画 worktree 入口仍然只问能力，禁止 `if spec.runtime == "herdr"`。
- `DaemonRuntime` 是 IPC 客户端，**不**进 Driver 表，不上新建项目卡。
- 生产 Driver **禁止** `Command::new("herdr")`。本地 Herdr 走 socket JSON；SSH 发现可以 `ssh … herdr session list`（和 `ssh … tmux list-sessions` 同类，属 Transport/Discovery，不是 Runtime）。
- 测试不得连 `/home/wlz/.config/herdr/herdr.sock`，不得对默认 tmux `kill-server`。

内置 id（锁死）：`tmux` / `herdr` / `shell`。

`accepted_transports` v1：

| Driver | 接受 |
|---|---|
| tmux | `local`, `ssh` |
| herdr | `local`, `ssh` |
| shell | `local` |

---

## 5. Transport 插件与 Connect

`trait Transport` 在 `src/core/catalog/transport.rs`：

```text
id() / name()
list_targets() -> Vec<TargetInfo>     # Local：一个空 target；SSH：~/.ssh/config Host
connect(target) -> Arc<Connect>       # Catalog 按 (id, target) 缓存，命中不第二次调用
```

`Connect`：

- 身份：`transport_id` + `target`（Local 的 target 为 `""`，SSH 为 alias）。
- Catalog.connects 里同一键 **一份** `Arc`。第二次 `open` 同一 Host 必须 `Arc::ptr_eq`。
- 探活（Inventory）用 Connect **exec 短命令**（`tmux ls` / `herdr session list`），**不要**为此 attach Runtime，**不要**每台机器常驻 `-CC`。
- 今天的 `herdr_sessions` 旁路表迁进 Connects 之后删掉。

---

## 6. Inventory

对象：**还没进 Pool 的** target 和 session。

| 做 | 不做 |
|---|---|
| 后台探 SSH 通不通、tmux/Herdr 还在不在 | 打开 Workspace 才知道断线（那是 Pool `BackendStatus`） |
| UI 只读 snapshot；stale-while-revalidate | GTK 线程里同步 `ssh` |
| 限并发；每 Host 复用 Connect | 为探活 attach Runtime / 开 `-CC` |
| 灯：Unknown / Ok / Err | 和 W15 `window.rs` 里的一次性探测长期共存（要搬走） |

W15 的 `SshReach` + `ConnectTimeout=2` 是原型，**层错了**。本轮把灯的数据源换成 Inventory snapshot；GTK 只绑结果。

---

## 7. 打开与 Pool

`Catalog::open(spec)`：

1. 查 Driver；没有 → **错误**（不要再悄悄变成 Shell）。
2. `accepted_transports` 不含 spec.transport → 错误。
3. `connect = Catalog.connect(transport, alias)`（缓存）。
4. `runtime = Driver.open(connect, spec)`。
5. `pool.open(id, name, runtime)`（复用已有槽位的语义不变）。

`WorkspacePool` **继续**做槽位 / activate / 淘汰 / 后台 `take_events`。它不再知道 Herdr 字符串，也不再持有 `herdr_sessions`。

`WorkspaceSpec.build_runtime()` 是过渡期的字符串工厂。Catalog 路径 **禁止**再调用它。CLI 遗留若仍调用，本轮末尾收掉或标 deprecated。

---

## 8. FFI / 前端（数据驱动）

一个 handle = 整个 Catalog（Pool 在里面）。不要 `if herdr`。

| 调用 | 含义 |
|---|---|
| `runtime_list()` | id + 显示名 + 静态 caps → 新建项目卡 |
| `transport_list()` | Local / SSH |
| `discover_targets(transport)` | SSH hosts；Local 单例 |
| `discover_sessions(transport, target)` | 该 connect 上各 Driver 的可 attach 格子。`transport="all"` 扇出全部 connect name |
| `inventory_snapshot()` | 灯 |
| `open(spec)` | 进 Pool |

**不要**再造 `discover-connection`（和「已有的连接」撞名）。

词：

- **target** = 怎么到那儿
- **session** = 可 attach 的格子（JSON 里仍用产品字段 `workspaces` 也可以，但参数名不要叫 connection）

现有 `muxterm_discover_workspaces_json` 走 `Catalog::discover_sessions` 的扇出（tmux + herdr）。JSON 形状见 [`WORKSPACE.md`](WORKSPACE.md) §6.2：必须带 **`target`（connect name）**。`id` 含 connect name，避免两台机器同名 session 撞车。

macOS：必须吃这套 FFI。Swift **禁止**长 Herdr 协议。卡从 `runtime_list()` 来，worktree 从 `support()` 来。不是「零 UI 改动」：少硬编码，多绑数据。

---

## 9. 和现有文件的关系

| 现在 | 以后 |
|---|---|
| `src/core/discovery.rs` + `discovery/existing.rs` | 被 Driver.list / Transport.list_targets 调用；platform 不直接调 |
| `WorkspaceSpec::build_runtime` | Catalog::open → Driver.open |
| `WorkspacePool.herdr_sessions` | `Catalog.connects` |
| `TmuxRuntime::new_ssh_attach` | Driver.open(ssh Connect, spec) |
| `window.rs` SSH 灯 | Inventory snapshot |
| QuickConnect `TargetRuntime` 枚举 | 可以留作 id 的 view；卡的来源改 `runtime_list()` |

`src/core/discovery/` 本轮不必删。先让 Catalog 当唯一门面。

---

## 10. 文件落点

```
src/core/catalog/
  mod.rs          Catalog
  driver.rs       RuntimeDriver + RuntimeInfo + SessionCandidate
  transport.rs    Transport 插件 + TargetInfo
  connect.rs      Connect
  inventory.rs    Inventory + Reach + snapshot
```

内置插件（实现放 Driver/Transport 旁或 `catalog/builtin/`）：

```
src/core/catalog/builtin/
  tmux.rs         TmuxDriver
  herdr.rs        HerdrDriver
  shell.rs        ShellDriver
  local.rs        Local Transport
  ssh.rs          Ssh Transport
```

测试：`src/core/catalog/` 内 `#[cfg(test)]`。隔离 tmux `-L muxterm-test-*`；隔离 Herdr `muxterm-test-*`。禁止默认 socket。
