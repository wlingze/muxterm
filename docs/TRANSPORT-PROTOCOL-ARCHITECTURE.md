# Muxterm 传输与协议架构

> **文档定位**：本文定义 muxterm 的主分层架构与核心接口契约。主链为
> **Frontend → Core Protocol → Runtime → Transport**，辅以横切的 **Config** 层和
> 连接前的 **Discovery** 查询能力。供后续实现 agent（Codex）按图施工。
>
> **基线**：`/home/wlz/Developer/self/muxterm` main `d69fab2`（2026-07-28）。
> **分支**：`design/transport-protocol`。
> **状态**：设计文档，不含代码实现。所有 Rust/Swift/C 接口为草案，标注「v1 稳定」或「未定」。
>
> 相关文档：
> - `PRODUCT.md` — 产品定位与路线图
> - `ARCHITECTURE.md` — 现有架构与交互模型
> - `docs/PROJECT-STRUCTURE.md` — 当前与目标目录结构
> - `docs/ARCHITECTURE-PLAN.md` — C ABI 拆分方案与平台前端方案
> - `docs/ID-SYSTEM.md` — ID 体系（本文 §3 扩展）
> - `docs/LAYER-MAPPING.md` — muxterm↔tmux 层级映射（本文 §5 引用）

---

## 1. 目标与非目标

### 1.1 目标

1. **主链清晰**：Frontend → Core Protocol → Runtime → Transport，四层各司其职，不混层。
2. **Core Protocol 是稳定产品语义**：Session → Window → Tab → Pane 层级、命令、事件、snapshot、ID 规则不随实现变更。
3. **Runtime 可扩展**：ShellRuntime、TmuxRuntime，以及未来 ZellijRuntime/其他 multiplexer runtime 可在不修改 Transport 或 Core Protocol 的前提下接入。
4. **Transport 可扩展**：LocalProcessTransport、SshProcessTransport，以及未来其他连接/字节流传输方式可在不修改 Runtime 或 Core Protocol 的前提下接入。
5. **SSH 是 Transport，不是 Runtime**：SSH 只提供连接和字节流；认证/密钥/agent/ProxyJump/known_hosts/密码交互全部委托系统 `ssh <alias>`，不设计自有 SSH 认证协议。
6. **Config 横切**：统一的配置层，CLI/FFI/TUI/Linux/macOS 都能读写；Config 不是 Runtime 或 Transport 的子层。
7. **Discovery 是连接前查询能力**：列出 SSH host 别名、tmux session、目录等，不建立长连接，不画进主运行时层。
8. **CLI 默认 JSON**；text 可选。
9. **FFI 不长期暴露 Rust 裸指针**：opaque handle + owned snapshot/event copy；平台侧 borrowed 指针只在单次调用内有效。

### 1.2 非目标

- 不实现自有 SSH 认证协议。
- 不实现自有终端模拟器（渲染由平台层负责）。
- 不在 v1 支持多 session 同时活跃（一个 Runtime 实例 = 一个 session 的一个 window）。
- 不在 v1 支持 Windows ConPTY（留到后续阶段）。
- 不在本文定义 UI 渲染细节。

---

## 2. 架构总览

### 2.1 主链与横切层

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Frontend                                     │
│   CLI  │  TUI (crossterm)  │  Linux GTK4  │  macOS SwiftUI  │ ...   │
└───────┬───────────────────────────────────────────────────────────┘
        │  Core Protocol (stable product semantics)
        │  Session → Window → Tab → Pane / Task / StateChange / Snapshot / ID
┌───────┴───────────────────────────────────────────────────────────┐
│                      Core Protocol                                  │
│  对外稳定的层级模型、命令、事件、快照、能力、错误、ID 规则           │
└───────┬───────────────────────────────────────────────────────────┘
        │  Task / StateChange / State (trait)
        │
┌───────┴───────────────────────────────────────────────────────────┐
│                       Runtime                                        │
│  ShellRuntime  │  TmuxRuntime  │  (future) ZellijRuntime │ ...     │
│  └ adapter：tmux control mode parser/command mapping（内部）        │
└───────┬───────────────────────────────────────────────────────────┘
        │  Transport trait (spawn_exec / read / write / resize / kill)
        │
┌───────┴───────────────────────────────────────────────────────────┐
│                      Transport                                       │
│  LocalProcessTransport  │  SshProcessTransport  │ (future) ...      │
│  本地 PTY               │  系统 ssh 进程 PTY    │                    │
└─────────────────────────────────────────────────────────────────────┘

  ┌─────────────────────────────────────────────────────────────────┐
  │                    Config（横切）                                 │
  │  统一配置 API：get / list / set / reset / 变更事件               │
  │  CLI / FFI / TUI / Linux / macOS 共享                           │
  └─────────────────────────────────────────────────────────────────┘

  ┌─────────────────────────────────────────────────────────────────┐
  │                  Discovery（连接前查询）                          │
  │  SSH hosts / tmux sessions / fs list                            │
  │  无状态，不建立长连接                                            │
  └─────────────────────────────────────────────────────────────────┘
```

### 2.2 数据流（运行时）

```
Frontend                    Core Protocol              Runtime              Transport
──────                      ────────────              ───────              ─────────
用户操作
  → Task ──────────────→  TerminalModel.execute(Task)
                             → Backend.execute(Task) ──→  Runtime.execute(Task)
                                                            → adapter（tmux command）
                                                            → Transport.write(cmd) ──→  pty/ssh stdin
                                                          Transport.read()  ←─────  pty/ssh stdout
                                                            ← adapter（%消息 parse）
                             ← Backend.take_events() ←──  Runtime.take_events()  (StateChange)
  ← poll_events / snapshot ←  TerminalModel
  → 渲染
```

### 2.3 四种模式组合

| 模式 | Transport | Runtime | 语义 |
|------|-----------|---------|------|
| **local-shell** | LocalProcessTransport | ShellRuntime | 本地 spawn shell 进程到 PTY，muxterm 自管 pane 分割 |
| **local-tmux** | LocalProcessTransport | TmuxRuntime | 本地 spawn `tmux -CC`，解析 %消息，pane 由 tmux 管理 |
| **ssh-shell** | SshProcessTransport | ShellRuntime | 远程经 SSH 执行 shell，pty 在远端 |
| **ssh-tmux** | SshProcessTransport | TmuxRuntime | 远程经 SSH 执行 `tmux -CC`，解析 %消息 |

**复用关系**：2 个 Transport × 2 个 Runtime = 4 种组合，每种组合构造一个 `Backend` 实例。

---

## 3. Core Protocol — 稳定产品语义

Core Protocol 是 muxterm 对外稳定的语义层。前端、CLI、FFI 只与 Core Protocol 交互。

### 3.1 层级模型（v1 稳定）

```
Session → Window → Tab → Pane
```

| 层级 | 说明 | 数量关系 | 可前端新建 |
|------|------|----------|-----------|
| Session | 一个终端会话（可后台 / 可 attach） | 每个 Runtime 实例 1 个 active | 是 |
| Window | 前端一级对象，类似浏览器窗口 | v1 固定 1 个，绑定 Session | v1 否（协议预留） |
| Tab | 窗口内的标签页 | 多个，用户可新建/关闭 | 是 |
| Pane | Tab 内的分割区域 | 多个，可分割 | 是 |

> **v1 约束**：一个 Runtime 实例 = 一个 Session 的一个 Window。多 Window 支持留到后续版本，但协议层已定义 Window 为一级对象，避免未来 breaking。

### 3.2 ID 规则（v1 稳定）

muxterm 对外只使用自有 ID，不暴露 tmux 的 `$N`/`@N`/`%N`。

| 层级 | ID 格式 | 示例 | 说明 |
|------|---------|------|------|
| Session | `s{name}` | `sdev` | 按名字引用，`[A-Za-z0-9_.-]+` |
| Window | `w{n}` | `w1` | 数字编号，从 1 开始；v1 固定 `w1` |
| Tab | `t{n}` | `t1`、`t2` | 数字编号，从 1 开始，按创建顺序 |
| Pane | `p{n}` | `p1`、`p3` | 数字编号，从 1 开始，按创建顺序 |

**组合路径**：
```
s{name}/w1/t2/p3   → 精确引用
s{name}/t2/p1      → 省略 window（默认 w1）
s{name}/p2         → 省略 window 和 tab（默认 w1 + active tab）
```

**CLI 简写**：`-s <name>` / `-w <n>` / `-t <n>` / `-p <n>`。

**ID 分配**：Tab/Pane 编号在父级内单调递增，不复用已关闭编号。Session name 用户指定，v1 不允许重名。

### 3.3 tmux 真实 ID 隔离

tmux 的 `$N`(session)、`@N`(window)、`%N`(pane) **只能存在于 TmuxRuntime 内部的 adapter**：
- adapter 维护 `muxterm_id ↔ tmux_id` 映射表。
- 前端、CLI、FFI、StateChange 事件、Snapshot 只携带 muxterm ID。
- 映射表不序列化到 Snapshot 的协议字段。

> 与 `docs/LAYER-MAPPING.md` 一致：tmux window → muxterm Tab；muxterm Window 虚拟固定 1 个。

### 3.4 命令（Task，v1 稳定）

```rust
pub enum Task {
    // 布局
    SplitPane { target: Option<PaneId>, dir: SplitDir, command: Option<Vec<String>>, workdir: Option<String> },
    ClosePane { target: PaneId },
    ResizePane { target: PaneId, cols: u16, rows: u16 },
    ResizePaneStep { target: PaneId, dir: SplitDir, delta: i32 },
    // 焦点
    SwitchPane { target: PaneId }, NextPane, PrevPane,
    // Window（v1 固定 1 个，保留命令）
    NewWindow { name: Option<String>, command: Option<Vec<String>>, workdir: Option<String> },
    CloseWindow { target: WindowId }, SwitchWindow { target: WindowId }, RenameWindow { target: WindowId, name: String },
    // Tab
    NewTab { window: WindowId, name: Option<String>, command: Option<Vec<String>>, workdir: Option<String> },
    CloseTab { target: TabId }, SwitchTab { target: TabId }, RenameTab { target: TabId, name: String },
    // Session
    SwitchSession { target: SessionId }, RenameSession { target: SessionId, name: String },
    // 输入
    SendKeys { target: PaneId, keys: Vec<KeyEvent> }, WriteRaw { target: PaneId, data: Vec<u8> },
    // 生命周期
    Shutdown,
}
```

### 3.5 事件（StateChange，v1 稳定）

```
PaneOutput{pane, data}   WindowAdded/Closed/Renamed   TabAdded/Closed/Renamed/ActiveTabChanged
PaneAdded/Closed/TitleChanged/Resized/ActivePaneChanged   ActiveWindowChanged
SessionChanged/Renamed/SessionsChanged   LayoutChanged{tab, layout}   BackendStatusChanged
```

所有 ID 为 muxterm ID。

### 3.6 Snapshot

完整状态快照（JSON），ID 全部为 muxterm 数字 ID，不含 tmux `@N`/`$N`/`%N`：

```json
{"sessions":[...],"windows":[...],"tabs":[...],"panes":[...],"layouts":[...],
 "outputs":[[1,"<base64>"]],"status":"Connected","active_session":1,...}
```

### 3.7 能力差异

不同模式支持的操作集不同。Runtime 在 `execute(Task)` 时对不支持的 Task 返回 `TaskOutcome::Rejected { reason }`。

| 操作 | local-shell | local-tmux | ssh-shell | ssh-tmux |
|------|:-:|:-:|:-:|:-:|
| new-session | ✅ | ✅ | ✅ | ✅ |
| attach/detach | ❌ | ✅ | ❌ | ✅ |
| list-sessions（跨 session） | ❌ | ✅ | ❌ | ✅ |
| new-tab / split-pane / resize / send-keys / capture / kill | ✅ | ✅ | ✅ | ✅ |
| display-message（tmux format） | ❌ | ✅ | ❌ | ✅ |

### 3.8 错误

| 层 | 类型 | 表示 |
|----|------|------|
| Transport | `TransportError` | spawn/read/write/resize 失败 |
| Runtime | `RuntimeError` | 不支持的 Task / ID 不存在 / 未连接 |
| Backend | `anyhow::Error` / `TaskOutcome::Rejected` | execute 返回 |
| FFI | 返回码 `-1` + `muxterm_last_error` | 见 §9 |
| CLI | stderr + exit code 1 | `错误: <message>` |

---

## 4. Runtime 层

Runtime 建立在 Transport 之上，理解终端语义（pane 生命周期）或复用协议语义（tmux %消息）。Runtime **不关心**连接是本地还是 SSH——那是 Transport 的职责。

### 4.1 可扩展设计

```
Runtime（trait 或共同行为）
├── ShellRuntime       — muxterm 自管 pane 分割 + shell 进程生命周期
├── TmuxRuntime        — tmux 控制模式；内部含 adapter（协议解析 + 命令映射 + ID 映射）
├── (future) ZellijRuntime  — zellij 或其他复用协议
└── (future) 其他 multiplexer runtime
```

**扩展规则**：新增 Runtime 不修改 Transport、不修改 Core Protocol。Runtime 通过实现 `Backend` trait 接入。

### 4.2 ShellRuntime

管理多个 shell 进程（一个 pane 一个），自行维护 Session/Window/Tab/Pane 层级与嵌套分割。

- 每个 pane 持有**一个独立 Transport 实例**（spawn 一个 shell 进程）。
- `split-pane`：在当前 pane 旁 spawn 新 Transport。
- `close-pane`：`transport.kill()` + 移除。
- `take_events`：遍历所有 pane 的 `transport.read()`，聚合成 `PaneOutput`。

> 本质是现有 `LocalBackend` 重构为 ShellRuntime + Transport。

### 4.3 TmuxRuntime

spawn 一个 `tmux -CC` 进程，解析 %消息，发送 tmux 命令，把 tmux 3 层模型映射为 muxterm 4 层。

- **单 Transport**：所有 pane 输出经 `%output %N "..."` 消息从同一 Transport 到达，Runtime 按 pane id 分发。
- **内部 adapter**（不扩散到 Transport）：
  - **ID Mapper**：`muxterm_pane ↔ tmux %N`、`muxterm_tab ↔ tmux @N`、tmux session name → muxterm session name。
  - **Protocol Parser**：复用现有 `core::tmux::protocol::parse_line()`（130+ 测试）。
  - **Command Builder**：复用现有 `core::tmux::command::TmuxCommand` 构造器。
- 命令经 `Transport::write()` 发送（命令字符串 + `\n`）。

> 现有 `TmuxBackend` 已实现大部分逻辑；重构为 TmuxRuntime 主要是抽出 Transport 接口、显式化 ID 映射表、从 StateChange 事件中清除 tmux ID。

### 4.4 Backend 组合

```rust
pub trait Backend: State {
    async fn connect(&mut self) -> Result<()>;
    fn execute(&mut self, task: &Task) -> Result<TaskOutcome>;
    fn take_events(&mut self) -> Vec<StateChange>;
    async fn shutdown(&mut self) -> Result<()>;
}

pub enum RuntimeMode {
    LocalShell,
    LocalTmux { socket: Option<String>, session: Option<String> },
    SshShell { alias: String },
    SshTmux { alias: String, session: Option<String> },
}

pub fn create_backend(mode: RuntimeMode) -> Box<dyn Backend> { ... }
```

### 4.5 Tmux Compatibility Adapter（TmuxRuntime 内部）

```
┌──────────────────────────────────────┐
│            TmuxRuntime               │
│  ┌────────────────────────────────┐  │
│  │     Tmux Adapter（内部）       │  │
│  │  ID Mapper  │ Protocol Parser │  │
│  │             │ Command Builder │  │
│  └────────────────────────────────┘  │
│             ↓↑ Transport              │
└──────────────────────────────────────┘
```

tmux ID（`$N`/`@N`/`%N`）严格隔离在 adapter 内，不越过 Runtime 边界。

---

## 5. Transport 层

Transport 是纯粹的字节流通道，不理解任何终端语义或复用协议。

### 5.1 可扩展设计

```
Transport（trait）
├── LocalProcessTransport  — 本地 portable-pty spawn
├── SshProcessTransport    — spawn 系统 ssh <alias> 进程
├── (future) 其他连接/字节流传输方式
```

**扩展规则**：新增 Transport 不修改 Runtime、不修改 Core Protocol。

### 5.2 Transport trait（草案）

```rust
pub trait Transport: Send {
    fn spawn_exec(&mut self, program: &str, args: &[&str], pty_size: PtySize) -> Result<()>;
    fn read(&mut self) -> io::Result<Option<Vec<u8>>>;     // 非阻塞
    fn write(&mut self, data: &[u8]) -> io::Result<usize>;
    fn resize(&mut self, cols: u16, rows: u16) -> Result<()>;
    fn kill(&mut self, signal: TransportSignal) -> Result<()>;
    fn try_wait(&mut self) -> io::Result<Option<u32>>;     // Some(code)=exited
    fn shutdown(&mut self) -> Result<()>;
    fn stderr(&self) -> &[u8];
}
```

> 同步接口（内部可 spawn 后台线程做 async→sync 桥接），与 `Backend::execute` 同步签名一致。

### 5.3 LocalProcessTransport

用 `portable-pty` 分配 PTY 对，spawn 子进程。复用现有 `core/tmux/pty.rs` 和 `core/terminal/process.rs` 的模式。

| 关注点 | 实现 |
|--------|------|
| 字节流 | pty master read/write |
| PTY | portable-pty 本地 pty |
| resize | `master.resize()` → SIGWINCH |
| 退出码 | `child.try_wait()` |
| stderr | pty 合并或分离 |

### 5.4 SshProcessTransport

**spawn 系统 `ssh <alias> <command>` 进程到 PTY**，认证/密钥/agent/ProxyJump/known_hosts 全部由系统 ssh 处理。

| 关注点 | 实现 |
|--------|------|
| 字节流 | ssh 进程的 pty master read/write |
| PTY | ssh 进程在本地有 pty（ssh 自己做远端 pty 转发） |
| resize | `master.resize()` → ssh 转发 window-change |
| 退出码 | ssh 进程退出码（远端命令退出码经 ssh 传递） |
| stderr | ssh stderr 用于错误诊断 |

**与现有代码差异**：现有 `RemoteTmuxClient` 用 `async-ssh2-tokio` 库级连接（自管认证）。本设计 v1 改为 spawn 系统 ssh 进程，以完全委托认证。`async-ssh2-tokio` 可保留作为后续无系统 ssh 依赖的选项，但 v1 不用。

### 5.5 系统 ssh alias、PTY/pipe、超时、背压

**SSH alias**：`SshProcessTransport` 只接受 alias（`~/.ssh/config` 的 Host 名），不做 DNS/认证解析。

**PTY/pipe**：local 和 ssh 都用 PTY（tty），因为 tmux -CC 和交互 shell 需要 tty。

**超时**：
- `spawn_exec` 连接超时默认 10s。
- `read` 无超时（非阻塞）。
- CLI 命令整体超时默认 30s。

**背压**：
- `read` 的 `mpsc::Receiver` 有界（256），满了后台读线程阻塞。
- pane 输出缓冲有界（`MAX_PANE_OUTPUT_BYTES = 2MB`）。
- 事件队列有界（`MAX_STATE_EVENTS = 8192`）。
- 单次 `write` 建议 ≤ 64KB。

**关闭**：`shutdown` 关闭写端 → 等退出 → 回收资源。`kill` 发信号（SIGHUP→SIGTERM→SIGKILL）。

---

## 6. Config — 横切配置层

Config 不是 Runtime 或 Transport 的子层，而是横切整个项目的统一配置服务。

### 6.1 Config API（草案）

```rust
pub trait ConfigService {
    fn get(&self, key: &str) -> Option<ConfigValue>;
    fn list(&self, prefix: &str) -> Vec<(String, ConfigValue)>;
    fn set(&mut self, key: &str, value: ConfigValue) -> Result<()>;
    fn reset(&mut self, key: &str) -> Result<()>;
    fn subscribe(&self, callback: ConfigChangeCallback);
}
```

### 6.2 配置文件位置与格式

| 路径 | 说明 |
|------|------|
| `~/.config/muxterm/config.toml` | 用户主配置（Alacritty 风格 TOML） |
| `~/.config/muxterm/themes/<name>.toml` | 用户主题 |
| `configs/themes/<name>.toml` | 内置主题（随包分发） |

格式见 `ARCHITECTURE.md` §3.2 与现有 `core/config.rs`：`[font]`/`[theme]`/`[tmux]`/`[ssh]`/`[scrollback]`/`[ui]`/`[pane]`/`[behavior]`/`[[keybindings]]`。

### 6.3 变更事件

Config 变更时发出 `ConfigChanged { key, old, new }` 事件，供前端即时刷新（主题切换、字号调整等）。

### 6.4 默认值

所有字段有默认值，空配置 = 正常运行。解析失败静默降级（warning）。

### 6.5 UI 配置 vs 核心连接配置的边界

| 类别 | 字段示例 | 谁读 |
|------|---------|------|
| UI 配置 | `font`/`theme`/`ui`/`keybindings` | 前端层 |
| 核心连接配置 | `ssh.host`/`ssh.port`/`tmux.default_session` | Runtime/Transport |
| 行为配置 | `pane.default_command`/`behavior.*` | Runtime |

### 6.6 CLI / FFI 共享

- CLI `settings get/set`（reserved）读写 Config。
- FFI `muxterm_config_get/set`（草案）读写 Config。
- Config 不依赖任何 GUI 框架（现有 `core/config.rs` 的 gtk4 引用需移到 platform 层）。

---

## 7. Discovery — 连接前查询能力

Discovery 是无状态查询，不建立长连接，不画进主运行时层。

### 7.1 查询类型

| 查询 | 接口 | 实现 |
|------|------|------|
| SSH hosts | `list_hosts() → Vec<SshHostEntry>` | 解析 `~/.ssh/config` 的 Host/HostName/Port/User |
| tmux sessions | `list_tmux_sessions(socket, ssh_alias?) → Vec<TmuxSessionInfo>` | `tmux -L list-sessions` 或 `ssh <alias> tmux list-sessions` |
| 目录 | `list_dir(path, ssh_alias?) → Vec<FsEntry>` | `std::fs::read_dir` 或 `ssh <alias> ls` |

### 7.2 SSH hosts 约束

只读取 `~/.ssh/config` 的 **Host alias**，不做 DNS、不连接、不认证。`IdentityFile`/`ProxyJump` 等由系统 ssh 处理，不解析。

### 7.3 与主链关系

```
Discovery（无状态）           Runtime（有状态长连接）
  ssh hosts                   ShellRuntime
  tmux session list           TmuxRuntime
  fs list                       ↓
    ↓                         Transport
  用户选择 target               ↓
    ↓                         Backend → Core Protocol → Frontend
  RuntimeMode::open(spec)
```

Discovery 结果供前端展示/用户选择/CLI 输出。选定 target 后才构造 Runtime + Transport。

---

## 8. CLI v1 语法

默认 JSON。全局参数 `-s`/`-L`/`-w`/`-t`/`-p`/`--remote`/`--format`/`-v`。

### 8.1 命令树

```
muxterm
├── ssh hosts                         # 列出 ~/.ssh/config Host 别名
├── shell open [-s NAME] [--remote ALIAS]  # 创建/连接 shell session
├── tmux
│   ├── session list [-L SOCKET] [--remote ALIAS]
│   ├── session new  -s NAME [-L SOCKET] [--remote ALIAS]
│   └── session attach -s NAME [-L SOCKET] [--remote ALIAS]
├── fs list <PATH> [--remote ALIAS]
├── settings [get|set]                # reserved
├── (legacy flat commands)            # 兼容现有语法
│   new-session / list-sessions / kill-session / attach-session / detach
│   new-window / list-windows / select-window / rename-window / kill-window
│   new-tab / list-tabs / select-tab / rename-tab / kill-tab
│   split-pane / list-panes / select-pane / resize-pane / kill-pane
│   send-keys / write-raw / capture-pane / display-message
│   list-layout / dump-state / watch
└── watch [-s NAME]                   # NDJSON 事件流
```

### 8.2 输出格式

- 默认 JSON（单行）；`--format text` 可选。
- `capture-pane`/`display-message` 输出原始文本。
- `watch` 输出 NDJSON（每行一个事件，`data` 为 base64）。
- `dump-state` 始终 JSON。
- 错误：stderr `错误: <message>`，exit code 1。

### 8.3 ID 使用

所有 CLI 输出/参数使用 muxterm ID（`-p 1` 而非 `-t @1`）。旧 `-t @1` 格式过渡期保留兼容。

---

## 9. FFI ABI 草案

### 9.1 Opaque Handle

```c
struct MuxtermHandle;  // opaque
struct MuxtermHandle* muxterm_open(const struct MuxtermOpenSpec* spec);
void muxterm_free(struct MuxtermHandle* h);
```

### 9.2 Open Spec

```c
struct MuxtermOpenSpec {
    uint32_t api_version;    // 1
    uint32_t mode;           // 0=LocalShell, 1=LocalTmux, 2=SshShell, 3=SshTmux
    const char* session_name;
    const char* tmux_socket;
    const char* ssh_alias;
};
```

### 9.3 Snapshot / Poll / Execute / Send

```c
int muxterm_connect(struct MuxtermHandle* h);
int muxterm_shutdown(struct MuxtermHandle* h);
int muxterm_execute(struct MuxtermHandle* h, const struct CTask* task);
int muxterm_send_input(struct MuxtermHandle* h, uint32_t pane_id, const uint8_t* data, size_t len);
int muxterm_resize_pane(struct MuxtermHandle* h, uint32_t pane_id, uint16_t cols, uint16_t rows);
int muxterm_poll_events(struct MuxtermHandle* h, struct CStateChange* out, int max_count);
int muxterm_get_sessions(struct MuxtermHandle* h, struct CSession* out, int max_count);
int muxterm_get_windows(struct MuxtermHandle* h, struct CWindow* out, int max_count);
int muxterm_get_tabs(struct MuxtermHandle* h, struct CTab* out, int max_count);
int muxterm_get_panes(struct MuxtermHandle* h, uint32_t tab_id, struct CPane* out, int max_count);
int muxterm_get_layout(struct MuxtermHandle* h, uint32_t tab_id, struct CLayoutNode* out);
int muxterm_get_pane_output(struct MuxtermHandle* h, uint32_t pane_id, uint8_t* buf, size_t buf_len);
int muxterm_get_pane_output_len(struct MuxtermHandle* h, uint32_t pane_id);
uint32_t muxterm_get_status(struct MuxtermHandle* h);
const char* muxterm_last_error(struct MuxtermHandle* h);
```

### 9.4 Discovery（不需要 handle）

```c
int muxterm_discover_ssh_hosts(struct CSshHost* out, int max_count);
int muxterm_discover_tmux_sessions(const char* socket, const char* ssh_alias, struct CTmuxSession* out, int max_count);
int muxterm_discover_fs_list(const char* path, const char* ssh_alias, struct CFsEntry* out, int max_count);
```

### 9.5 内存所有权（核心约束）

**原则：不长期暴露 Rust 裸指针给平台层。**

| 数据 | 所有权 | 有效期 | 调用方 |
|------|--------|--------|--------|
| `MuxtermHandle*` | owned by C | open→free | free 一次 |
| `CStateChange.data/name` | **borrowed** from handle | 下次 poll 前 | 立即 memcpy/strdup |
| `CTab.name`/`CSession.name`/`CPane.title` | **borrowed** from handle | 下次同类 get 前 | 立即 strdup |
| `CLayoutNode` 子节点 | **borrowed** from handle | 下次 get_layout 前 | 深拷贝整棵树 |
| `get_pane_output` buf | **owned by C** | 调用方管理 | C 提供缓冲，Rust copy |
| `muxterm_last_error` | **borrowed** from handle | 下次任意调用前 | 立即 strdup |
| Discovery 字符串 | **borrowed** from static | 下次同类调用前 | 立即 strdup |

**禁止**：平台层把 borrowed 指针存到 struct 长期持有；`free()` borrowed 指针；跨线程共享 borrowed 指针。

### 9.6 v1 稳定 vs 未定

**v1 稳定**：`muxterm_open`/`free`/`connect`/`shutdown`/`execute`/`send_input`/`resize_pane`/`poll_events`/`get_tabs`/`get_panes`/`get_layout`/`get_pane_output`。

**未定**：`muxterm_send_keys`/`get_sessions`/`get_windows`/`get_pane_title`/`get_pane_output_len`/`get_status`/`last_error`/`discover_*`/`config_get/set`。

---

## 10. TUI / Linux GTK / macOS SwiftUI 共同边界

### 10.1 共用核心

所有平台共用 `core/` 纯逻辑层 + FFI C ABI。前端不直接调用 Runtime/Transport，只经 Core Protocol（TerminalModel/Backend/State/Task）或 FFI。

### 10.2 平台桥接

| 平台 | 桥接 | 语言 | 轮询机制 |
|------|------|------|---------|
| Linux GTK | `platform/linux/ffi_bridge.rs` | Rust | glib timeout 16ms |
| TUI | `platform/tui/ffi_bridge.rs` | Rust | 事件循环 poll |
| macOS | `CoreBridge.swift` | Swift | Timer/DispatchSource |

三端桥接同构：`poll_events` → 拷贝 owned 数据 → 渲染；`snapshot()` 拉完整快照。

### 10.3 必须一致的行为

遵循 `ARCHITECTURE.md` §2：窗口/tab/pane 生命周期、嵌套分割、焦点管理、进程名更新、快捷键、配置格式、主题格式。

### 10.4 必须处理的 StateChange 事件集

三端不能遗漏：`PaneOutput`/`LayoutChanged`/`Tab*`/`Pane*`/`Active*Changed`/`PaneResized`/`BackendStatusChanged`。

---

## 11. 测试契约

### 11.1 测试矩阵

| 类别 | 内容 | 依赖 | 超时 |
|------|------|------|------|
| 协议单元 | %消息解析、命令构造、ID 映射 | 无 | 2s |
| Transport | LocalProcessTransport spawn/read/write/kill | `true`/`sleep` | 5s |
| Transport (SSH) | SshProcessTransport spawn `ssh <alias>` | SSH host（免密） | 15s |
| ShellRuntime | local-shell split/close/switch/send/capture | `cat`/`sleep` | 10s |
| TmuxRuntime | local-tmux attach + %消息 → StateChange | tmux | 15s |
| ssh-shell | SshProcessTransport + ShellRuntime 端到端 | SSH host | 30s |
| ssh-tmux | SshProcessTransport + TmuxRuntime 端到端 | SSH host + tmux | 30s |
| CLI | parse + format（JSON/text） | 无 | 2s |
| CLI daemon | new-session + list + send + kill | 无 | 15s |
| CLI tmux | session list/new/attach + split + capture | tmux | 15s |
| FFI | open/connect/execute/poll/free（local-shell） | 无 | 10s |
| FFI (tmux) | open LocalTmux + get_tabs/panes/layout | tmux | 15s |
| TUI | muxterm --tui + 宿主 tmux capture | tmux + 编译 | 30s |
| GTK | UI | DISPLAY/xvfb | 30s |
| macOS | CoreBridge FFI + tmux attach | tmux | 30s |
| E2E | 真实二进制 + tmux -L | 编译 | 30s |

### 11.2 硬超时

所有可能卡住的测试必须有硬超时。纯单元 2s；本地进程 5-10s；tmux 15s；SSH 30s；UI 30s。

### 11.3 SSH 测试

默认 `#[ignore]`（不跑 CI），手动 `cargo test -- --ignored`。或 Docker sshd 容器（CI 可选）。Transport 层可 mock（`MockTransport`）测 Runtime 不需真 SSH。

---

## 12. 设计检查清单

| # | 检查项 | 通过条件 |
|---|--------|---------|
| 1 | 四种模式组合明确 | local-shell/local-tmux/ssh-shell/ssh-tmux = 2 Transport × 2 Runtime |
| 2 | 未来 Runtime 扩展不修改 Transport | 新增 ZellijRuntime 只实现 `Backend`，不改 `Transport` trait |
| 3 | 未来 Transport 扩展不修改 Core Protocol | 新增 Transport 只实现 `Transport` trait，不改 Task/StateChange/Snapshot |
| 4 | Config 可从 CLI/FFI/平台读写 | Config API get/list/set/reset + 变更事件，不依赖 GUI |
| 5 | tmux IDs 不越过 Runtime 边界 | `$N`/`@N`/`%N` 只在 TmuxRuntime adapter 内，StateChange/Snapshot 用 muxterm ID |
| 6 | SSH 认证委托系统 ssh | SshProcessTransport spawn `ssh <alias>`，不自管认证 |
| 7 | 主链不混层 | Frontend→Core Protocol→Runtime→Transport，Discovery 不在主链 |
| 8 | FFI 不长期暴露裸指针 | borrowed 指针下次调用前有效；owned 由 C 管理 |
| 9 | CLI 默认 JSON | `--format json` 默认；text 可选 |
| 10 | Window 是前端一级对象 | 协议定义为一级对象，v1 固定 1 个，可前端新建（v2+） |

---

## 13. 分阶段实施顺序

| 阶段 | 交付物 | 风险 |
|------|--------|------|
| 0 | 本文档 + `PROJECT-STRUCTURE.md` | 无 |
| 1 | Transport 抽象（local）；LocalBackend→ShellRuntime+LocalTransport；TmuxBackend→TmuxRuntime+LocalTransport | 破坏现有测试 |
| 2 | ID 隔离强化；StateChange/Snapshot 清除 tmux ID；CLI 改 muxterm ID | CLI 语法 breaking |
| 3 | SshProcessTransport（spawn 系统 ssh）；~/.ssh/config 解析；CLI ssh/fs/tmux --remote | 系统 ssh 兼容 |
| 4 | SSH Runtime（ssh-shell/ssh-tmux）；RuntimeMode 工厂 | 远端 pty 差异 |
| 5 | FFI 扩展（OpenSpec/mode/CPane.title/sessions/windows/discovery/last_error） | ABI breaking |
| 6 | CLI v1 新命令（ssh hosts/shell open/tmux session/fs/watch）；format.rs→serde_json | 脚本兼容 |
| 7 | daemon 优化（watch 长连接/增量同步） | 丢事件 |
| 8 | 平台前端适配（三端 ffi_bridge + macOS muxterm.h 同步） | 三端不同步 |

---

## 14. 未决问题

| # | 问题 | 选项 | 倾向 |
|---|------|------|------|
| 1 | SshProcessTransport spawn 系统 ssh vs 库级 | A: 系统 ssh B: 库级 | A（委托认证） |
| 2 | ShellRuntime 多 pane 各自 Transport | A: 是 B: 单 Transport mux | A |
| 3 | seq 序列号 v1 实现 | A: 是 B: v2 | B |
| 4 | CLI 旧 `-t @1` 兼容期 | A: v1 保留 B: 立即废弃 | A |
| 5 | format.rs 迁移 serde_json 时机 | A: 阶段 6 B: 独立 | A |
| 6 | muxterm.h 是否 cbindgen 自动生成 | A: 手写 B: cbindgen | A 现状 / B 长期 |
| 7 | DaemonBackend 是否重构为 Transport+Runtime | A: 是 B: 保留独立 | B（IPC client） |
| 8 | 远程 fs list 用 exec ls vs sftp | A: ls B: sftp | A |
| 9 | SSH 测试用 Docker sshd 容器 | A: 是 B: #[ignore] | A+B |
| 10 | Transport::read 同步 vs 异步 | A: 同步 B: 异步 | A |
| 11 | 多 Window 何时支持 | A: v2 B: 后续 | B |
| 12 | Config 变更事件机制 | A: callback B: channel | 未定 |

---

## 15. v1 稳定 vs 未承诺

### v1 稳定（不 breaking）

- Core Protocol 层级（Session/Window/Tab/Pane）
- ID 规则（`s{name}`/`wN`/`tN`/`pN`）
- Task enum 18 变体语义
- StateChange enum 17 变体语义
- Snapshot JSON 结构（新增允许）
- FFI v1 函数集（§9.6）
- `#[repr(C)]` v1 结构体布局（末尾新增允许）
- CLI 现有 flat 命令（废弃标注，不删除）
- `~/.config/muxterm/config.toml` 格式（新增允许）

### 未承诺（v1 内可能变更）

- `MuxtermOpenSpec`/`muxterm_open` 字段
- `muxterm_discover_*` 签名
- `muxterm_send_keys` 格式
- `muxterm_last_error` 错误码体系
- `RuntimeMode` 变体名
- `Transport` trait 方法签名
- `SshProcessTransport` 实现方式
- `seq` 序列号
- `--watch` 协议细节
- daemon 增量同步策略
- `fs list --remote` 实现
- Config API 细节

---

## 附录 A：现有代码与设计对照

| 设计概念 | 现有代码 | 重构方向 |
|---------|---------|---------|
| `Transport` trait | `core/tmux/pty.rs` + `core/backend/local.rs` | 提取为 `core/transport/` |
| `LocalProcessTransport` | `LocalBackend` 内 portable-pty | 提取 |
| `SshProcessTransport` | `core/ssh/client.rs`（库级） | 改为 spawn 系统 ssh |
| `ShellRuntime` | `LocalBackend` | 重命名 + 接受 Transport |
| `TmuxRuntime` | `TmuxBackend` | 重命名 + 接受 Transport + ID 映射强化 |
| `ConfigService` | `core/config.rs` | 加 API + 移除 gtk4 依赖 |
| `SshHostDiscovery` | 无 | 新增 `core/discovery/` |
| `TmuxSessionDiscovery` | `main.rs::find_existing_tmux_session` | 提取 |
| `RuntimeMode` | `main.rs::cli_mode` match | 提取为 enum + 工厂 |

## 附录 B：常量清单

```
RuntimeMode:  LOCAL_SHELL=0, LOCAL_TMUX=1, SSH_SHELL=2, SSH_TMUX=3
BackendStatus: Disconnected=0, Connecting=1, Connected=2, Error=3, Exited=4
StateChange.type_: PANE_OUTPUT=0, TAB_ADDED=1, TAB_CLOSED=2, LAYOUT_CHANGED=3,
  PANE_ADDED=4, PANE_CLOSED=5, ACTIVE_TAB_CHANGED=6, ACTIVE_PANE_CHANGED=7,
  TAB_RENAMED=8, PANE_RESIZED=9, BACKEND_STATUS=10, OTHER=99
Task.type_: SPLIT_PANE=0..SHUTDOWN=7, SWITCH_PANE=8,
  (新增) NEW_WINDOW=9..RENAME_SESSION=15, RESIZE_PANE_STEP=16
SplitDir: HORIZONTAL=0, VERTICAL=1
LayoutNode: LEAF=0, SPLIT_H=1, SPLIT_V=2
TransportSignal: HANGUP=0, TERM=1, KILL=2
```

---

> **本文档为架构设计，不含代码实现。主链：Frontend → Core Protocol → Runtime → Transport。**
