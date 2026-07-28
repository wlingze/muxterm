# Muxterm 传输与协议架构

> **文档定位**：本文定义 muxterm 跨平台终端的传输层、运行时层、核心协议、CLI、FFI 的完整架构设计。供后续实现 agent（Codex）按图施工。
>
> **基线**：`/home/wlz/Developer/self/muxterm` main `d69fab2`（2026-07-28）。
> **状态**：设计文档，不含代码实现。所有 Rust/Swift/C 接口为草案，标注「v1 稳定」或「未定」。
>
> 相关文档：
> - `PRODUCT.md` — 产品定位与路线图
> - `ARCHITECTURE.md` — 现有架构与交互模型
> - `docs/ID-SYSTEM.md` — ID 体系（本文 §3 扩展）
> - `docs/LAYER-MAPPING.md` — muxterm↔tmux 层级映射（本文 §5 引用）
> - `docs/ARCHITECTURE-PLAN.md` — C ABI 拆分方案（本文 §9 扩展）

---

## 1. 目标与非目标、术语、四种模式矩阵

### 1.1 目标

1. **抽象传输层与运行时层**，让 local/remote × shell/tmux 四种模式最大程度复用同一套核心逻辑。
2. **保留 muxterm 自有协议层级** Session → Window → Tab → Pane，不把 tmux 的 session/window/pane 概念泄漏到前端或 CLI。
3. **SSH 认证完全委托系统 `ssh`**，muxterm 只读取 `~/.ssh/config` 的 Host 别名，不实现认证/密钥/agent/known_hosts/密码交互。
4. **Discovery 与 Runtime 分离**：发现可用的 host/session/socket 是独立阶段，建立运行时连接是另一阶段。
5. **默认 CLI 输出 JSON**，text 作为可选。
6. **FFI 不长期暴露 Rust 裸指针**：所有跨 ABI 的数据要么是值类型（u32/u16/bool），要么是 borrowed（指针 + len，生命周期由 handle 担保），要么是 owned（调用方提供 buffer，核心 copy 写入）。

### 1.2 非目标

- 不实现自有 SSH 认证协议（不做密钥管理、密码缓存、known_hosts 校验）。
- 不实现自有终端模拟器（终端渲染由平台层 vte4/SwiftTerm/等负责）。
- 不在 v1 支持多 session 同时活跃（一个 Runtime 实例 = 一个 session 的一个 window）。
- 不在 v1 支持非 Unix 的本地 PTY（Windows ConPTY 留到后续阶段）。
- 不在本文定义 UI 渲染细节（见 `ARCHITECTURE.md` §4 与 `docs/RENDERING-OPTIMIZATION.md`）。

### 1.3 术语

| 术语 | 定义 |
|------|------|
| **Transport** | 字节流通道：在本地或远程执行命令、读写 stdin/stdout、传递 PTY resize/信号。与协议无关。 |
| **Runtime** | 建立在 Transport 之上的语义层：理解「终端」概念（pane、shell 进程生命周期）或「tmux 控制模式」（%消息解析、命令发送）。 |
| **Backend** | `core::model::Backend` trait 的实现，由 Runtime + Transport 组合而成，对 TerminalModel 暴露统一的 `execute(Task)` / `take_events()` 接口。 |
| **Core Protocol** | muxterm 自有的 Session/Window/Tab/Pane 层级及 ID 体系，所有前端和 CLI 只使用这套 ID。 |
| **Tmux Compatibility Adapter** | TmuxControlRuntime 内部的翻译层：tmux 的 `$N`/`@N`/`%N` 真实 ID 只在此层存在，对外映射为 muxterm 的 `s{name}`/`wN`/`tN`/`pN`。 |
| **Discovery** | 无状态的查询阶段：列出 SSH host 别名、本地/远程 tmux session、文件目录等，供用户或前端选择。不建立长连接。 |
| **Snapshot** | 某时刻 Runtime 状态的完整只读快照（sessions/windows/tabs/panes/layouts/outputs/status）。 |

### 1.4 四种模式矩阵

运行位置（local/ssh）与运行模式（shell/tmux）是两个正交维度：

| | **Shell 模式** | **Tmux 控制模式** |
|---|---|---|
| **Local** | `local-shell` | `local-tmux` |
| **SSH Remote** | `ssh-shell` | `ssh-tmux` |

| 模式 | Transport | Runtime | 语义 |
|------|-----------|---------|------|
| **local-shell** | `LocalProcessTransport` | `ShellRuntime` | 本地 spawn shell 进程到 PTY，muxterm 自行管理 pane 分割 |
| **local-tmux** | `LocalProcessTransport` | `TmuxControlRuntime` | 本地 spawn `tmux -CC`，解析 %消息，pane 由 tmux 管理 |
| **ssh-shell** | `SshProcessTransport` | `ShellRuntime` | 远程经 SSH 执行 shell，pty 在远端 |
| **ssh-tmux** | `SshProcessTransport` | `TmuxControlRuntime` | 远程经 SSH 执行 `tmux -CC`，解析 %消息 |

**复用关系：**
- `ShellRuntime` 不关心 Transport 是 local 还是 SSH，只调用 `Transport::spawn_exec()` / `read()` / `write()` / `resize()` / `kill()`。
- `TmuxControlRuntime` 同理，只调用 Transport 的字节流接口 + 行解析。
- 四种模式 = 2 个 Transport 实现 × 2 个 Runtime 实现，共 4 个组合，每个组合对应一个 `Backend` 实例。

**现有代码对照：**
- `LocalBackend` ≈ `local-shell`（`LocalProcessTransport` + `ShellRuntime`，目前耦合在一起）。
- `TmuxBackend` ≈ `local-tmux`（`LocalProcessTransport` + `TmuxControlRuntime`，目前耦合在一起）。
- `DaemonBackend` 是 CLI daemon client，不是上述四种之一（它是 `local-shell`/`local-tmux` 的 daemon IPC 变体）。
- `RemoteTmuxClient` ≈ `SshProcessTransport` + `TmuxControlRuntime` 的一部分（已实现 SSH exec + 行解析，但未抽象为 Transport/Runtime，也未实现 `Backend` trait）。

---

## 2. 目标与非目标（细化）

见 §1.1/§1.2。补充约束：

- **协议 ID 稳定性**：Core Protocol 的 ID 规则（§3）一旦发布即为 v1 稳定，不随实现重构变更。
- **FFI ABI 稳定性**：`#[repr(C)]` 结构体布局在 v1 内不变；新增字段只能加在末尾并保证向后兼容（或用版本号协商，见 §9.7）。
- **CLI 语法稳定性**：已定义的命令和参数在 v1 内不 breaking；新增命令/参数允许。

---

## 3. Core Protocol — Session/Window/Tab/Pane 层级、ID 规则、能力差异

### 3.1 层级模型

```
Session → Window → Tab → Pane
```

| 层级 | 说明 | 数量关系 | 可前端新建 |
|------|------|----------|-----------|
| Session | 一个终端会话（可后台 / 可 attach） | 每个 Runtime 实例 1 个 active | 是（new-session） |
| Window | 前端一级对象，类似浏览器窗口 | **固定 1 个**，绑定 Session（v1） | 否（v1 固定 1 个） |
| Tab | 窗口内的标签页 | 多个，用户可新建/关闭 | 是（new-tab） |
| Pane | Tab 内的分割区域 | 多个，可分割 | 是（split-pane） |

> **v1 约束**：一个 Runtime 实例 = 一个 Session 的一个 Window。多 Window 支持留到后续版本，但协议层已定义 Window 为一级对象，避免未来 breaking。

### 3.2 ID 规则（v1 稳定）

muxterm 对外只使用自有 ID，不暴露 tmux 的 `$N`/`@N`/`%N`。

| 层级 | ID 格式 | 示例 | 说明 |
|------|---------|------|------|
| Session | `s{name}` | `sdev`、`smain` | 按名字引用，name 为非空字符串（`[A-Za-z0-9_.-]+`） |
| Window | `w{n}` | `w1` | 数字编号，从 1 开始；v1 固定为 `w1` |
| Tab | `t{n}` | `t1`、`t2` | 数字编号，从 1 开始，按创建顺序 |
| Pane | `p{n}` | `p1`、`p3` | 数字编号，从 1 开始，按创建顺序 |

**组合路径（层级引用）：**
```
s{name}                 → 指定 session
s{name}/w1              → session 内的 window 1
s{name}/w1/t2           → session 内 window 1 的 tab 2
s{name}/w1/t2/p3        → session 内 window 1 tab 2 的 pane 3
s{name}/t2/p1           → 省略 window（默认 w1）
s{name}/p2              → 省略 window 和 tab（默认 w1 + active tab）
```

**CLI 简写：**
- `-s <name>` → session
- `-w <n>` → window（默认 1）
- `-t <n>` → tab（默认 active tab）
- `-p <n>` → pane（默认 active pane）

**ID 分配规则：**
- Tab/Pane 编号在所属父级内单调递增，不复用已关闭的编号（避免歧义）。
- Session name 用户指定；重名在 v1 不允许（new-session 失败）。
- 编号从 1 开始（非 0），与 `ID-SYSTEM.md` 一致。

### 3.3 tmux 真实 ID 隔离

tmux 的 ID（`$N` session、`@N` window、`%N` pane）**只能存在于 TmuxControlRuntime 内部**：
- `TmuxControlRuntime` 维护映射表 `muxterm_id ↔ tmux_id`。
- 前端、CLI、FFI、StateChange 事件只携带 muxterm ID。
- 映射表不序列化到 Snapshot 的协议字段（实现内部细节）。

> 与 `docs/LAYER-MAPPING.md` 一致：tmux window → muxterm Tab；muxterm Window 虚拟固定 1 个。

### 3.4 能力差异矩阵

不同模式支持的操作集合不同。Backend 在 `execute(Task)` 时对不支持的 Task 返回 `TaskOutcome::Rejected { reason }`。

| 操作 | local-shell | local-tmux | ssh-shell | ssh-tmux | 说明 |
|------|:-----------:|:---------:|:--------:|:--------:|------|
| new-session | ✅ | ✅ | ✅ | ✅ | 所有模式都支持创建新 session |
| attach-session | ❌（无 server） | ✅ | ❌ | ✅ | shell 模式无 attach 语义 |
| detach | ❌ | ✅ | ❌ | ✅ | shell 模式无 detach |
| list-sessions（跨 session） | ❌ | ✅ | ❌ | ✅ | 需 tmux server 查询 |
| new-window | ✅（虚拟） | ✅（虚拟） | ✅ | ✅ | v1 固定 1 个，实际为 no-op / 预留 |
| new-tab | ✅ | ✅ | ✅ | ✅ | shell=新 pane+新 tab；tmux=new-window |
| split-pane | ✅ | ✅ | ✅ | ✅ | shell=嵌套分割+spawn；tmux=split-window |
| resize-pane | ✅ | ✅ | ✅ | ✅ | shell=pty resize；tmux=resize-pane |
| send-keys / write-raw | ✅ | ✅ | ✅ | ✅ | shell=pty write；tmux=send-keys |
| capture-pane | ✅ | ✅ | ✅ | ✅ | shell=本地 scrollback；tmux=capture-pane |
| kill-pane / kill-tab | ✅ | ✅ | ✅ | ✅ | |
| kill-session | ✅（关 daemon） | ✅ | ✅ | ✅ | |
| rename-session/tab | ✅ | ✅ | ✅ | ✅ | |
| display-message | ❌ | ✅ | ❌ | ✅ | tmux format 查询，shell 模式无 |

### 3.5 StateChange 事件（v1 稳定）

Runtime 通过 `take_events()` 产出 `StateChange` 事件（定义在 `core::model::state`，现有 17 变体保留）：

```
PaneOutput{pane, data: Vec<u8>}     // 增量输出
WindowAdded / WindowClosed / WindowRenamed
TabAdded / TabClosed / TabRenamed / ActiveTabChanged
PaneAdded / PaneClosed / PaneTitleChanged / PaneResized / ActivePaneChanged
ActiveWindowChanged / SessionChanged / SessionRenamed / SessionsChanged
LayoutChanged{tab, layout: TabLayout}  // 完整布局树快照
BackendStatusChanged(BackendStatus)
```

> 所有事件中的 ID 均为 muxterm ID（`p1`/`t2` 等），非 tmux ID。

---

## 4. Transport 层设计

Transport 是纯粹的字节流通道，不理解任何终端语义。两个实现：

### 4.1 Transport trait（草案）

```rust
/// 传输层抽象：在本地或远程执行一个长驻命令，提供双向字节流。
///
/// 一个 Transport 实例 = 一次进程生命周期（spawn → read/write → exit）。
/// 不理解 pane/session/tmux，只管字节流 + PTY 控制。
pub trait Transport: Send {
    /// 在远端（或本地）以 PTY 模式启动一个长驻命令。
    /// `program` 在 local 为 shell 路径，在 tmux 为 "tmux"，在 ssh 为经 SSH 执行的命令。
    /// `pty_size` 初始字符格尺寸。
    fn spawn_exec(
        &mut self,
        program: &str,
        args: &[&str],
        pty_size: PtySize,
    ) -> anyhow::Result<()>;

    /// 非阻塞读取 stdout/pty master 的下一块字节。None 表示 EOF / 进程退出。
    fn read(&mut self) -> std::io::Result<Option<Vec<u8>>>;

    /// 写入 stdin/pty master。返回写入字节数。
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize>;

    /// 调整 PTY 字符格尺寸（SIGWINCH / pty resize）。
    fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()>;

    /// 发送信号给子进程（SIGTERM / SIGHUP / SIGKILL）。
    fn kill(&mut self, signal: TransportSignal) -> anyhow::Result<()>;

    /// 非阻塞探测是否已退出。Some(code) 表示已退出；None 表示仍运行。
    fn try_wait(&mut self) -> std::io::Result<Option<u32>>;

    /// 优雅关闭：关闭写端，等待退出，回收资源。
    fn shutdown(&mut self) -> anyhow::Result<()>;

    /// stderr 累积（调试用；可选）。
    fn stderr(&self) -> &[u8];
}

pub enum TransportSignal {
    Hangup,   // SIGHUP
    Term,     // SIGTERM
    Kill,     // SIGKILL
}
```

> **注意**：现有代码用 tokio async + mpsc channel（`tmux/pty.rs`、`ssh/client.rs`）。Transport trait 为同步接口（内部可 spawn 后台线程做 async→sync 桥接），与现有 `Backend::execute` 同步签名一致。具体实现可用 `portable-pty`（local）或 `async-ssh2-tokio`（ssh，内部 tokio runtime）。

### 4.2 LocalProcessTransport

**职责**：在本地用 `portable-pty` 分配 PTY 对，spawn 子进程，提供读写/resize/kill。

```rust
pub struct LocalProcessTransport {
    master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    reader: mpsc::Receiver<Vec<u8>>,  // 后台读线程喂入
    stderr_buf: Vec<u8>,
    pid: u32,
}
```

**实现要点**（复用现有 `core::tmux::pty` 和 `core::backend::local` 的模式）：
- `spawn_exec`：`NativePtySystem::openpty(PtySize)` + `CommandBuilder::new(program)` + `spawn_command`。
- `read`：从 `mpsc::Receiver` 非阻塞 `try_recv`；后台读线程 `spawn_blocking` 循环 `read`。
- `write`：`master.take_writer()` + `write_all`（`spawn_blocking` 避免阻塞事件循环）。
- `resize`：`master.resize(PtySize)`。
- `kill`：`child.kill()` 或 `libc::kill(pid, sig)`。
- `try_wait`：`child.try_wait()` 或 `libc::kill(pid, 0)`。
- `shutdown`：close writer，wait child，drop master。

**现有代码复用**：`core::terminal::process.rs` 的 `spawn_program` / `kill` / `get_process_name` 可直接复用或提取为 Transport 实现。

### 4.3 SshProcessTransport

**职责**：经 SSH 连接远程主机，在远端以 PTY 模式执行命令，提供读写/resize/kill。

```rust
pub struct SshProcessTransport {
    session: SshSession,          // 复用现有 core::ssh::client::SshSession
    exec_stream: Option<CommandStream>,  // async-ssh2-tokio execute_io
    stdin_tx: mpsc::Sender<Vec<u8>>,
    stdout_rx: mpsc::Receiver<Vec<u8>>,
    stderr_buf: Vec<u8>,
    join: Option<JoinHandle<Result<u32>>>,
    rt: tokio::runtime::Runtime,  // 内部 async runtime
}
```

**实现要点**（复用现有 `core::ssh::client`）：
- `spawn_exec`：`SshSession::connect(config)` + `client.execute_io(cmd, stdout_tx, stderr_tx, stdin_rx, request_pty=true)`。
- `read`：`stdout_rx.try_recv()`。
- `write`：`stdin_tx.try_send(data)`。
- `resize`：SSH 协议的 `window-change` channel request（async-ssh2-tokio 支持 pty resize）。
- `kill`：关闭 stdin（发 EOF）+ close channel；或发 `SSH_MSG_CHANNEL_CLOSE`。
- `try_wait`：检查 `join` 是否完成。
- `shutdown`：close stdin，await join，disconnect session。

**SSH 认证策略（关键约束）**：
- `SshProcessTransport` **不自己处理认证**。
- 构造 `SshConfig` 时只填 `host = <alias>`（从 `~/.ssh/config` 读取的 Host 别名）。
- 实际连接时调用系统 `ssh <alias> <command>`（通过 `std::process::Command` spawn ssh 客户端进程，而非用 async-ssh2-tokio 的库级连接）。
- **修正**：现有 `core::ssh::client` 用 `async-ssh2-tokio` 做库级 SSH 连接（自己处理认证）。按本设计要求，`SshProcessTransport` 应改为 **spawn 系统 `ssh` 进程**，认证/密钥/agent/ProxyJump/known_hosts 全部由系统 ssh 处理。

**SshProcessTransport 的真正实现方式（v1）**：
```rust
pub struct SshProcessTransport {
    // 用 portable-pty 或 std::process spawn 系统 ssh 进程
    master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    reader: mpsc::Receiver<Vec<u8>>,
    stderr_buf: Vec<u8>,
    pid: u32,
}
```
- `spawn_exec`：`CommandBuilder::new("ssh")` + args `["<alias>", "<remote-command>"]`，分配 PTY（ssh 需要 tty 才能做 PTY 转发）。
- 这与 `LocalProcessTransport` 结构几乎相同（都是 spawn 进程到 PTY），区别仅在 program/args 不同。
- **优点**：复用同一套 PTY 管理；认证完全委托系统 ssh；支持 ProxyJump / Jump host / 所有 ssh config 特性。

> **与现有代码的差异**：现有 `RemoteTmuxClient` 用 `async-ssh2-tokio` 库级连接。本设计建议 v1 改为 spawn 系统 `ssh` 进程，以完全委托认证。`async-ssh2-tokio` 的 `RemoteTmuxClient` 可保留作为 Phase 2 的「无系统 ssh 依赖」选项，但 v1 不用。

### 4.4 字节流、PTY、resize、生命周期

| 关注点 | LocalProcessTransport | SshProcessTransport |
|--------|----------------------|---------------------|
| 字节流 | pty master read/write | ssh 进程的 pty master read/write |
| PTY | portable-pty 本地 pty | ssh 进程在本地也有 pty（ssh 自己做远端 pty 转发） |
| resize | `master.resize()` → 子进程 SIGWINCH | `master.resize()` → ssh 进程转发 window-change 到远端 |
| 退出码 | `child.try_wait()` → exit code | ssh 进程退出码（远端命令退出码通过 ssh 传递） |
| stderr | pty 合并（ssh -t）或分离 | ssh stderr 用于错误诊断 |

### 4.5 stderr / 退出码 / 超时 / 背压

**stderr**：
- Transport 累积 stderr 到内部 `Vec<u8>`（有界，上限 64KB），供错误诊断。
- Runtime 不消费 stderr（只用于 spawn 失败时的错误信息）。

**退出码**：
- `try_wait` 返回 `Option<u32>`：`Some(code)` = 已退出，`None` = 运行中。
- Runtime 在 `take_events` 时检测退出，产出 `BackendStatusChanged(Exited)` 或 `BackendStatusChanged(Error)`。

**超时**：
- `spawn_exec` 有连接超时（SSH 连接 / tmux 启动），默认 10s。
- `read` 无超时（非阻塞）；Runtime 轮询频率由前端决定（FFI 16ms / CLI 按需）。
- CLI 命令模式有整体超时（默认 30s），避免 daemon 无响应时永久挂起。

**背压**：
- `read` 的 `mpsc::Receiver` 有界（容量 256），满了后台读线程阻塞（自然背压）。
- pane 输出缓冲有界（`MAX_PANE_OUTPUT_BYTES = 2MB`，见 `core/buffer_cap.rs`），超限丢弃最旧前缀。
- 事件队列有界（`MAX_STATE_EVENTS = 8192`），超限丢弃最旧的 `PaneOutput`。
- `write` 无背压（pty write 通常很快）；但 Runtime 应限制单次 write 大小（建议 ≤ 64KB）。

### 4.6 Transport 总结

```
┌─────────────────────────────────────────────┐
│              Transport trait                 │
│  spawn_exec / read / write / resize / kill   │
│  try_wait / shutdown / stderr                │
├──────────────────┬──────────────────────────┤
│ LocalProcessTrans │  SshProcessTransport      │
│ (portable-pty)    │  (spawn 系统 ssh 进程)    │
│ local-shell       │  ssh-shell                │
│ local-tmux        │  ssh-tmux                 │
└──────────────────┴──────────────────────────┘
```

---

## 5. Runtime 层设计

Runtime 建立在 Transport 之上，理解终端语义。两个实现：

### 5.1 ShellRuntime

**职责**：管理一个 shell 进程（或多个 pane 的 shell 进程），自行维护 Session/Window/Tab/Pane 层级与嵌套分割。

```rust
pub struct ShellRuntime {
    transport: Box<dyn Transport>,
    // 自维护状态（复用现有 LocalBackend 的逻辑）
    sessions: Vec<SessionInfo>,
    windows: Vec<WindowInfo>,
    tabs: Vec<LocalTab>,
    panes: Vec<LocalPane>,     // 每个 pane 有自己的 PTY（multi-transport？见下）
    layouts: HashMap<TabId, TabLayout>,
    outputs: HashMap<PaneId, Vec<u8>>,
    events: VecDeque<StateChange>,
    next_tab: u32,
    next_pane: u32,
}
```

**多 pane 问题**：Shell 模式下每个 pane 是独立的 shell 进程（独立 PTY），但一个 `Transport` 实例只管一个进程。

**解决方案**：ShellRuntime 内部持有 **多个 Transport 实例**（一个 pane 一个）：
```rust
pub struct ShellRuntime {
    panes: HashMap<PaneId, PaneTransport>,  // 每个 pane 一个 Transport
    // ... 状态字段
}
struct PaneTransport {
    transport: Box<dyn Transport>,
    output: Vec<u8>,
    pid: u32,
}
```

- `connect()`：创建第一个 window/tab/pane，spawn 默认 shell。
- `split-pane`：在当前 pane 旁 spawn 新 Transport（新 shell 进程）。
- `close-pane`：`transport.kill()` + 移除。
- `resize-pane`：`transport.resize()`。
- `send-keys`：`transport.write()`。
- `take_events`：遍历所有 pane 的 `transport.read()`，聚合成 `PaneOutput` 事件。

> 这本质上是把现有 `LocalBackend` 的逻辑重构为 ShellRuntime + Transport。现有 `LocalBackend` 已经用 portable-pty 管理多 pane 的多对 pty，这里只是抽象接口。

### 5.2 TmuxControlRuntime

**职责**：spawn 一个 `tmux -CC` 进程，解析 %消息，发送 tmux 命令，把 tmux 的 3 层模型映射为 muxterm 的 4 层。

```rust
pub struct TmuxControlRuntime {
    transport: Box<dyn Transport>,
    // 复用现有 TmuxBackend 的逻辑
    cmd_tx: mpsc::Sender<String>,          // 命令发送 channel
    sessions: Vec<SessionInfo>,
    tabs: Vec<TabInfo>,                    // tmux window → muxterm Tab
    panes: Vec<PaneInfo>,                   // tmux pane → muxterm Pane
    layouts: HashMap<TabId, TabLayout>,
    outputs: HashMap<PaneId, Vec<u8>>,
    events: VecDeque<StateChange>,
    // ID 映射表（tmux → muxterm）
    tmux_session: Option<String>,          // tmux session name
    tab_map: HashMap<TabId, TmuxWindowId>, // muxterm tN → tmux @N
    pane_map: HashMap<PaneId, TmuxPaneId>, // muxterm pN → tmux %N
    // ... 响应累积
    response_accum: HashMap<i64, Vec<String>>,
    pending_queries: VecDeque<PendingQuery>,
}
```

**单 Transport**：Tmux 模式只有**一个** `tmux -CC` 进程，所有 pane 的输出都通过 `%output %N "..."` 消息经同一个 Transport 到达。Runtime 内部按 pane id 分发。

**ID 映射（核心隔离）**：
- `tmux_session name` → muxterm `SessionInfo.name`（用户可见名，非 tmux ID）。
- `tmux @N (window)` → muxterm `t{index}`（TabId，index 从 1 开始按 tmux window index 映射）。
- `tmux %N (pane)` → muxterm `p{counter}`（PaneId，counter 在 Runtime 内单调递增）。
- 映射表 `tab_map` / `pane_map` 在 Runtime 内部，**不出现在 StateChange 事件或 Snapshot 协议字段中**。

**控制协议解析**（复用现有 `core::tmux::protocol`）：
- Transport `read()` 返回字节流 → 按真换行切行 → `parse_line()` → `Message` enum。
- `%output %N "..."` → 解码 C 转义 → 查 `pane_map` 得 muxterm PaneId → `PaneOutput` 事件。
- `%layout-change` / `%window-add` / `%window-close` / `%pane-mode-changed` 等 → 更新内部 state + 映射表 + 产出事件。
- `%begin`/`%end`/`%error` 之间为命令响应行，按 `pending_queries` 处理。

**命令映射**（复用现有 `core::tmux::command`）：
- `Task::SplitPane` → `tmux split-window -h/-v -t %N`（%N 从 `pane_map` 查得）。
- `Task::NewTab` → `tmux new-window -n <name>`。
- `Task::ClosePane` → `tmux kill-pane -t %N`。
- `Task::SwitchTab` → `tmux select-window -t @N`。
- `Task::SendKeys` → `tmux send-keys -t %N <keys>`。
- `Task::ResizePane` → `tmux resize-pane -t %N -x W -y H`。

> 命令发送经 `Transport::write()`（写命令字符串 + `\n` 到 tmux stdin）。现有 `TmuxClientHandle` 用 tokio mpsc + pty writer，这里改为 Transport::write。

### 5.3 Backend 组合关系

```rust
// Backend trait（现有，不变）
pub trait Backend: State {
    async fn connect(&mut self) -> Result<()>;
    fn execute(&mut self, task: &Task) -> Result<TaskOutcome>;
    fn take_events(&mut self) -> Vec<StateChange>;
    async fn shutdown(&mut self) -> Result<()>;
}

// 四种模式的 Backend 构造
impl Backend for ShellRuntime { ... }      // local-shell / ssh-shell
impl Backend for TmuxControlRuntime { ... } // local-tmux / ssh-tmux

// 构造工厂
pub fn create_backend(mode: RuntimeMode) -> Box<dyn Backend> {
    match mode {
        RuntimeMode::LocalShell => {
            let transport = LocalProcessTransport::new();
            Box::new(ShellRuntime::new(Box::new(transport)))
        }
        RuntimeMode::LocalTmux { socket, session } => {
            let transport = LocalProcessTransport::new();
            Box::new(TmuxControlRuntime::new(
                Box::new(transport), socket, session,
            ))
        }
        RuntimeMode::SshShell { alias } => {
            let transport = SshProcessTransport::new(alias);
            Box::new(ShellRuntime::new(Box::new(transport)))
        }
        RuntimeMode::SshTmux { alias, session } => {
            let transport = SshProcessTransport::new(alias);
            Box::new(TmuxControlRuntime::new(
                Box::new(transport), None, session,
            ))
        }
    }
}

pub enum RuntimeMode {
    LocalShell,
    LocalTmux { socket: Option<String>, session: Option<String> },
    SshShell { alias: String },
    SshTmux { alias: String, session: Option<String> },
}
```

### 5.4 Tmux Compatibility Adapter

`TmuxControlRuntime` 内部的 tmux 兼容层：

```
┌──────────────────────────────────────────┐
│         TmuxControlRuntime                │
│  ┌──────────────────────────────────────┐│
│  │   Tmux Compatibility Adapter          ││
│  │  ┌─────────────┐  ┌────────────────┐  ││
│  │  │ ID Mapper   │  │ Protocol Parser│  ││
│  │  │ muxterm↔tmux│  │ %消息→Message  │  ││
│  │  └─────────────┘  └────────────────┘  ││
│  │  ┌─────────────────────────────────┐ ││
│  │  │ Command Builder                  │ ││
│  │  │ Task→TmuxCommand(用 tmux ID)     │ ││
│  │  └─────────────────────────────────┘ ││
│  └──────────────────────────────────────┘│
│              ↓↑ Transport                 │
│           (字节流)                         │
└──────────────────────────────────────────┘
```

**ID Mapper 职责**：
- `muxterm_pane_to_tmux(p: PaneId) -> Option<TmuxPaneId>`
- `tmux_pane_to_muxterm(tp: TmuxPaneId) -> Option<PaneId>`（自动分配新 muxterm ID）
- `muxterm_tab_to_tmux(t: TabId) -> Option<TmuxWindowId>`
- `tmux_window_to_muxterm(tw: TmuxWindowId) -> Option<TabId>`
- tmux session name 在 attach 时记录，映射为 muxterm session name。

**Protocol Parser**：完全复用现有 `core::tmux::protocol::parse_line()`（130+ 测试）。

**Command Builder**：完全复用现有 `core::tmux::command::TmuxCommand` 构造器。

> 现有 `TmuxBackend` 已经实现了上述大部分逻辑（ID 映射、协议解析、命令构造），重构为 `TmuxControlRuntime` 主要是抽出 Transport 接口、增加 ID 映射表的显式管理、把 tmux ID 从 StateChange 事件中清除。

---

## 6. Discovery 层

Discovery 是无状态的查询阶段，不建立长连接。分三类查询：

### 6.1 SSH Host 查询

**约束**：只读取 `~/.ssh/config` 的 `Host` 名称（别名），不做 DNS 解析、不连接、不认证。

```rust
/// 发现可用的 SSH host 别名。
pub trait SshHostDiscovery {
    /// 列出 ~/.ssh/config 中的 Host 条目（别名 + HostName + Port + User）。
    fn list_hosts(&self) -> Result<Vec<SshHostEntry>>;
}

pub struct SshHostEntry {
    pub alias: String,       // Host 别名（如 "myserver"）
    pub hostname: String,    // HostName（如 "server.example.com"）
    pub port: u16,           // Port（默认 22）
    pub user: String,        // User
    // 不含 IdentityFile / ProxyJump 等敏感配置（认证交给系统 ssh）
}
```

**实现策略**：
- 解析 `~/.ssh/config`（POSIX）或 `%USERPROFILE%\.ssh\config`（Windows）。
- 只提取 `Host`、`HostName`、`Port`、`User` 字段；`IdentityFile`、`ProxyJump`、`AddKeysToAgent` 等由系统 ssh 处理，不解析。
- wildcard Host（`Host *`）不列出。
- 支持多个 `Host` 行别名（`Host myserver backup`）→ 每个别名一个 entry。

**CLI**：`muxterm ssh hosts` → JSON `[{"alias":"myserver","hostname":"...","port":22,"user":"alice"}]`

### 6.2 目录查询（fs list）

**用途**：前端文件浏览器 / `fs list` 命令 / QuickPick 文件选择。

```rust
pub trait FsDiscovery {
    /// 列出目录内容。
    fn list_dir(&self, path: &str) -> Result<Vec<FsEntry>>;
    /// 列出 home 目录路径。
    fn home_dir(&self) -> Result<String>;
}

pub struct FsEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: u64,  // unix timestamp
}
```

**实现策略**：
- 本地：`std::fs::read_dir`（纯 std）。
- 远程：经 `SshProcessTransport` spawn `ls -la --time-style=+%s <path>`，解析输出（或用 sftp，但 v1 倾向用简单 exec 以减少依赖）。

**CLI**：`muxterm fs list <path>` → JSON

### 6.3 tmux session 查询

**用途**：前端列出可 attach 的 tmux session（命令面板 / `tmux session list` 命令）。

```rust
pub trait TmuxSessionDiscovery {
    /// 列出本地 tmux server 的 session。
    fn list_local_sessions(&self, socket: Option<&str>) -> Result<Vec<TmuxSessionInfo>>;
    /// 列出远程 tmux server 的 session（经 SSH exec tmux list-sessions）。
    fn list_remote_sessions(&self, ssh_alias: &str) -> Result<Vec<TmuxSessionInfo>>;
}

pub struct TmuxSessionInfo {
    pub name: String,
    pub windows: u32,
    pub attached: bool,
    pub created: u64,
}
```

**实现策略**：
- 本地：`tmux -L <socket> list-sessions -F '#{session_name}\t#{session_windows}\t#{session_attached}\t#{session_created}'`，解析 TSV 输出。
- 远程：`ssh <alias> tmux list-sessions -F '...'`（同样 TSV）。
- **不建立 tmux -CC 控制连接**，只是 list-sessions（一次性 exec）。

**CLI**：
- `muxterm tmux session list [-L socket]` — 本地
- `muxterm tmux session list --remote <alias>` — 远程

### 6.4 Discovery 与 Runtime 分离

```
Discovery（无状态查询）         Runtime（有状态长连接）
─────────────────────          ─────────────────────
ssh hosts                      ShellRuntime
fs list                        TmuxControlRuntime
tmux session list              ↓
  ↓                            Transport (LocalProcess/SshProcess)
用户选择 target                 ↓
  ↓                            Backend → TerminalModel → 前端
Runtime::open(spec)
```

- Discovery 不持有 Transport，不建立长连接。
- Discovery 结果供前端展示 / 用户选择 / CLI 输出。
- 选定 target 后才构造 Runtime + Transport（`RuntimeMode` enum）。

---

## 7. Runtime Command/Event Protocol

Runtime 层内部（Backend ↔ TerminalModel ↔ 前端 / CLI）的命令与事件协议。

### 7.1 命令（Task，v1 稳定）

现有 `core::model::task::Task` enum（18 变体）保留，补充未覆盖的：

```rust
pub enum Task {
    // 布局
    SplitPane { target: Option<PaneId>, dir: SplitDir, command: Option<Vec<String>>, workdir: Option<String> },
    ClosePane { target: PaneId },
    ResizePane { target: PaneId, cols: u16, rows: u16 },
    ResizePaneStep { target: PaneId, dir: SplitDir, delta: i32 },

    // 焦点
    SwitchPane { target: PaneId },
    NextPane,
    PrevPane,

    // Window（v1 固定 1 个，但保留命令）
    NewWindow { name: Option<String>, command: Option<Vec<String>>, workdir: Option<String> },
    CloseWindow { target: WindowId },
    SwitchWindow { target: WindowId },
    RenameWindow { target: WindowId, name: String },

    // Tab
    NewTab { window: WindowId, name: Option<String>, command: Option<Vec<String>>, workdir: Option<String> },
    CloseTab { target: TabId },
    SwitchTab { target: TabId },
    RenameTab { target: TabId, name: String },

    // Session
    SwitchSession { target: SessionId },
    RenameSession { target: SessionId, name: String },

    // 输入
    SendKeys { target: PaneId, keys: Vec<KeyEvent> },
    WriteRaw { target: PaneId, data: Vec<u8> },

    // 生命周期
    Shutdown,
}
```

> 所有 ID 为 muxterm ID。`target: None` 表示用 active pane/tab。

### 7.2 事件（StateChange，v1 稳定）

见 §3.5。现有 17 变体保留。

### 7.3 Snapshot

完整状态快照（用于 `dump-state` / DaemonBackend 同步 / FFI 拉取）：

```json
{
  "sessions": [{"id": 1, "name": "dev", "active_window": 1}],
  "windows": [{"id": 1, "name": "main", "session": 1, "active": true}],
  "tabs": [
    {"id": 1, "name": "zsh", "window": 1, "active": true},
    {"id": 2, "name": "vim", "window": 1, "active": false}
  ],
  "panes": [
    {"id": 1, "tab": 1, "active": true, "title": "zsh", "cols": 80, "rows": 24},
    {"id": 2, "tab": 1, "active": false, "title": "bash", "cols": 40, "rows": 24},
    {"id": 3, "tab": 2, "active": true, "title": "vim", "cols": 80, "rows": 24}
  ],
  "layouts": [
    {"tab": 1, "tree": {"type":"split","dir":"horizontal","ratio":500,"first":{"type":"leaf","pane":1},"second":{"type":"leaf","pane":2}}, "active": 1},
    {"tab": 2, "tree": {"type":"leaf","pane":3}, "active": 3}
  ],
  "outputs": [[1, "hello world\n"], [2, ""], [3, "..."]],
  "status": "Connected",
  "active_session": 1,
  "active_window": 1,
  "active_tab": 1,
  "active_pane": 1
}
```

> ID 全部为 muxterm 数字 ID（1, 2, 3...），不含 tmux `@N`/`$N`/`%N`。

### 7.4 序列号（seq）

**建议**（未定）：Snapshot / 事件附带单调递增 seq，供前端判断是否有遗漏：

```json
{"seq": 142, "events": [...]}
```

- 每次 `take_events` 返回的事件携带 `seq` 范围。
- 每次 Snapshot 携带当前 `seq`。
- 前端可比较本地 seq 与 Snapshot seq 判断是否需要全量刷新。
- **v1 是否实现**：未定（现有代码无 seq）。如果实现，加在 `StateChange` 和 Snapshot 的 wrapper 里，不改现有 enum。

### 7.5 错误

| 层 | 错误类型 | 表示 |
|----|---------|------|
| Transport | `TransportError`（thiserror） | spawn 失败 / read-write 失败 / resize 失败 |
| Runtime | `RuntimeError`（thiserror） | 不支持的 Task / ID 不存在 / backend 未连接 |
| Backend | `anyhow::Error` | execute 返回 `Err(anyhow)`；或 `Ok(TaskOutcome::Rejected{reason})` |
| FFI | 返回码 `-1` + `muxterm_last_error` 字符串 | 见 §9.6 |
| CLI | stderr 文本 + exit code 1 | `错误: <message>` |

### 7.6 版本兼容

| 变更类型 | 兼容性 |
|---------|--------|
| 新增 Task 变体 | 向后兼容（旧前端忽略未知 Task） |
| 新增 StateChange 变体 | 向后兼容（旧前端忽略未知 type_） |
| 新增 Snapshot 字段 | 向后兼容（旧前端忽略未知字段） |
| 修改已有字段语义 | **breaking**，需版本号 |
| 删除已有字段/变体 | **breaking** |

**版本协商**（未定）：
- FFI：`muxterm_new` 可接受 `api_version` 参数，核心返回支持的最大版本。
- CLI：`--api-version 1` 请求特定版本。
- v1 不强制实现版本协商；v2 引入。

### 7.7 JSON / NDJSON 示例

**CLI 命令输出（JSON，单行）：**
```bash
$ muxterm list-panes -s dev -t 1
[{"id":1,"active":true,"cols":80,"rows":24},{"id":2,"active":false,"cols":40,"rows":24}]
```

**CLI watch 模式（NDJSON，每行一个事件）：**
```bash
$ muxterm watch -s dev
{"seq":1,"type":"PaneOutput","pane":1,"data":"aGVsbG8="}
{"seq":2,"type":"PaneAdded","pane":2,"tab":1}
{"seq":3,"type":"LayoutChanged","tab":1,"layout":{"type":"split","dir":"horizontal","ratio":500,"first":{"type":"leaf","pane":1},"second":{"type":"leaf","pane":2}}}
{"seq":4,"type":"ActivePaneChanged","tab":1,"pane":2}
```

> `data` 字段为 base64 编码（字节流 → JSON 安全文本）。

**dump-state（JSON，单行）：**
```bash
$ muxterm dump-state -s dev
{"sessions":[...],"windows":[...],"tabs":[...],"panes":[...],"layouts":[...],"outputs":[[1,"aGVsbG8="]],"status":"Connected","active_session":1,...}
```

---

## 8. CLI v1 语法

默认输出 JSON。所有命令支持全局参数 `-s <session>` / `-L <socket>` / `--format <json|text>`。

### 8.1 命令树

```
muxterm
├── ssh
│   ├── hosts                         # 列出 ~/.ssh/config Host 别名
│   └── <user@host|alias> [<cmd>]     # 经 SSH 执行（reserved，v1 可只列 hosts）
├── shell
│   └── open [-s NAME] [-w W] [-t T]  # 打开 shell session（local/ssh）
├── tmux
│   ├── session
│   │   ├── list [-L SOCKET] [--remote ALIAS]  # 列出 tmux session
│   │   ├── new  -s NAME [-L SOCKET] [--remote ALIAS]
│   │   └── attach -s NAME [-L SOCKET] [--remote ALIAS]
│   ├── window ...                    # = tab 的别名（v1 固定 1 window）
│   ├── tab ...
│   └── pane ...
├── fs
│   └── list <PATH> [--remote ALIAS]  # 列出目录
├── settings                          # 显示/修改配置（reserved）
└── (legacy flat commands)            # 兼容现有语法
```

### 8.2 命令清单

#### SSH Discovery

| 命令 | 语法 | 输出 |
|------|------|------|
| `ssh hosts` | `muxterm ssh hosts` | `[{alias,hostname,port,user}]` |

#### Shell 管理

| 命令 | 语法 | 说明 |
|------|------|------|
| `shell open` | `muxterm shell open [-s NAME] [-w W] [-t T] [--remote ALIAS]` | 创建/连接 shell session；无 `--remote` 为 local-shell，有则为 ssh-shell |

#### tmux session

| 命令 | 语法 | 输出 |
|------|------|------|
| `tmux session list` | `muxterm tmux session list [-L SOCKET] [--remote ALIAS]` | `[{name,windows,attached,created}]` |
| `tmux session new` | `muxterm tmux session new -s NAME [-L SOCKET] [--remote ALIAS]` | 创建新 session（启动 daemon 或远程 tmux） |
| `tmux session attach` | `muxterm tmux session attach -s NAME [-L SOCKET] [--remote ALIAS]` | attach 到已有 session |

#### Window（v1 固定 1 个，保留命令）

| 命令 | 语法 | 输出 |
|------|------|------|
| `new-window` | `muxterm new-window [-s NAME] [-n NAME]` | v1: 预留（no-op 或创建虚拟 window） |
| `list-windows` | `muxterm list-windows [-s NAME]` | `[{id,name,session,tabs,active}]` |
| `select-window` | `muxterm select-window -t wN [-s NAME]` | 切换激活 window |
| `rename-window` | `muxterm rename-window <new_name> [-s NAME]` | 重命名 |
| `kill-window` | `muxterm kill-window [-t wN] [-s NAME]` | 关闭 |

#### Tab

| 命令 | 语法 | 输出 |
|------|------|------|
| `new-tab` | `muxterm new-tab [-s NAME] [-n NAME] [-t wN]` | `{"id":N,"name":"..."}` |
| `list-tabs` | `muxterm list-tabs [-s NAME] [-t wN]` | `[{id,name,panes,active}]` |
| `select-tab` | `muxterm select-tab -t tN [-s NAME]` | |
| `rename-tab` | `muxterm rename-tab <new_name> [-s NAME]` | |
| `kill-tab` | `muxterm kill-tab [-t tN] [-s NAME]` | |

#### Pane

| 命令 | 语法 | 输出 |
|------|------|------|
| `split-pane` | `muxterm split-pane [-h] [-s NAME] [-t tN] [-p pN] [-l SIZE]` | `{"id":N,"cols":W,"rows":H}` |
| `list-panes` | `muxterm list-panes [-s NAME] [-t tN]` | `[{id,active,cols,rows,title}]` |
| `select-pane` | `muxterm select-pane -p pN [-s NAME]` | |
| `resize-pane` | `muxterm resize-pane -p pN [-x W] [-y H] [-s NAME]` | |
| `kill-pane` | `muxterm kill-pane [-p pN] [-s NAME]` | |

#### 输入输出

| 命令 | 语法 | 输出 |
|------|------|------|
| `send-keys` | `muxterm send-keys -p pN <text> [-s NAME]` | |
| `write-raw` | `muxterm write-raw -p pN <data> [-s NAME]` | |
| `capture-pane` | `muxterm capture-pane [-p pN] [-S LINES] [-s NAME]` | pane 输出文本 |
| `display-message` | `muxterm display-message -p pN -F <fmt> [-s NAME]` | tmux format（仅 tmux 模式） |
| `list-layout` | `muxterm list-layout [-s NAME] [-t wN]` | 布局树 |
| `dump-state` | `muxterm dump-state [-s NAME]` | 完整快照 JSON |

#### fs

| 命令 | 语法 | 输出 |
|------|------|------|
| `fs list` | `muxterm fs list <PATH> [--remote ALIAS]` | `[{name,is_dir,size,modified}]` |

#### settings（reserved）

| 命令 | 语法 | 说明 |
|------|------|------|
| `settings get` | `muxterm settings get [KEY]` | 读配置（reserved） |
| `settings set` | `muxterm settings set <KEY> <VALUE>` | 写配置（reserved） |

#### session 管理（daemon）

| 命令 | 语法 | 说明 |
|------|------|------|
| `new-session` | `muxterm new-session -s NAME [-L SOCKET]` | 创建 daemon session |
| `list-sessions` | `muxterm list-sessions` | 列出活跃 daemon session（扫描 socket） |
| `kill-session` | `muxterm kill-session [-s NAME]` | 关闭 daemon |
| `attach-session` | `muxterm attach-session -t <target>` | attach tmux session |
| `detach` | `muxterm detach [-s NAME]` | detach（关 client，不 kill daemon） |
| `rename-session` | `muxterm rename-session <new_name>` | |
| `watch` | `muxterm watch [-s NAME]` | 长连接事件流（NDJSON） |

### 8.3 全局参数

| 参数 | 说明 |
|------|------|
| `-s, --session <NAME>` | session 名（daemon 模式 / attach 目标） |
| `-L, --socket <NAME>` | tmux socket 名（`tmux -L`） |
| `-w, --window <N>` | window 编号（默认 1） |
| `-t, --tab <N>` | tab 编号（默认 active） |
| `-p, --pane <N>` | pane 编号（默认 active） |
| `--remote <ALIAS>` | SSH 别名（ssh 模式） |
| `--format <json\|text>` | 输出格式（默认 json） |
| `-v, --verbose` | 详细日志 |

### 8.4 与现有 CLI 的关系

现有 `cli/command.rs` 的 30 个变体作为 **legacy flat commands** 保留兼容：
- `new-session` / `list-sessions` / `split-pane` / `send-keys` 等保持现有语法。
- 新增 `ssh hosts` / `shell open` / `tmux session list` / `fs list` / `watch` 等为 v1 新命令。
- 旧命令的 `-t @1`（tmux 格式）应逐步废弃，改为 `-p 1`（muxterm 格式）；v1 过渡期两者并存。

---

## 9. FFI ABI 草案

### 9.1 Opaque Handle

```c
struct MuxtermHandle;  // opaque，C 侧只持有指针
struct MuxtermHandle* muxterm_open(const struct MuxtermOpenSpec* spec);
void muxterm_free(struct MuxtermHandle* h);
```

- 一个 handle = 一个 TerminalModel + 一个 tokio runtime + 缓冲区。
- `muxterm_open` 接受 `MuxtermOpenSpec`（见 §9.2），返回 opaque 指针；失败返回 NULL + `muxterm_last_error`。
- `muxterm_free` 唯一释放点：`Box::from_raw` → `shutdown()` → drop。

### 9.2 Open Spec

```c
struct MuxtermOpenSpec {
    uint32_t api_version;       // 请求的 API 版本（v1 = 1）
    uint32_t mode;              // RuntimeMode: 0=LocalShell, 1=LocalTmux, 2=SshShell, 3=SshTmux
    const char* session_name;   // session 名（可选）
    const char* tmux_socket;    // tmux -L socket 名（可选，仅 tmux 模式）
    const char* ssh_alias;      // SSH 别名（可选，仅 ssh 模式）
    // 新增字段只能加在末尾，旧代码传的 spec 较短时核心按 api_version 兼容
};

#define MODE_LOCAL_SHELL  0u
#define MODE_LOCAL_TMUX   1u
#define MODE_SSH_SHELL    2u
#define MODE_SSH_TMUX     3u
```

```c
struct MuxtermHandle* muxterm_open(const struct MuxtermOpenSpec* spec);
// 旧版兼容（如果不传 spec）：
struct MuxtermHandle* muxterm_new(const char* backend_type, const char* socket, const char* session);
// muxterm_new 在 v1 保留为 wrapper，内部转成 OpenSpec
```

### 9.3 Discovery

```c
// SSH host discovery（不需要 handle）
struct CSshHost {
    const char* alias;
    const char* hostname;
    uint16_t port;
    const char* user;
};
int muxterm_discover_ssh_hosts(struct CSshHost* out, int max_count);
// 返回写入数量；指针在下次调用前有效（或由调用方 strdup）

// tmux session discovery（不需要 handle）
struct CTmuxSession {
    const char* name;
    uint32_t windows;
    uint8_t attached;
    uint64_t created;
};
int muxterm_discover_tmux_sessions(const char* socket, const char* ssh_alias,
                                    struct CTmuxSession* out, int max_count);
// socket=NULL 用默认；ssh_alias=NULL 查本地

// fs list（不需要 handle）
struct CFsEntry {
    const char* name;
    uint8_t is_dir;
    uint64_t size;
    uint64_t modified;
};
int muxterm_discover_fs_list(const char* path, const char* ssh_alias,
                              struct CFsEntry* out, int max_count);
```

> Discovery 函数不需要 handle（无状态查询）。字符串指针由内部静态缓冲持有，**下次同类调用前有效**；调用方需立即拷贝。

### 9.4 Snapshot

```c
struct CSession {
    uint32_t id;
    const char* name;
    uint8_t is_active;
};
struct CWindow {
    uint32_t id;
    const char* name;
    uint32_t session_id;
    uint8_t is_active;
};
struct CTab {
    uint32_t id;
    const char* name;
    uint8_t is_active;
};
struct CPane {
    uint32_t id;
    uint16_t cols;
    uint16_t rows;
    uint8_t is_active;
    const char* title;     // v1 新增
};
struct CLayoutNode {
    uint32_t type_;        // 0=leaf, 1=split_h, 2=split_v
    uint32_t pane_id;
    uint32_t ratio;
    const struct CLayoutNode* first;
    const struct CLayoutNode* second;
};

// 查询函数
int muxterm_get_sessions(struct MuxtermHandle* h, struct CSession* out, int max_count);
int muxterm_get_windows(struct MuxtermHandle* h, struct CWindow* out, int max_count);
int muxterm_get_tabs(struct MuxtermHandle* h, struct CTab* out, int max_count);
int muxterm_get_panes(struct MuxtermHandle* h, uint32_t tab_id, struct CPane* out, int max_count);
int muxterm_get_layout(struct MuxtermHandle* h, uint32_t tab_id, struct CLayoutNode* out);
int muxterm_get_pane_output(struct MuxtermHandle* h, uint32_t pane_id, uint8_t* buf, size_t buf_len);
int muxterm_get_pane_output_len(struct MuxtermHandle* h, uint32_t pane_id);  // 返回总字节数
uint32_t muxterm_get_status(struct MuxtermHandle* h);  // 0=Disconnected,1=Connecting,2=Connected,3=Error,4=Exited
```

### 9.5 Poll Events

```c
struct CStateChange {
    uint32_t type_;        // STATE_* 常量
    uint32_t pane_id;
    uint32_t tab_id;
    uint32_t window_id;
    const uint8_t* data;   // borrowed：handle 持有，下次 poll 前有效
    size_t data_len;
    const char* name;      // borrowed：handle 持有
};

int muxterm_poll_events(struct MuxtermHandle* h, struct CStateChange* out, int max_count);
// 返回写入数量（>=0）或 -1（err）
```

### 9.6 Execute / Send Input / Error

```c
struct CTask {
    uint32_t type_;        // TASK_* 常量
    uint32_t target_pane;
    uint32_t target_tab;
    uint32_t target_window;
    uint32_t dir;          // DIR_HORIZONTAL / DIR_VERTICAL
    const char* name;      // 新名（rename）/ 命令名
};

int muxterm_execute(struct MuxtermHandle* h, const struct CTask* task);
int muxterm_send_input(struct MuxtermHandle* h, uint32_t pane_id, const uint8_t* data, size_t len);
int muxterm_send_keys(struct MuxtermHandle* h, uint32_t pane_id, const char* keys_str);
// send_keys: keys_str 为 tmux 特殊键名（"Enter"/"C-c"/"Up"）或逐字文本
int muxterm_resize_pane(struct MuxtermHandle* h, uint32_t pane_id, uint16_t cols, uint16_t rows);

// 错误
const char* muxterm_last_error(struct MuxtermHandle* h);
// 返回 handle 内部最近一次错误字符串（borrowed，下次调用前有效）
int muxterm_last_error_code(struct MuxtermHandle* h);
// 返回错误码（0=无错误，>0=具体码）
```

### 9.7 内存所有权（核心约束）

**原则：不长期暴露 Rust 裸指针给平台层。**

| 数据类型 | 所有权 | 生命周期 | 调用方责任 |
|---------|--------|---------|-----------|
| `MuxtermHandle*` | owned by C | `muxterm_open` → `muxterm_free` | 确保 free 一次 |
| `CStateChange.data` | **borrowed** from handle | 下次 `muxterm_poll_events` 前 | 如需保留，立即 `memcpy` |
| `CStateChange.name` | **borrowed** from handle | 下次 `muxterm_poll_events` 前 | 如需保留，立即 `strdup` |
| `CTab.name` / `CSession.name` / `CWindow.name` | **borrowed** from handle | 下次同类 `get_*` 调用前 | 如需保留，立即 `strdup` |
| `CPane.title` | **borrowed** from handle | 下次 `muxterm_get_panes` 前 | 如需保留，立即 `strdup` |
| `CLayoutNode` 子节点指针 | **borrowed** from handle 内部池 | 下次 `muxterm_get_layout` 前 | 如需保留，深拷贝整棵树 |
| `muxterm_get_pane_output` 的 `buf` | **owned by C** | 调用方管理 | C 提供缓冲，Rust copy 写入 |
| `CSshHost.alias` / `CTmuxSession.name` | **borrowed** from static | 下次同类 discovery 调用前 | 如需保留，立即 `strdup` |
| `muxterm_last_error` 返回值 | **borrowed** from handle | 下次任何 `muxterm_*` 调用前 | 如需保留，立即 `strdup` |

**禁止**：
- 平台层把 borrowed 指针存到 struct 字段长期持有（下次调用后悬垂）。
- 平台层 `free()` 任何 borrowed 指针（不是 C malloc 的）。
- 跨线程共享 borrowed 指针（handle 不是 `Send`/`Sync`）。

**CTask 中的 `name`**：owned by C（调用方分配/释放），核心在 `execute` 期间 `CStr::from_ptr` 借用，执行后不持有。

### 9.8 ABI 版本兼容

- `MuxtermOpenSpec.api_version`：C 侧请求的版本，核心返回实际支持版本。
- 新增 `#[repr(C)]` 字段加在结构体末尾；旧 C 代码传较短 struct，核心按 `api_version` 只读已知字段。
- 新增 FFI 函数不影响旧调用方。
- **v1 稳定**的函数集：`muxterm_open`/`free`/`connect`/`shutdown`/`execute`/`send_input`/`resize_pane`/`poll_events`/`get_tabs`/`get_panes`/`get_layout`/`get_pane_output`。
- **v1 未定**的函数集：`muxterm_send_keys`/`get_sessions`/`get_windows`/`get_pane_title`/`get_pane_output_len`/`get_status`/`last_error`/`discover_*`（草案，可能在 v1 内确定或推迟）。

### 9.9 与现有 FFI 的差异

| 方面 | 现有（`core/ffi`） | 本设计草案 |
|------|-------------------|-----------|
| 创建 | `muxterm_new(backend_type, socket, session)` | `muxterm_open(MuxtermOpenSpec)` + mode 枚举 |
| Mode | 字符串 "local"/"tmux"/"daemon" | 枚举 0-3（四种模式） |
| Discovery | 无 | `muxterm_discover_ssh_hosts` / `discover_tmux_sessions` / `discover_fs_list` |
| Sessions | 无 | `muxterm_get_sessions` |
| Windows | 无 | `muxterm_get_windows` |
| Pane title | 无 | `CPane.title` |
| Error | -1 only | `muxterm_last_error` / `last_error_code` |
| Send keys | 无（只有 send_input） | `muxterm_send_keys`（特殊键名） |
| Output len | 无 | `muxterm_get_pane_output_len` |
| Status | 从事件推断 | `muxterm_get_status` |

> **迁移**：现有 `muxterm_new` 保留为 wrapper（内部转 `MuxtermOpenSpec`），不 break 现有调用方。新代码用 `muxterm_open`。

---

## 10. TUI / Linux GTK / macOS SwiftUI 共同边界

### 10.1 共用核心

所有平台共用 `core/` 的纯逻辑层 + FFI C ABI：

```
core/ (Rust, 纯逻辑 + C ABI)
├── model/     TerminalModel / Backend / State / Task / Layout
├── backend/  ShellRuntime / TmuxControlRuntime / DaemonBackend
├── transport/ LocalProcessTransport / SshProcessTransport  (新增)
├── tmux/     protocol / command / pty  (复用)
├── ssh/      SshSession / config  (复用 + 新增 ~/.ssh/config 解析)
├── terminal/ input / process / scrollback
├── config/   TOML 配置
├── types/    ID 类型
└── ffi/      C ABI 导出 (扩展)
```

### 10.2 平台桥接层

| 平台 | 桥接文件 | 语言 | 机制 |
|------|----------|------|------|
| Linux GTK | `platform/linux/ffi_bridge.rs` | Rust | 同进程 `extern "C"` 调用 |
| TUI | `platform/tui/ffi_bridge.rs` | Rust | 同进程 `extern "C"` 调用 |
| macOS | `platform/macos/CoreBridge/CoreBridge.swift` | Swift | C ABI import（`muxterm.h`） |

**三端桥接同构**（现有模式保留扩展）：
- 都定义 `BridgeEvent` / `BridgeLayout` / `BridgeTab` / `BridgePane` / `BridgeSession` / `FrameSnapshot`。
- 都用 `poll_events` → 拷贝出 owned 数据 → 渲染。
- 都用 `snapshot()` 一次性拉取完整渲染快照。
- 轮询频率：GTK 用 `glib::timeout_add_local`（16ms）；TUI 在事件循环 poll；macOS 用 `DispatchSource`/Timer。

### 10.3 共同边界定义

**所有平台必须实现的行为**（遵循 `ARCHITECTURE.md` §2）：
1. 窗口/tab/pane 生命周期一致（§2.1-2.3）。
2. 嵌套分割模型一致（§2.4）。
3. 焦点管理一致（§2.5）。
4. 进程名自动更新一致（§2.6）。
5. 快捷键映射一致（Alt+N/T/D/1-9/[]/R/P）。
6. 配置文件格式一致（`~/.config/muxterm/config.toml`）。
7. 主题格式一致（`configs/themes/<name>.toml`）。

**FFI 桥接必须处理的 StateChange 事件集**（三端不能遗漏）：
- `PaneOutput` → 喂终端渲染器
- `LayoutChanged` → 重建 pane 容器布局
- `TabAdded`/`TabClosed`/`TabRenamed` → 更新 tab 栏
- `PaneAdded`/`PaneClosed` → 创建/销毁 pane 渲染器
- `ActiveTabChanged`/`ActivePaneChanged` → 更新高亮/焦点
- `PaneResized` → 调整渲染器尺寸
- `BackendStatusChanged` → 更新状态栏

### 10.4 必须重新实现的（平台特定）

| 模块 | Linux GTK | TUI | macOS |
|------|-----------|-----|-------|
| 应用启动 | `app.rs` | `app.rs` | `AppDelegate.swift` |
| 窗口 | `window.rs` | `render.rs` | `MainWindow.swift` |
| Tab 栏 | `tab_bar.rs` | `render.rs` | `TabBar.swift` |
| Pane 渲染 | `pane_view.rs`（vte4） | `render.rs`（ANSI） | `TerminalView.swift`（SwiftTerm） |
| 布局容器 | `notebook.rs`（GtkPaned） | `render.rs` | `PaneLayout.swift` |
| 命令面板 | `command_palette.rs` | `app.rs` | `ContentView.swift` |
| 快捷键 | `keymap.rs` | `app.rs` | `KeyBindings.swift` |
| 主题 | `theme.rs` | `render.rs` | Swift Color |
| SSH host 选择 | `tmux_dialog.rs` | `app.rs` | QuickPick |

---

## 11. 测试契约

### 11.1 测试矩阵

| 类别 | 测试内容 | 依赖 | 超时 |
|------|---------|------|------|
| **协议单元** | `%output`/`%layout-change`/`%window-add` 等消息解析 | 无 | 2s |
| **命令构造** | `TmuxCommand` 字符串生成 | 无 | 2s |
| **ID 映射** | muxterm ID ↔ tmux ID 双向映射 | 无 | 2s |
| **Transport** | LocalProcessTransport spawn/read/write/resize/kill | `true`/`sleep` | 5s |
| **Transport (SSH)** | SshProcessTransport spawn `ssh <alias> echo ok` | 可达 SSH host + 免密 | 15s |
| **ShellRuntime** | local-shell 四种模式：split/close/switch/send-keys/capture | `cat`/`sleep` | 10s |
| **TmuxControlRuntime** | local-tmux：attach + %消息 → StateChange | tmux 3.x | 15s |
| **ssh-shell** | SshProcessTransport + ShellRuntime 端到端 | SSH host | 30s |
| **ssh-tmux** | SshProcessTransport + TmuxControlRuntime 端到端 | SSH host + tmux | 30s |
| **CLI** | parse_cli_command + format_output（JSON/text） | 无 | 2s |
| **CLI daemon** | new-session + list-sessions + send-keys + kill-session | 无 | 15s |
| **CLI tmux** | tmux session list/new/attach + split-pane + capture | tmux | 15s |
| **CLI ssh** | ssh hosts + shell open --remote | ~/.ssh/config | 5s |
| **FFI** | muxterm_open/connect/execute/poll/free（local-shell） | 无 | 10s |
| **FFI (tmux)** | muxterm_open LocalTmux + get_tabs/panes/layout | tmux | 15s |
| **FFI (ssh)** | muxterm_open SshShell + poll PaneOutput | SSH host | 30s |
| **TUI** | muxterm --tui 进程 + 宿主 tmux capture-pane 断言 | tmux + 编译 | 30s |
| **GTK** | 窗口/tab/pane/命令面板 UI | DISPLAY/xvfb | 30s |
| **macOS** | CoreBridge FFI + tmux attach 2tab3pane | tmux | 30s |
| **E2E** | 真实 muxterm 二进制 + tmux -L 隔离 | 编译 | 30s |

### 11.2 硬超时要求

**所有可能卡住的测试必须有硬超时**（防止 CI 挂起）：

```rust
// 测试模板
#[test]
fn test_with_timeout() {
    let result = std::thread::scope(|s| {
        let h = s.spawn(|| {
            // 测试逻辑
            do_something()
        });
        // 硬超时：15 秒后 panic
        let timeout = Duration::from_secs(15);
        match h.join_timeout(timeout) {
            Ok(r) => r,
            Err(_) => panic!("测试超时（{}s）", timeout.as_secs()),
        }
    });
}
```

或用 `tokio::time::timeout` 包裹 async 测试：

```rust
#[tokio::test]
async fn test_tmux_attach_timeout() {
    tokio::time::timeout(Duration::from_secs(15), async {
        // 测试逻辑
    })
    .await
    .expect("测试超时 15s");
}
```

**超时分级**：
- 纯单元测试：2s
- 本地进程测试（spawn/pty）：5-10s
- tmux 集成测试：15s
- SSH 远程测试：30s
- UI 测试（GTK/TUI/macOS）：30s

### 11.3 四种模式的测试策略

| 模式 | 单元测试 | 集成测试 | E2E |
|------|---------|---------|-----|
| local-shell | ShellRuntime + LocalProcessTransport（`true`/`sleep`/`cat`） | CLI daemon + LocalBackend | muxterm binary |
| local-tmux | TmuxControlRuntime + 协议解析 | TmuxBackend + tmux -L 隔离 | muxterm binary + tmux |
| ssh-shell | SshProcessTransport（mock SSH？） | 需可达 SSH host（`#[ignore]` 默认） | muxterm binary + ssh |
| ssh-tmux | TmuxControlRuntime + SshProcessTransport（mock） | 需可达 SSH host（`#[ignore]`） | muxterm binary + ssh + tmux |

**SSH 测试策略**：
- 默认 `#[ignore]`（不跑 CI），需手动 `cargo test -- --ignored`。
- 或用 `docker` 起本地 sshd 容器（CI 可选）。
- Transport 层可 mock：实现 `MockTransport`（fake read/write），测 TmuxControlRuntime 不需真 SSH。

### 11.4 协议传输测试

| 测试 | 说明 |
|------|------|
| 半行缓冲 | Transport 一次 read 返回半行，下次补全，parse_line 正确 |
| C 转义解码 | `%output @1 "hello\nworld"` → `hello\nworld`（真换行） |
| DCS 前缀 | `\x1bP1000p%output ...` → 去前缀后正确解析 |
| 多 pane 输出交错 | %output @1 / %output @2 交替到达，分别路由 |
| 命令响应 | `%begin`/`%end` 之间的行正确收集 |
| layout tree 解析 | tmux `window_layout` 字符串 → LayoutNode |

### 11.5 现有测试保留

现有 216+ 测试全部保留，新增测试补充以上矩阵。现有测试不 break（除非明确标记 deprecated）。

---

## 12. 分阶段实施顺序

### 阶段 0：文档与设计（本文）

- 交付物：`docs/TRANSPORT-PROTOCOL-ARCHITECTURE.md`（本文）
- 不改代码。

### 阶段 1：Transport 抽象（local）

- 交付物：
  - `core/transport/` 模块（`Transport` trait + `LocalProcessTransport`）
  - 从 `core/backend/local.rs` / `core/tmux/pty.rs` 提取 PTY 管理逻辑到 `LocalProcessTransport`
  - `LocalBackend` 重构为 `ShellRuntime + LocalProcessTransport`（行为不变，测试不 break）
  - `TmuxBackend` 重构为 `TmuxControlRuntime + LocalProcessTransport`（行为不变）
- 风险：重构破坏现有 216+ 测试；需保证行为完全一致。
- 验证：`cargo test` 全绿 + `tests/cli_integration.rs` + `tests/tmux_backend_integration.rs`。

### 阶段 2：ID 隔离强化

- 交付物：
  - `TmuxControlRuntime` 显式 ID 映射表（`tab_map` / `pane_map`）
  - `StateChange` 事件中的 ID 全部改为 muxterm ID（移除 tmux `@N`/`%N` 泄漏）
  - Snapshot 的 `outputs` key 改为 muxterm PaneId 数字
  - CLI 输出 ID 全部用 muxterm 格式（`-p 1` 而非 `-t @1`）
- 风险：CLI 语法 breaking（`-t @1` → `-p 1`）；需过渡期兼容。
- 验证：协议单元测试 + CLI 格式化测试更新。

### 阶段 3：SshProcessTransport（spawn 系统 ssh）

- 交付物：
  - `core/transport/SshProcessTransport`（spawn `ssh <alias> <command>` 到 PTY）
  - `core/ssh/config` 模块（解析 `~/.ssh/config` Host 别名）
  - CLI `ssh hosts` / `fs list --remote` / `tmux session list --remote`
- 风险：系统 ssh 不可用（Windows）；PTY 转发兼容性。
- 验证：`SshProcessTransport` 单元测试（mock 或 `#[ignore]` 真实 SSH）。

### 阶段 4：SSH Runtime（ssh-shell / ssh-tmux）

- 交付物：
  - `ShellRuntime` 接受 `SshProcessTransport`（多 pane 各自一个 Transport）
  - `TmuxControlRuntime` 接受 `SshProcessTransport`（单 Transport，远端 `tmux -CC`）
  - CLI `shell open --remote <alias>` / `tmux session attach --remote <alias>`
  - `RuntimeMode` enum + `create_backend()` 工厂
- 风险：远端 pty resize / 信号传递差异；ssh 进程退出码语义。
- 验证：`#[ignore]` 真实 SSH 测试 + mock Transport 测试。

### 阶段 5：FFI 扩展

- 交付物：
  - `muxterm_open(MuxtermOpenSpec)` + mode 枚举（替代 `muxterm_new` 字符串）
  - `CPane.title`、`CSession`、`CWindow` 结构体
  - `muxterm_get_sessions` / `get_windows` / `get_pane_title` / `get_pane_output_len` / `get_status`
  - `muxterm_send_keys`、`muxterm_last_error` / `last_error_code`
  - `muxterm_discover_ssh_hosts` / `discover_tmux_sessions` / `discover_fs_list`
  - CTask 补全 `NEW_WINDOW`/`CLOSE_WINDOW`/`SWITCH_WINDOW`/`RENAME_*`/`SWITCH_SESSION`
  - 同步更新 `muxterm.h` + Swift `CoreBridge.swift` + Rust `ffi_bridge.rs`
- 风险：ABI breaking（`CPane` 加字段）；需三端同步更新。
- 验证：FFI 单元测试 + macOS 集成测试 + TUI 集成测试。

### 阶段 6：CLI v1 新命令

- 交付物：
  - `ssh hosts` / `shell open` / `tmux session list|new|attach` / `fs list` / `watch`
  - `watch` 长连接 NDJSON 事件流
  - `--format json-pretty`
  - `format.rs` 迁移到 serde_json（消除手写 JSON 转义风险）
  - `cli_mode_tmux` 固定 sleep → 事件等待
- 风险：CLI 语法变更影响脚本兼容。
- 验证：CLI 集成测试 + E2E 测试。

### 阶段 7：daemon 优化

- 交付物：
  - `--watch` 长连接事件流
  - `DaemonBackend` 增量同步（首次全量 + 后续 diff + 定期校正）
  - `muxterm_get_pane_output` 分块读（`get_pane_output_len` + offset/limit）
- 风险：增量同步丢事件；需 seq 机制。
- 验证：daemon 集成测试。

### 阶段 8：平台前端适配

- 交付物：
  - Linux GTK `ffi_bridge.rs` 适配新 FFI（CPane.title / get_sessions / ...）
  - TUI `ffi_bridge.rs` 同上
  - macOS `CoreBridge.swift` + `muxterm.h` 同步
  - 命令面板增加 SSH host 选择 / tmux session 选择（用 Discovery）
- 风险：三端不同步导致行为差异。
- 验证：TUI 集成 + GTK 集成 + macOS 集成。

---

## 13. 稳定 v1 vs 未承诺

### 13.1 v1 稳定（承诺不 breaking）

| 接口 | 说明 |
|------|------|
| Core Protocol 层级（Session/Window/Tab/Pane） | 四层模型固定 |
| ID 规则（`s{name}`/`wN`/`tN`/`pN`） | 格式固定 |
| `Task` enum 现有 18 变体 | 语义不变 |
| `StateChange` enum 现有 17 变体 | 语义不变 |
| Snapshot JSON 结构 | 字段名/类型不变（新增允许） |
| FFI v1 函数集（§9.8） | 签名不变（新增允许） |
| `#[repr(C)]` v1 结构体布局 | 字段不变（末尾新增允许） |
| CLI 现有 flat 命令 | 语法保留（废弃标注，不删除） |
| `~/.config/muxterm/config.toml` 格式 | 字段不变（新增允许） |

### 13.2 未承诺（v1 内可能变更）

| 接口 | 说明 |
|------|------|
| `MuxtermOpenSpec` / `muxterm_open` | 草案，可能在实现中调整字段 |
| `muxterm_discover_*` | 草案，签名可能变 |
| `muxterm_send_keys` | 草案，keys_str 格式可能变 |
| `muxterm_last_error` / `last_error_code` | 错误码体系未定 |
| `RuntimeMode` enum | 草案，变体名可能调整 |
| `Transport` trait | 草案，方法签名可能在实现中调整 |
| `SshProcessTransport` 实现方式 | spawn 系统 ssh vs 库级连接未最终定 |
| `seq` 序列号机制 | 未定是否在 v1 实现 |
| `--watch` 事件流协议细节 | NDJSON vs 二进制未最终定 |
| daemon 增量同步策略 | 未定 |
| `fs list --remote` 实现 | exec ls vs sftp 未定 |

### 13.3 未决问题

| # | 问题 | 选项 | 倾向 |
|---|------|------|------|
| 1 | `SshProcessTransport` spawn 系统 ssh vs 库级 async-ssh2-tokio | A: 系统 ssh（委托认证） B: 库级（无系统 ssh 依赖） | A（符合 §1.1.3 约束） |
| 2 | ShellRuntime 多 pane 是否各自独立 Transport | A: 是（每 pane 一个 Transport） B: 单 Transport + mux（复杂） | A（简单，复用现有模式） |
| 3 | `seq` 序列号是否在 v1 实现 | A: 是 B: 否（v2） | B（v1 保持简单） |
| 4 | CLI 旧 `-t @1` 格式兼容期 | A: v1 保留 B: 立即废弃 | A（过渡期） |
| 5 | `format.rs` 何时迁移 serde_json | A: 阶段 6 B: 独立 PR | A（与 CLI 新命令一起） |
| 6 | `muxterm.h` 是否用 cbindgen 自动生成 | A: 手写同步（现状） B: cbindgen | A 现状 / B 长期 |
| 7 | macOS SwiftUI vs AppKit | A: SwiftUI（现有） B: AppKit | A（现状，终端渲染用 SwiftTerm） |
| 8 | `DaemonBackend` 是否重构为 Transport + Runtime | A: 是 B: 保留为独立 backend | B（daemon 是 IPC client，不 fit Transport 抽象） |
| 9 | 远程 `fs list` 用 exec ls vs sftp | A: exec ls（简单） B: sftp（需 sftp-server） | A（v1） |
| 10 | SSH 测试是否用 Docker sshd 容器 | A: 是（CI 可复现） B: `#[ignore]` 手动 | A+B（容器可选，默认 ignore） |
| 11 | `Transport::read` 同步 vs 异步 | A: 同步（try_recv） B: 异步（tokio） | A（与 Backend::execute 同步一致） |
| 12 | 多 Window 何时支持 | A: v2 B: 后续 | B（v1 固定 1 个，协议预留） |

---

## 附录 A：现有代码与设计对照

| 设计概念 | 现有代码 | 重构方向 |
|---------|---------|---------|
| `Transport` trait | `core/tmux/pty.rs` + `core/backend/local.rs` 的 PTY 管理 | 提取为 `core/transport/` |
| `LocalProcessTransport` | `LocalBackend` 内部 portable-pty | 提取 |
| `SshProcessTransport` | `core/ssh/client.rs::RemoteTmuxClient`（库级） | 改为 spawn 系统 ssh |
| `ShellRuntime` | `LocalBackend` | 重命名 + 接受 Transport 参数 |
| `TmuxControlRuntime` | `TmuxBackend` | 重命名 + 接受 Transport 参数 + ID 映射强化 |
| `RuntimeMode` | `main.rs::cli_mode` 的 match | 提取为 enum + 工厂 |
| `SshHostDiscovery` | 无 | 新增 `core/ssh/config.rs` |
| `TmuxSessionDiscovery` | `main.rs::find_existing_tmux_session` | 提取为 trait |
| `FsDiscovery` | 无 | 新增 |
| `MuxtermOpenSpec` | `muxterm_new(backend_type, socket, session)` | 新增 spec 结构体 |
| ID 映射表 | `TmuxBackend` 内部隐式 | 显式化 `tab_map` / `pane_map` |

## 附录 B：与 ID-SYSTEM.md / LAYER-MAPPING.md 的一致性

- ID 规则与 `ID-SYSTEM.md` 一致（`s{name}`/`wN`/`tN`/`pN`）。
- 层级映射与 `LAYER-MAPPING.md` 一致（tmux window → muxterm Tab；muxterm Window 虚拟固定 1 个）。
- 本文补充：tmux 真实 ID（`$N`/`@N`/`%N`）严格隔离在 TmuxControlRuntime 内部。

## 附录 C：常量清单（与现有 `core/ffi/types.rs` 对齐）

```
RuntimeMode:  LOCAL_SHELL=0, LOCAL_TMUX=1, SSH_SHELL=2, SSH_TMUX=3
BackendStatus: Disconnected=0, Connecting=1, Connected=2, Error=3, Exited=4
StateChange.type_: PANE_OUTPUT=0, TAB_ADDED=1, TAB_CLOSED=2, LAYOUT_CHANGED=3,
  PANE_ADDED=4, PANE_CLOSED=5, ACTIVE_TAB_CHANGED=6, ACTIVE_PANE_CHANGED=7,
  TAB_RENAMED=8, PANE_RESIZED=9, BACKEND_STATUS=10, OTHER=99
Task.type_: SPLIT_PANE=0, NEW_TAB=1, SWITCH_TAB=2, CLOSE_PANE=3, CLOSE_TAB=4,
  NEXT_PANE=5, PREV_PANE=6, SHUTDOWN=7, SWITCH_PANE=8,
  (新增) NEW_WINDOW=9, CLOSE_WINDOW=10, SWITCH_WINDOW=11, RENAME_WINDOW=12,
  RENAME_TAB=13, SWITCH_SESSION=14, RENAME_SESSION=15, RESIZE_PANE_STEP=16
SplitDir: HORIZONTAL=0, VERTICAL=1
LayoutNode: LEAF=0, SPLIT_H=1, SPLIT_V=2
TransportSignal: HANGUP=0, TERM=1, KILL=2
```

---

> **本文档为架构设计，不含代码实现。所有接口为草案，标注 v1 稳定或未定。**
