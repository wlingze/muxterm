# Muxterm

产品结构：[`docs/WORKSPACE.md`](docs/WORKSPACE.md)。Runtime：[`docs/RUNTIME.md`](docs/RUNTIME.md)。
像素：[`docs/SURFACE.md`](docs/SURFACE.md)。前端：[`docs/FRONTEND.md`](docs/FRONTEND.md)。
配置：[`docs/CONFIG.md`](docs/CONFIG.md)。施工：[`TASKS.md`](TASKS.md)。

## 一句话

Muxterm 是跨平台终端：**Workspace = Runtime(Transport) + path**。把 tmux / Herdr / 本地 shell 收成同一套原生 Tab / Pane UI，而不是黑框里按 `Ctrl+B`。体验对齐 iTerm2 的 tmux 集成，但产品层级是 Muxterm 自己的，不对某一个 Runtime 特化。

## 产品树

```text
Frontend（cli / tui / linux / macos / windows）
  └── C FFI + 统一 ffi_client
        └── Muxterm（组合根：一个产品会话 = 一个 FFI handle）
              ├── Config          唯一事实源 config.toml
              ├── Catalog         能打开什么（providers 视图 + inventory + resolver）
              ├── Projects        Project / Worktree；引用 WorkspaceTemplate
              ├── Activity        commands / agents / attention
              ├── ConnectionRegistry
              └── WorkspacePool
                    └── Workspace = Runtime(Transport) + path
                          └── Tab → Pane
```

GUI **Window 不是产品树节点**，只是某个 Workspace 的体现。产品层没有 Session，也没有虚拟 Window。

## 平台

| 前端 | 形态 |
|------|------|
| Linux | GTK4 + vte4，`frontend/linux` |
| macOS | Swift + SwiftTerm，`frontend/macos` |
| TUI | ratatui，`muxterm tui` |
| CLI | 无 Scene；无 subcommand 即 CLI |
| Windows | 占位 `frontend/windows` |

唯一 binary 是薄 `src/main.rs`（禁止 `mod` 声明）：无 subcommand = CLI，`muxterm tui`，`muxterm gui`（linux→GTK4，macos→macOS 前端）。

## 核心概念

| 词 | 是什么 |
|---|---|
| **Workspace** | 可连接实例。`Runtime` 套在 `Transport` 上，外加 cwd 锚点 `path`。复用与切换发生在 WorkspacePool。 |
| **Tab / Pane** | Workspace 内部拓扑。所有 Runtime 都必须给出同一套，包括没有 tmux 的 shell。 |
| **Runtime** | `shell` / `tmux` / `herdr`。对外是统一 `Runtime` trait；具体类型隐藏。shell 是默认基础 runtime。 |
| **Transport** | `local` / `ssh`。与 Runtime 独立组合，不写 `LocalTmux` 这类复合 mode。 |
| **Project** | 持久化项目管理：名字、目标环境、主目录、默认 Runtime / Transport、默认模板。 |
| **Worktree** | Project 下的 git checkout。打开后是池里独立的一格 Workspace，带 `provenance`。 |
| **WorkspaceTemplate** | 原 `twork`：tab 数量、名字、布局、每格命令 / cwd。只在新建会话时应用。 |
| **Candidate / OpenRequest / WorkspaceSpec** | 列表项 → 用户选择 → resolver 输出。前端永不构造 `WorkspaceSpec`。 |
| **三条 lane** | Control（拓扑）/ Render-data（原始字节）/ Activity（命令、agent、attention）。 |

## 打开与切换

QuickConnect / 命令面板列出 **Candidate**（Project / Worktree / Existing / Recent）。用户选中产生 **OpenRequest**。只有 Catalog resolver 能产出 **WorkspaceSpec**，再由 WorkspacePool 打开。

切换已打开的 Workspace / Tab：**点击路径零 Core 调用、零锁等待**。每个已打开 Workspace 一棵常驻 Scene，每 pane 一个常驻 Surface。切换 = 换可见场景。

前端没有 warm/cold slot、`bridgeLock`、串行后台队列或前台校准。页面与 Scene 见
[`docs/FRONTEND.md`](docs/FRONTEND.md)。像素定律见 [`docs/SURFACE.md`](docs/SURFACE.md) §3。
切换延迟不是 tmux 全局锁，见同文档 §9。

## 能力与问询

GUI 问 Runtime 能力只用 `support()`，禁止 `if runtime == "herdr"`。Worktree 的 native 能力（Herdr `worktree.*`）与 generic 策略（Projects service 在 TargetConnection 上跑 git）对 UI 是同一入口。

## 明确不做

- 产品层 Session / 虚拟 Window
- frontend 直接引用 Core 内部类型、Runtime concrete type、WorkspacePool
- 复合 `RuntimeMode`（`LocalShell` / `SshTmux` …）
- 把 Index 网格再编码成 ANSI 灌进 Surface（`visible_ansi` 不是 live API）
- 对用户默认 tmux server 执行 `kill-server` / `kill-session`
- 无名字的 `herdr server stop`
- 本轮实现 Windows 前端（只占位）

## 文档地图

| 文档 | 内容 |
|------|------|
| [`docs/WORKSPACE.md`](docs/WORKSPACE.md) | 产品树、打开路径、FFI/CLI |
| [`docs/RUNTIME.md`](docs/RUNTIME.md) | Runtime × Transport、provider、能力、适配表 |
| [`docs/SURFACE.md`](docs/SURFACE.md) | 单面、像素定律 |
| [`docs/FRONTEND.md`](docs/FRONTEND.md) | Scene / EventPump / 页面 / 各平台落地 |
| [`docs/CATALOG.md`](docs/CATALOG.md) | 能打开什么；不是组合根 |
| [`docs/CONFIG.md`](docs/CONFIG.md) | `config.toml` 契约 |
| [`docs/PROJECT-STRUCTURE.md`](docs/PROJECT-STRUCTURE.md) | 目录与 crate 分层 |
| [`TASKS.md`](TASKS.md) | 施工顺序 |
| [`AGENTS.md`](AGENTS.md) | coding agent 约定 |
