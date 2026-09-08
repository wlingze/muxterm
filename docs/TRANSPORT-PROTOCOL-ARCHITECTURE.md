# Muxterm Transport 与协议分层

> 契约冻结：2026-09-08（`2026-09-08T15:02:22+08:00`，Asia/Shanghai）
> **本文不再是 2026-07-28 的四模式施工单。** 旧 `RuntimeMode { LocalShell, LocalTmux, SshShell, SshTmux }`、
> Session/Window 协议、`muxterm_open(mode)` 全部作废。
> 产品树：[`WORKSPACE.md`](WORKSPACE.md)。Runtime：[`RUNTIME.md`](RUNTIME.md)。
> Catalog：[`CATALOG.md`](CATALOG.md)。Config：[`CONFIG.md`](CONFIG.md)。

**一句话：** 主链是 **Frontend（ffi_client）→ Muxterm 组合根 → Workspace → Runtime → ByteChannel**。
Runtime 与 Transport 独立组合。SSH 只提供连接和字节流。

---

## 1. 目标与非目标

### 1.1 目标

1. Frontend 只经 C FFI + 统一 `ffi_client`。Core 是库。
2. 产品语义稳定：Workspace → Tab → Pane。Window 只表示 GUI；tmux `$N` 不出 Runtime。
3. Runtime 可扩展：新增 provider 自注册，不改 Transport，不改复合 enum。
4. Transport 可扩展：声明 `supported_channels`，所有 Runtime 自动可组合。
5. **SSH 是 Transport，不是 Runtime**：认证/密钥/agent/ProxyJump/known_hosts/密码交互全部委托系统 `ssh <alias>`。
6. Config 横切，但 **runtime / transport 不读 config.toml**。
7. Discovery 是连接前查询：列出 SSH host、tmux session、Herdr workspace。探活进 Inventory，不建立产品 Workspace。
8. CLI 默认 JSON；text 可选。无 subcommand 即 CLI。
9. FFI：opaque handle + owned snapshot/event copy；borrowed 指针只在单次调用内有效。

### 1.2 非目标

- 不实现自有 SSH 认证协议。
- 不实现自有终端模拟器作为 Core 显示缓存（渲染由 frontend Surface 负责；Index 只供搜索/attention）。
- 不把四种组合写成 `RuntimeMode`。
- 不把 daemon 注册成第四种 Runtime。
- 不在 frontend 做连接池。

---

## 2. Transport 三层

当前代码里曾经有两个都叫 `Transport` 的 trait。目标明确拆开：

```text
TransportProvider
  └── TargetConnection（可复用；ConnectionRegistry 持有）
        └── ByteChannel × N
              └── Runtime instance
```

| Runtime | ByteChannel |
|---|---|
| tmux | 1 条 Exec PTY（`tmux -CC`） |
| shell | 每个 pane 1 条 Exec PTY |
| herdr | 1 条 UnixSocket，同一 server 共享 |

### 2.1 TransportProvider

- `list_targets()`
- `supported_channels() -> &'static [ChannelKind]`
- `connect(target) -> Arc<dyn TargetConnection>`
- 探活进 Inventory，不进 GUI

### 2.2 TargetConnection

```text
open_channel(ChannelRequest) -> ByteChannel
probe()
target() -> TargetId
```

`ChannelRequest`：

- `Exec { argv, cwd, env, pty }`
- `UnixSocket { path }`（local 直连；ssh 侧转发）

列出 SSH 不要 `-tt`。attach 才要 pty。

### 2.3 ByteChannel

read / write / resize / shutdown / EOF。Runtime 只依赖这一层，不根据字符串判断 SSH。

### 2.4 ConnectionRegistry

`TargetId → Arc<dyn TargetConnection>` + 最近使用时间。由 Muxterm 持有。
不叫 pool（避免与 WorkspacePool 混淆）。同一 SSH host / 同一 Herdr socket 一份 `Arc`。

兼容性：

```text
compatible(runtime, transport) :=
    runtime.channel_requirements() ⊆ transport.supported_channels()
```

local：`Exec` + `UnixSocket`。ssh：远端 `Exec` + socket 转发。

---

## 3. 协议边界

`protocol` 是跨边界必须稳定的数据契约，不是新的 model 仓库：

- ids、Candidate / OpenRequest、events（三 lane）、task、snapshot、error envelope、FFI

协议类型不能包含：`TmuxRuntime`、tmux `$N`、Herdr wire frame、GTK/VTE/SwiftTerm、Core `TerminalState` 的可变引用。

打开输入三分：Candidate → OpenRequest → WorkspaceSpec。frontend 永不构造 WorkspaceSpec。

---

## 4. 仍有效的工程选择

这些从 2026-07-28 文稿保留：

- 系统 `ssh <alias>`，不自研 SSH。
- Transport 是字节管道，不懂 Tab/Pane。
- tmux 内部 ID 停在 adapter。
- Config 不是 Runtime 的子层（细节 [`CONFIG.md`](CONFIG.md)）。
- FFI 不把 Core 内部 buffer 借给 frontend 长期持有；poll 时复制 bytes。

## 5. 明确作废

- `RuntimeMode` 0=LocalShell … 3=SshTmux
- `muxterm_get_sessions` / `muxterm_get_windows` 作为产品 API
- Core Protocol 的 Session/Window 层级
- `Backend` / `TerminalModel` 作为数据仓库
- DaemonBackend 作为用户可选 Runtime
- `src/platform` 直接 `use` Core 内部模块
