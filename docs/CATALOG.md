# CATALOG.md — Catalog 是「能打开什么」的目录

产品树：[`WORKSPACE.md`](WORKSPACE.md)。Runtime：[`RUNTIME.md`](RUNTIME.md)。
像素：[`SURFACE.md`](SURFACE.md)。
Herdr identity：[`HERDR-RUNTIME-STABILITY.md`](HERDR-RUNTIME-STABILITY.md) §7。

**一句话：** Catalog 回答三个问题：有哪些 Runtime / Transport 可选；某个 target 上现在有哪些
可 attach 的东西；用户选中的 Candidate 应解析成什么 `WorkspaceSpec`。它**不拥有** Workspace、
连接、Runtime 实例，也不是 app 的总门面。

QuickConnect / macOS / CLI 都读这一份。不要在 frontend 里长第二套发现或连接池。

---

## 1. 词

| 词 | 是什么 | 不是什么 |
|---|---|---|
| **Muxterm** | 组合根。FFI handle 指向它 | Catalog |
| **Catalog** | providers 只读视图 + Existing inventory + resolver | Pool；连接表 |
| **RuntimeProvider** | 注册表里的 Runtime 插件 | 已经 attach 的 `trait Runtime` |
| **TransportProvider** | local / ssh 插件 | 一条 ByteChannel |
| **TargetConnection** | 可复用连接；`ConnectionRegistry` 持有 | Workspace |
| **Inventory** | **尚未 attach** 的 Existing 台账（探活、灯、request generation） | Pool 里已打开格子的状态 |
| **Candidate** | QuickConnect 列表项：Project / Worktree / Existing / Recent | `WorkspaceSpec` |
| **OpenRequest** | 用户选择 → Core 的输入 | spec |
| **WorkspaceSpec** | `Catalog::resolve` 的输出；Core 内部 | frontend 可构造的结构 |
| **target / connect name** | 怎么到对端：本机 `local` 或 SSH Host alias | 可 attach 的格子 |
| **session**（发现层） | 可 attach 的格子：tmux session 名、Herdr workspace | 产品类型 Session（已删） |

### 1.1 两个曾经都叫 Transport 的东西

| 层 | 名字 | 职责 |
|---|---|---|
| provider | `TransportProvider` | 列出 target，拿出可复用 `TargetConnection` |
| 字节 | `ByteChannel` | 已经存在的 read/write/resize |

不要发明 `TransportDriver`。不要再让两个 trait 都叫 `Transport`。

### 1.2 三个都叫 `local` 的东西

| 词 | id / 名字 | 怎么列 |
|---|---|---|
| Local **Transport** | `"local"` | `discover_targets("local")` → 单例，target id 是 `""` |
| SSH **Host alias** | 用户 `~/.ssh/config` 的 `Host` 名，**可以就叫 `local`** | `discover_targets("ssh")` 含 `id == "local"` |
| Runtime **插件** | `"tmux"` / `"herdr"` / `"shell"` | `runtime_list()`，不是 SSH host 表 |

`discover_sessions("local", "")` 是本机。`discover_sessions("ssh", "local")` 是 SSH Host 名叫
`local` 的那台。两者禁止串。

### 1.3 列出 SSH 不是 attach SSH

attach 需要远端 pty：`ssh -tt -o ConnectTimeout=10`。

列出 / 探活是短命令：`ssh -o BatchMode=yes -o ConnectTimeout=2`，**不要 `-tt`**。
`-tt` 会灌 MOTD / `\r` / 提示符，`list-sessions` 解析成空，面板就把有 session 的 host 丢掉。

测试用隔离 tmux：`MUXTERM_TEST_LOCAL_TMUX_SOCKET` 与 `MUXTERM_TEST_REMOTE_TMUX_SOCKET`，
对标 `HERDR_SOCKET_PATH`。生产不设 = 默认 server。禁止测用户默认 `tmux` / `herdr.sock`。

### 1.4 connect name 和 runtime list

产品里并列的「机器」叫 **connect name**：本机 `local` + 每个 SSH Host alias。
这是 `TargetConnection` 的身份，**不是** `transport_list()` 的插件 id（插件只有 `local` | `ssh`）。

同一台机器既是 local 又被 SSH 指回来时，**同一 session 出现两行**：`tmux @ local` 和
`tmux @ self`。这是要的，不是去重 bug。

用户说的「runtime list」= Existing Candidate 行。`runtime_list()` 仍是新建卡片用的 provider 视图。

---

## 2. 为什么要这一层

没有 Catalog 时，QuickConnect、CLI、macOS 各自发现、各自拼 spec、各自 fallback。
有 Catalog 之后：发现、探活、兼容性检查、结构化错误只有一处。

Catalog **不是**「什么都往里塞」的单例。组合关系：

```text
Muxterm
  ├── Catalog          只读索引 + 解析
  │     ├── providers  runtime_list / transport_list
  │     ├── inventory  Existing：discover + 探活 + generation
  │     └── resolver   candidates() / resolve(OpenRequest) → WorkspaceSpec
  ├── Projects
  ├── ConnectionRegistry
  ├── WorkspacePool
  └── Activity
```

若最后 Catalog 只剩 `resolver.rs` 一个文件，就并入 `workspace/`，不为一个名字保留目录。

---

## 3. Catalog 保留 / 不保留

保留：

- providers 只读视图（registry 本体在 `runtime/registry.rs`、`transport/registry.rs`）
- Existing inventory（发现 + 探活 + request generation：旧 completion 不能覆盖新 target）
- Candidate 列表聚合（Project / Worktree / Existing / Recent 四类一张表）
- `resolve(OpenRequest) → WorkspaceSpec`，含 `channel_requirements ⊆ supported_channels`

不保留：

- `catalog/builtin/` 里的 concrete Runtime 构造器
- Runtime protocol parser
- Transport 的具体实现
- 连接复用表（→ `ConnectionRegistry`）
- WorkspacePool 与 open/close/activate（→ Muxterm / Pool）
- Project/Worktree 持久化
- Pane / Surface / TerminalState

---

## 4. 解析

```text
Frontend  open(OpenRequest)
    ▼
Muxterm::open
    ├── Catalog::resolve → WorkspaceSpec
    └── WorkspacePool::open(spec)
          ├── ConnectionRegistry::acquire(target)
          ├── RuntimeProvider::new_instance(conn, spec)
          ├── Workspace::new → runtime.connect()
          ├── create 时 apply WorkspaceTemplate
          └── PoolChanged（+ 可选 activate）
```

以下情况必须返回结构化 `ResolveError`，不能静默 fallback：

- RuntimeId / TransportId 未注册
- `channel_requirements` 不是 `supported_channels` 的子集
- target connection 失败
- Runtime open 失败
- Project / Worktree 目录不存在或身份不完整

尤其禁止把 SSH target 的失败组合静默降级为本地 ShellRuntime。

`AttachOnly` 是普通重连和旧配置迁移。只有用户本次明确新建才允许 `CreateIfMissing`。

Workspace 显示名取 Candidate / Project 的用户可见名称，不能把 Herdr named session 误当 Project 名。
identity key 包含 transport target、runtime、session、target-side socket 与 workspace_id；
Project name/path 只是显示/项目元数据，不是 attach identity。

---

## 5. 注册

```text
Muxterm::new(config_store)
  → runtime::registry::with_builtins()      // shell / tmux / herdr 自注册
  → transport::registry::with_builtins()    // local / ssh 自注册
  → ConnectionRegistry::new()
  → Catalog::new(&runtime_registry, &transport_registry)
  → Projects / WorkspacePool / Activity
```

具体 provider 的代码自己位于 `runtime/<name>/`、`transport/<name>/`。
如果未来改为动态插件，registry interface 不需要改变。

daemon 不注册为 provider，不出现在用户可选择的 runtime 列表。

---

## 6. 前端怎么用

页面与 Scene 见 [`FRONTEND.md`](FRONTEND.md)。Catalog 侧：

- 新建卡片：`runtime_list()` / `transport_list()` + `support()` 决定次级动作
- 一级列表：`candidates()`（Project / Worktree / Existing / Recent）
- 已有的连接：Existing 扁平列表，不要本地 / SSH / Host 多层目录
- 选择：产出 `OpenRequest`，不要拼 `WorkspaceSpec`
- 徽章：runtime_name / transport_name / connect name。不要 `if herdr`

Discovery JSON 形状（产品）：

```json
{ "workspaces": [
    { "id": "local/local/tmux/yaklang-workspace",
      "name": "yaklang-workspace",
      "runtime": "tmux",
      "transport": "local",
      "target": "local",
      "in_pool": false }
]}
```

`transport` = 插件 id。`target` = connect name。不要 `{ "sessions": [ { "id": "$4" } ] }`。

---

## 7. 明确不做

- Catalog 持有 Pool 或 ConnectionRegistry
- frontend / FFI 把 `WorkspaceSpec` 当产品输入
- Driver 这个名字（改称 RuntimeProvider）
- `TransportDriver`
- 在 `window.rs` 里探活
- 测用户默认 tmux / herdr.sock
