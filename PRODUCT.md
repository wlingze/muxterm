# Muxterm

产品结构：[`docs/WORKSPACE.md`](docs/WORKSPACE.md)。Runtime：[`docs/RUNTIME.md`](docs/RUNTIME.md)。
像素：[`docs/SURFACE.md`](docs/SURFACE.md)。前端：[`docs/FRONTEND.md`](docs/FRONTEND.md)。
配置：[`docs/CONFIG.md`](docs/CONFIG.md)。施工：[`TASKS.md`](TASKS.md)。

## 一句话

Muxterm 是日常使用的跨平台原生终端，原生集成 tmux / Herdr，并可按 Project 组织本地和远程工作。
**Workspace = Runtime(Transport) + path**，以一致的 Tab / Pane UI 连接和操作已有工作现场。
默认体验是普通 shell；Project、复用器和阅读增强按需使用。

## 当前发展方向（2026-10）

以日用终端的可靠性为主线，完成独立编辑框、批量复制/输出阅读、图片投递与呈现、
远程文件点击打开；端口转发按实际需要补齐，随后持续维护兼容性、发布与外部用户体验。
ACP 优先使用现成客户端，本轮不接入；图片预览和外部文件打开不扩张成完整 IDE 或文件管理器。
Agent 按轮折叠作为可选只读探索，不把通用终端建立在画面猜测或全 agent 适配之上。

当前取舍、验收与竞品核对见 [`docs/PRODUCT-DIRECTION.md`](docs/PRODUCT-DIRECTION.md)。
旧战略报告和旧 1.0 清单中的方向判断若冲突，以该文为准；下述产品树与架构契约保持有效。

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
| [`docs/PRODUCT-DIRECTION.md`](docs/PRODUCT-DIRECTION.md) | 当前定位、ACP 边界、日用功能收口与稳定期 |
| [`docs/OUTPUT-READING.md`](docs/OUTPUT-READING.md) | 批量复制、命令折叠与只读 agent 阅读的可行性 |
| [`docs/WORKSPACE.md`](docs/WORKSPACE.md) | 产品树、打开路径、FFI/CLI |
| [`docs/RUNTIME.md`](docs/RUNTIME.md) | Runtime × Transport、provider、能力、适配表 |
| [`docs/SURFACE.md`](docs/SURFACE.md) | 单面、像素定律 |
| [`docs/FRONTEND.md`](docs/FRONTEND.md) | Scene / EventPump / 页面 / 各平台落地 |
| [`docs/CATALOG.md`](docs/CATALOG.md) | 能打开什么；不是组合根 |
| [`docs/CONFIG.md`](docs/CONFIG.md) | `config.toml` 契约 |
| [`docs/PROJECT-STRUCTURE.md`](docs/PROJECT-STRUCTURE.md) | 目录与 crate 分层 |
| [`TASKS.md`](TASKS.md) | 施工顺序 |
| [`AGENTS.md`](AGENTS.md) | coding agent 约定 |
