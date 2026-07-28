# Muxterm 项目结构

> **文档定位**：说明 muxterm 当前目录结构与目标目录结构，标注现有与 proposed。
> 不创建代码目录，仅记录结构设计。
>
> **基线**：`/home/wlz/Developer/self/muxterm` main `d69fab2`（2026-07-28）。
> 相关文档：`docs/TRANSPORT-PROTOCOL-ARCHITECTURE.md`、`docs/ARCHITECTURE-PLAN.md`、`ARCHITECTURE.md`。

---

## 1. 当前目录结构

```
muxterm/
├── src/
│   ├── main.rs                  # bin 入口：CLI 命令模式 vs 交互模式
│   ├── main_entry.rs            # cli_command_to_task()（CliCommand → Task 映射）
│   ├── lib.rs                   # 库入口（供集成测试）
│   ├── cli/                      # CLI 命令解析 + 格式化 + daemon IPC
│   │   ├── command.rs            #   parse_cli_command() → CliCommand enum（30 变体）
│   │   ├── format.rs             #   format_output() → JSON/text
│   │   ├── client.rs             #   send_command() → unix socket
│   │   ├── daemon.rs             #   run_daemon() → LocalBackend + TerminalModel + socket
│   │   ├── ipc.rs                #   Request/Response（serde_json over unix socket）
│   │   ├── session.rs            #   socket 路径推导 / 列出 session
│   │   └── mod.rs
│   ├── core/                     # 全平台共用核心（无 GUI 依赖）
│   │   ├── types.rs              #   PaneId/WindowId/TabId/SessionId（newtype + serde）
│   │   ├── config.rs             #   TOML 配置 + 主题解析（⚠️ 有 gtk4 引用待移除）
│   │   ├── buffer_cap.rs         #   有界缓冲（pane output / event queue 上限）
│   │   ├── model/                #   纯模型层（State/Task/Layout/Backend/TerminalModel）
│   │   │   ├── state.rs          #     State trait + StateChange enum + 快照类型
│   │   │   ├── task.rs            #     Task enum + TaskOutcome
│   │   │   ├── layout.rs          #     LayoutNode（嵌套二叉树）+ TabLayout
│   │   │   ├── backend.rs          #     Backend trait
│   │   │   ├── backend/mock.rs     #     MockBackend（测试用）
│   │   │   ├── terminal_model.rs  #     TerminalModel（编排 task→backend→events）
│   │   │   └── mod.rs
│   │   ├── backend/              #   Backend 实现
│   │   │   ├── local.rs           #     LocalBackend（≈ ShellRuntime + LocalTransport，待拆分）
│   │   │   ├── tmux.rs            #     TmuxBackend（≈ TmuxRuntime + LocalTransport，待拆分）
│   │   │   ├── daemon.rs          #     DaemonBackend（IPC client，连本地 daemon）
│   │   │   └── mod.rs
│   │   ├── tmux/                  #   tmux 控制协议
│   │   │   ├── protocol.rs        #     parse_line() → Message enum（130+ 测试）
│   │   │   ├── command.rs          #     TmuxCommand 构造器
│   │   │   ├── client.rs           #     TmuxClient / TmuxClientHandle（异步 spawn + 事件流）
│   │   │   ├── pty.rs              #     PTY 辅助（待提取为 Transport）
│   │   │   └── mod.rs
│   │   ├── ssh/                  #   SSH 远程传输（库级，待改为 spawn 系统 ssh）
│   │   │   ├── client.rs          #     SshConfig / SshSession / RemoteTmuxClient
│   │   │   └── mod.rs
│   │   ├── terminal/             #   终端管理
│   │   │   ├── input.rs            #     KeyEvent + encode()（键盘→pty 字节）
│   │   │   ├── process.rs          #     spawn_program / kill / get_process_name
│   │   │   ├── scrollback.rs        #     ScrollbackBuffer（环形行缓冲）
│   │   │   └── mod.rs
│   │   ├── ffi/                  #   C ABI 导出（feature = "ffi"）
│   │   │   ├── api.rs              #     muxterm_new/free/connect/execute/poll/...
│   │   │   ├── types.rs            #     #[repr(C)] 结构体 + 常量
│   │   │   ├── callbacks.rs        #     FfiCallbacks + muxterm_set_callbacks()
│   │   │   └── mod.rs
│   │   └── mod.rs
│   └── platform/                 #   平台适配层
│       ├── mod.rs
│       ├── linux/                #   GTK4 前端（feature = "gtk"）
│       │   ├── app.rs / window.rs / notebook.rs / pane_view.rs / tab_bar.rs
│       │   ├── command_palette.rs / quick_pick.rs / pane_switcher.rs
│       │   ├── keymap.rs / theme.rs / title_watch.rs / tmux_dialog.rs
│       │   ├── ffi_bridge.rs / renderer.rs / layout_host.rs / lifecycle.rs
│       │   ├── input_bar.rs（预留未用）
│       │   └── mod.rs
│       ├── tui/                  #   crossterm TUI 前端（feature = "tui"）
│       │   ├── app.rs / render.rs / ffi_bridge.rs
│       │   └── mod.rs
│       └── macos/                #   macOS SwiftUI 前端（Swift，Xcode 项目）
│           ├── App/ Chrome/ CoreBridge/ Terminal/ UI/ ...
│           └── ...
├── tests/                        # 集成测试
│   ├── cli_integration.rs        #   CLI × LocalBackend（cat 子进程）
│   ├── cli_tmux_e2e.rs           #   真实二进制 + tmux -L
│   ├── tmux_backend_integration.rs #  TmuxBackend
│   ├── tui_integration.rs        #   TUI 进程 + 宿主 tmux capture
│   ├── macos_integration.rs      #   FFI + tmux attach（feature = "ffi"）
│   ├── linux_gtk_integration.rs #   GTK UI（需 DISPLAY）
│   └── samples/                  #   tmux -CC 抓样例输出
├── configs/                      # 内置配置示例与主题
│   ├── config.example.toml
│   └── themes/{dark,light}.toml
├── docs/                         # 文档
│   ├── ARCHITECTURE-PLAN.md
│   ├── ID-SYSTEM.md
│   ├── LAYER-MAPPING.md
│   ├── RENDERING-OPTIMIZATION.md
│   ├── TRANSPORT-PROTOCOL-ARCHITECTURE.md  # 本文配套
│   ├── PROJECT-STRUCTURE.md                 # 本文
│   └── macos-ui-research.md
├── scripts/
│   └── worktree-setup.sh
├── Cargo.toml
├── PRODUCT.md
└── ARCHITECTURE.md
```

---

## 2. 目标目录结构（proposed）

以下目录在代码中**尚不存在**，标记为 `[proposed]`。实现时按 `TRANSPORT-PROTOCOL-ARCHITECTURE.md` 的分层创建。

```
src/core/
├── protocol/                    [proposed] — Core Protocol 层
│   ├── mod.rs                   #   Session/Window/Tab/Pane 模型、ID 规则、能力声明
│   ├── task.rs                  ← 从 model/task.rs 迁入
│   ├── state.rs                 ← 从 model/state.rs 迁入
│   ├── layout.rs                ← 从 model/layout.rs 迁入
│   └── snapshot.rs              #   Snapshot 序列化/反序列化（v1 稳定）
│
├── runtime/                     [proposed] — Runtime 层
│   ├── mod.rs                   #   RuntimeMode enum + create_backend() 工厂
│   ├── shell.rs                 ← 从 backend/local.rs 重构（ShellRuntime）
│   ├── tmux.rs                  ← 从 backend/tmux.rs 重构（TmuxRuntime）
│   ├── tmux_adapter.rs          #   ID 映射 + 协议解析 + 命令构造（从 tmux/ 迁入）
│   └── backend.rs               ← 从 model/backend.rs 迁入（Backend trait）
│
├── transport/                   [proposed] — Transport 层
│   ├── mod.rs                   #   Transport trait + TransportSignal
│   ├── local.rs                 ← 从 tmux/pty.rs + backend/local.rs 提取（LocalProcessTransport）
│   └── ssh.rs                   ← 从 ssh/client.rs 重构（SshProcessTransport，spawn 系统 ssh）
│
├── config/                      [proposed] — Config 横切层
│   ├── mod.rs                   #   ConfigService trait + ConfigValue + 变更事件
│   ├── loader.rs                ← 从 config.rs 迁入（TOML 解析）
│   └── defaults.rs              #   默认值定义
│
├── discovery/                   [proposed] — Discovery 连接前查询
│   ├── mod.rs                   #   SshHostDiscovery / TmuxSessionDiscovery / FsDiscovery traits
│   ├── ssh_config.rs            #   解析 ~/.ssh/config Host alias
│   ├── tmux_sessions.rs         ← 从 main.rs::find_existing_tmux_session 提取
│   └── fs.rs                    #   目录列表（本地 + 远程 exec ls）
│
├── ffi/                         # 现有 — C ABI 导出（扩展）
│   ├── api.rs                   #   补 muxterm_open / discover_* / get_sessions / ...
│   ├── types.rs                 #   补 CSession / CWindow / CPane.title / MuxtermOpenSpec
│   ├── callbacks.rs             #   现有
│   └── mod.rs
│
├── terminal/                    # 现有 — 终端管理（纯逻辑）
│   ├── input.rs                 #   现有
│   ├── process.rs               #   现有
│   ├── scrollback.rs            #   现有
│   └── mod.rs
│
├── tmux/                        # 现有 — tmux 协议解析/命令（被 TmuxRuntime adapter 复用）
│   ├── protocol.rs              #   现有（130+ 测试，不改）
│   ├── command.rs               #   现有（不改）
│   ├── client.rs                #   现有（逐步迁入 transport + runtime）
│   ├── pty.rs                   #   现有（逐步迁入 transport/local.rs）
│   └── mod.rs
│
├── types.rs                     # 现有 — ID 类型（newtype）
└── buffer_cap.rs                # 现有 — 有界缓冲
```

```
src/cli/                         # 现有 — CLI 命令解析 + 格式化 + daemon IPC
├── command.rs                   #   补 ssh/shell/tmux session/fs/watch 命令
├── format.rs                    #   迁移到 serde_json；补 json-pretty
├── client.rs / daemon.rs / ipc.rs / session.rs  # 现有
└── mod.rs
```

```
src/platform/
├── tui/                         # 现有 — crossterm TUI
│   ├── ffi_bridge.rs            #   适配新 FFI（CPane.title / get_sessions / ...）
│   ├── app.rs / render.rs
│   └── mod.rs
├── linux/                       # 现有 — GTK4
│   ├── ffi_bridge.rs            #   适配新 FFI
│   ├── ...（现有 UI 模块）
│   └── mod.rs
└── macos/                       # 现有 — SwiftUI
    ├── CoreBridge/
    │   ├── CoreBridge.swift     #   适配新 FFI
    │   ├── include/muxterm.h    #   与 types.rs 同步
    │   └── shim.c
    └── ...
```

```
tests/                           # 现有 + 新增
├── protocol_unit.rs             [proposed] — Core Protocol 单元测试
├── transport_local.rs           [proposed] — LocalProcessTransport
├── transport_ssh.rs             [proposed] — SshProcessTransport（#[ignore] 默认）
├── runtime_shell.rs             [proposed] — ShellRuntime
├── runtime_tmux.rs              [proposed] — TmuxRuntime
├── discovery_ssh.rs             [proposed] — ~/.ssh/config 解析
├── cli_v1_new.rs                [proposed] — ssh hosts / shell open / tmux session / fs / watch
├── ffi_extended.rs              [proposed] — 新 FFI 函数
├── cli_integration.rs           #   现有
├── cli_tmux_e2e.rs              #   现有
├── tmux_backend_integration.rs #   现有
├── tui_integration.rs           #   现有
├── macos_integration.rs         #   现有
├── linux_gtk_integration.rs     #   现有
└── samples/                     #   现有
```

---

## 3. 迁移说明

### 3.1 model/ → protocol/ + runtime/

现有 `core/model/` 里的纯数据类型（Task/StateChange/LayoutNode）迁入 `core/protocol/`；Backend trait 迁入 `core/runtime/`。TerminalModel 可留在 `core/protocol/` 或 `core/runtime/`（它是编排层，用 Backend trait）。

### 3.2 backend/ → runtime/ + transport/

- `backend/local.rs` → `runtime/shell.rs`（ShellRuntime）+ `transport/local.rs`（LocalProcessTransport）。
- `backend/tmux.rs` → `runtime/tmux.rs`（TmuxRuntime）+ 复用 `transport/local.rs`。
- `backend/daemon.rs` 保留（IPC client，不 fit Transport 抽象）。

### 3.3 ssh/ → transport/ssh.rs + discovery/ssh_config.rs

- `ssh/client.rs` 的 SSH exec 逻辑 → `transport/ssh.rs`（改为 spawn 系统 ssh）。
- `~/.ssh/config` 解析 → `discovery/ssh_config.rs`。

### 3.4 config.rs → config/

移除 `config.rs` 的 gtk4 引用；拆分为 `config/loader.rs`（解析）+ `config/defaults.rs` + `config/mod.rs`（ConfigService API）。

### 3.5 优先级

1. Transport 抽象（提取不破坏行为）
2. Runtime 重命名 + 接受 Transport
3. ID 隔离强化
4. SshProcessTransport + Discovery
5. Config API + 移除 gtk4 依赖
6. FFI 扩展

> 所有迁移为 `[proposed]`，不在本文档执行。代码目录不创建。
