# Muxterm Architecture

> 项目组织结构与设计理念
> 本文档指导各平台前端如何实现，以及核心层与平台层的职责划分。

---

## 一、架构总览

```
src/
├── main.rs                  # 入口：选择平台
├── core/                    # 全平台共用核心（逻辑与状态，无 UI）
│   ├── types.rs             # 共享类型（PaneId, WindowId 等）
│   ├── config.rs            # 配置解析（TOML）
│   ├── terminal/            # 终端管理抽象
│   │   └── mod.rs           # （待填充：进程管理、输出缓冲）
│   └── tmux/                # tmux 控制协议（与平台无关）
│       ├── mod.rs
│       ├── protocol.rs      # % 消息解析器
│       ├── client.rs        # 异步 tmux -CC 客户端
│       ├── command.rs       # 命令构造器
│       └── pty.rs           # PTY 辅助
│
├── platform/                # 平台适配层（各平台独立实现）
│   ├── linux/               # Linux → GTK4
│   │   ├── mod.rs
│   │   ├── app.rs           # GTK Application
│   │   ├── window.rs        # 主窗口
│   │   ├── notebook.rs      # Tab 管理 + Pane 嵌套分割
│   │   ├── pane_view.rs     # 终端渲染（vte4）
│   │   ├── tab_bar.rs       # 底部极简 TabBar
│   │   ├── command_palette.rs  # 命令面板
│   │   ├── quick_pick.rs    # 快捷选择器
│   │   ├── pane_switcher.rs # Pane 切换
│   │   ├── keymap.rs        # 快捷键绑定
│   │   ├── title_watch.rs   # 标题自动更新
│   │   ├── tmux_dialog.rs   # tmux 选择对话框
│   │   ├── theme.rs         # ANSI 样式映射
│   │   ├── input_bar.rs     # 输入栏（保留未用）
│   │   ├── wiring.rs        # Tmux Client ↔ UI 桥接
│   │   └── lifecycle.rs     # 生命周期测试辅助
│   ├── macos/               # macOS → SwiftUI（规划中）
│   └── windows/             # Windows → WinUI3（规划中）
│
└── ARCHITECTURE.md           # 本文档
```

### 核心原则

**core/ 不依赖任何 GUI 框架。** 它的所有代码都可以在无显示环境下编译和测试。`core/tmux/` 只做 tmux 协议通信（字节流 → 结构化消息），`core/config.rs` 只做配置解析（TOML → 结构化配置）。`core/terminal/` 将存放终端管理抽象（进程生命周期、输出缓冲等），目前为空壳。

**platform/ 各平台独立实现同一个交互模型。** 每个平台的前端必须实现本文档描述的所有交互行为，保持一致的用户体验。平台层可以使用各自的原生 API（GTK4 / SwiftUI / WinUI3），但交互逻辑必须遵循本文档。

**main.rs 只做一件事：选择当前平台并启动。** Linux 就调 `platform::linux::app::run()`，macOS 就调 `platform::macos::app::run()`。

---

## 二、交互模型（各平台必须实现的行为）

以下定义所有用户交互的正确行为。每个平台前端必须严格遵循，否则会出现跨平台体验不一致的问题。这些行为也是所有测试的验证依据。

### 2.1 窗口生命周期

- 应用启动 → 创建主窗口 → 主窗口包含**一个默认 tab** → 默认 tab 包含**一个默认 pane**
- 默认 pane 运行可执行程序（配置项 `pane.default_command`，默认 `$SHELL`）
- 用户可配置启动时不创建默认 pane（`pane.create_on_start = false`），此时窗口显示空状态
- 窗口关闭 → 所有 tab 关闭 → 所有 pane 关闭 → 进程退出

### 2.2 Tab 生命周期

**创建 Tab：**
- 用户操作（快捷键 / 命令面板）触发 `new_tab` → 创建新 tab → 创建第一个 pane → 焦点跳到新 pane
- 新 tab 的 pane 在 `pane.workdir`（默认 `$HOME`）启动 `pane.default_command`
- 已有 pane 的工作目录可以传递给新 tab（继承当前 pane 的 cwd）

**关闭 Tab：**
- 当 tab 内所有 pane 都关闭时 → 自动关闭 tab
- 关闭 tab 时，所有 pane 的进程收到 SIGHUP（通过 pty 关闭）
- 关闭前检查是否有未保存的工作（由配置 `behavior.confirm_close_tab` 控制，默认 false）
- 最后一个 tab 关闭时的行为由配置 `behavior.on_last_pane_exit` 控制：
  - `close_window`（默认）→ 关闭窗口
  - `keep_empty` → 保留空窗口

**Tab 显示：**
- 每个 tab 显示为 TabBar 上一行文字：`<序号>:<名字>`
- 名字 = 当前激活 pane 的进程名（自动更新）
- 多 pane tab 显示 "· Npanes" 后缀（如 `2:bash · 2panes`）
- 当前激活 tab 高亮（反色或下划线）
- TabBar 默认在窗口底部（可配置 `ui.tab_bar_position`），高度 ≤ 24px

### 2.3 Pane 生命周期

**Pane = 一个可执行程序：**
- 每个 pane 运行且只运行一个可执行程序
- 程序可以是 shell（bash/zsh）、TUI 工具（vim/htop/opencode）、脚本、任何可执行文件
- 程序通过 pty（伪终端）启动，与用户交互

**Pane 创建：**
- Alt+D → 水平分割当前 pane（在当前 pane 位置嵌套一个 GtkPaned，原 pane 放一侧，新 pane 放另一侧）
- Alt+Shift+D → 竖直分割
- 新 pane 的启动程序同默认配置（`pane.default_command`）
- 新 pane 的工作目录 = 当前 pane 的工作目录
- 焦点立刻移到新 pane

**Pane 关闭：**
- 当 pane 内的程序**正常退出**（exit code 0）→ 关闭 pane
- 当 pane 内的程序**异常退出**（exit code ≠ 0）→ 关闭 pane + 状态栏显示 `{程序名} exited with code {N}`（短暂显示后消失）
- 所有 pane 都关闭 → 关闭 tab
- 进程被信号终止（SIGKILL/SIGTERM）→ 视为退出，关闭 pane

### 2.4 嵌套分割模型（关键）

Pane 分割使用**嵌套模型**（不是平铺模型）。这是用户多次纠正后确认的正确行为。

**平铺模型（错误）：**
- 第一次分割：左右各 50%
- 第二次分割（左边）：整个区域重新分配为三份
- 结果：每次分割都重新分配所有 pane 的大小

**嵌套模型（正确）：**
- 第一次分割：左右各 50%
- 第二次分割（左边）：左边 pane 被替换为一个 GtkPaned，原左边 pane 占 start_child（左侧，上下各 50%），新 pane 占 end_child（右侧，整体不变）
- 结果：右边半块始终为一整个 pane，左边半块被分成上下两个。即：
  ```
  初始：    Alt+D：    然后焦点在左 → Alt+Shift+D：
  ┌────┐   ┌──┬───┐   ┌──┬───┐
  │    │   │  │   │   │上│   │
  │ A  │   │ A│ B │   │──│ B │
  │    │   │  │   │   │下│   │
  └────┘   └──┴───┘   └──┴───┘
  ```
- 每次分割只替换**当前激活的叶子 pane**，不碰其他 pane
- 实现：当前被分割的 pane 位置替换为一个 GtkPaned（或其他平台的等同容器），原 pane 和新 pane 各占一端
- 嵌套深度不应该有上限，但实现应保证深度 ≥ 10 次连续分割不崩溃

**实现要点（GTK4 参考）：**
- 用 `GtkPaned` 实现嵌套，不是 `GtkGrid`
- 分割前先 `unparent` 当前 pane 的 terminal widget
- 创建新 Paned，start_child = 原 terminal，end_child = 新 terminal
- 新 Paned 替换原 terminal 在父容器中的位置
- 切换焦点（Alt+[ / Alt+]）时遍历 PaneNode 树，找到下一个/上一个叶子

### 2.5 焦点管理

- 任何用户操作后（创建/关闭/切换 tab 或 pane），焦点必须落到终端（terminal widget）
- 焦点不落到输入框、工具栏、菜单等其他控件
- tmux attach 模式下：没有额外的底部输入框。用户键盘输入通过 VTE commit 信号直接转发为 `send-keys` 到 tmux
- 输入框（`input_bar.rs`）保留但隐藏。它只用于调试/计划中的 CLI 模式，不在正常 UI 中显示

### 2.6 进程名自动更新

- Tab 和 Pane 的显示名字 = 当前运行进程的 argv[0] 的 basename
- 例：`/usr/bin/bash` → "bash"，`/usr/local/bin/opencode` → "opencode"
- 检测方式：
  - **本地模式**：读取 `/proc/{pid}/comm` 或查进程树
  - **tmux 模式**：`tmux display-message -p -t @N '#{pane_current_command}'`
- 更新频率：每秒轮询一次（`title_watch.rs`）
- 当用户在一个 pane 里启动新程序（如在 bash 中运行 `opencode`），标题自动更新
- 程序退出后，如果 pane 未关闭，标题恢复为配置的默认 shell 名

---

## 三、核心层详解（core/）

### 3.1 core/types.rs

跨平台共享的基本类型，不依赖任何 crate（纯 std）：

```rust
// Pane 标识符
pub struct PaneId(pub u32);  // "@1", "@12"
pub struct WindowId(pub u32); // "@1"（注意与 PaneId 同格式但语义不同）
pub struct SessionId(pub u32); // "$1", "$s"

// 布局描述
pub struct LayoutRect {
    pub cols: u32,
    pub rows: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
```

- `Parse` / `Display` trait 实现
- 非法输入时返回错误（非 panic）

### 3.2 core/config.rs

Alacritty 风格 TOML 配置，全平台共用。不同平台可忽略不支持的配置项。

```toml
[font]
family = "JetBrains Mono"
size = 13.0

[theme]
name = "light"

[tmux]
auto_mouse = true
default_session = ""

[scrollback]
lines = 10000

[ui]
tab_bar_position = "bottom"  # 或 "top"
show_title_bar = true

[pane]
default_command = "$SHELL"
workdir = "$HOME"

[behavior]
on_last_pane_exit = "close_window"
on_program_exit_abnormal = "notify"
confirm_close_tab = false

[[keybindings]]
key = "n"
mods = ["alt"]
action = "new_window"
```

- 解析失败时静默降级为默认值（记录 warning 但不崩溃）
- 所有字段有默认值，空配置文件 = 正常运行
- 所有快捷键可自定义，未配置的快捷键使用默认绑定

### 3.3 core/tmux/（完整）

tmux 控制协议的核心实现。详情见 `core/tmux/mod.rs` 和 `PRODUCT.md` 的协议说明。

- **protocol.rs**：行导向的 `%` 消息解析器。`parse_line(&str) -> Option<Message>`。纯函数，可独立单元测试
- **command.rs**：强类型命令构造器（`PaneId` newtype 防注入）。`send_keys()` / `split_window()` 等
- **client.rs**：异步 tmux -CC 进程管理（tokio）。spawn → read stdout（半行 buffer）→ parse → channel 发消息 → write stdin（命令）
- **pty.rs**：PTY 辅助函数

各平台前端不直接操作 tmux 进程，而是通过 `TmuxClient` 的消息流接收事件，通过 `CommandSender` 发送命令。

### 3.4 core/terminal/（待填充）

将来存放跨平台的终端管理抽象。目前为空壳，但以下概念已被识别：

- **进程生命周期管理**：spawn、pty 管理、exit 检测、信号发送
- **输出缓冲**：scrollback 行缓冲（纯环形缓冲区，无 ANSI 解析）
- **输入协议**：输入编码（将键盘事件转换为字节流写入 pty）
- **终端尺寸通知**：SIGWINCH / pty resize

各平台实现应将这些逻辑逐步上移到 `core/terminal/`，使其脱离 GUI 框架依赖。

---

## 四、平台层详解（platform/）

### 4.1 Linux 实现（GTK4）

当前 Linux 实现使用 GTK4 + gtk4-rs + vte4。

**关键模块职责：**

| 模块 | 职责 | 依赖 core |
|------|------|-----------|
| `app.rs` | GTK Application 启动、配置加载、主题加载 | config |
| `window.rs` | 主窗口布局、快捷键分发、焦点管理、生命周期协调 | 所有 core + platform 模块 |
| `notebook.rs` | `PaneNotebook`（tab 管理）+ `TabContent`（pane 列表）+ `PaneNode`（嵌套分割树） | types |
| `pane_view.rs` | `PaneView`：包裹 vte4 Terminal，处理 spawn、child-exited、commit 信号 | tmux(command) |
| `tab_bar.rs` | 极简 TabBar：序号+名字，高亮激活，点击切换 | — |
| `command_palette.rs` | VSCode 风格命令面板：输入框 + 命令列表浮层 | — |
| `quick_pick.rs` | 可复用选择器：`QuickPick<T>` 泛型，模糊匹配 + 滚动列表 | — |
| `pane_switcher.rs` | Alt+R pane 切换：列出所有 pane + 模糊过滤 | — |
| `keymap.rs` | 快捷键解析、匹配、默认键位表 | config |
| `title_watch.rs` | 定时轮询进程名，更新 UI 标题 | — |
| `tmux_dialog.rs` | tmux session 选择对话框（旧版，逐渐被命令面板替代） | tmux |
| `theme.rs` | ANSI SGR → 颜色样式映射（16 色/256 色/24-bit） | config |
| `wiring.rs` | `TmuxBridge`：连接 TmuxClient 的 Message 流到 UI 更新 | tmux |
| `input_bar.rs` | 输入栏（预留未用） | — |
| `lifecycle.rs` | 生命周期测试辅助 | — |

### 4.2 平台实现指南（给其他平台的开发者）

当你实现 macOS（SwiftUI）或 Windows（WinUI3）版本时：

1. **可复用的代码**（直接拿过来用）：
   - `core/tmux/` 全部（纯 Rust，无系统依赖）
   - `core/config.rs`（纯 Rust）
   - `core/types.rs`（纯 Rust）
   - `core/terminal/`（将来填充后）
   - Rust FFI 绑定：将核心编译为静态库，通过 C ABI 暴露给平台语言

2. **必须重新实现的模块**（平台特定 UI）：
   - `app.rs` → 应用启动、菜单栏
   - `window.rs` → 主窗口、布局管理
   - `notebook.rs` → Tab + Pane 容器
   - `pane_view.rs` → 终端渲染组件
   - `tab_bar.rs` → Tab 栏 UI
   - `command_palette.rs` → 命令面板 UI
   - `quick_pick.rs` → 选择器 UI
   - `keymap.rs` → 快捷键系统（绑定到平台事件）
   - `wiring.rs` → tmux 事件 → UI 更新桥接
   - `theme.rs` → ANSI → 原生颜色映射

3. **交互行为必须一致**（参考本文档第二部分）：
   - 窗口/tab/pane 生命周期完全一致
   - 嵌套分割模型完全一致
   - 焦点管理完全一致
   - 快捷键映射尽量一致（Alt+N/T/D/1-9/[]/R/P 为标准）
   - 配置文件格式完全一致（跨平台共享同一份 config.toml）

4. **技术选型建议**：
   - macOS：Rust 核心层编译为静态库 → Swift 通过 FFI 调用 → SwiftUI UI + SwiftTerm 渲染
   - Windows：Rust 核心层编译为 cdylib → WinUI 3 C#/C++ 前端调用 → DirectWrite 渲染
   - iOS/Android：Rust 核心层编译为静态库 → Swift/Kotlin FFI 绑定

---

## 五、快捷键表（默认）

| 组合键 | 动作 | 说明 |
|--------|------|------|
| Alt+N | 新窗口（new-window） | = 新 tab + 新 pane |
| Alt+T | 新 tab（本地 shell） | 在当前窗口新建 tab |
| Alt+D | 水平分割 pane | 在当前激活 pane 处嵌套分割 |
| Alt+Shift+D | 竖直分割 pane | 同上，方向不同 |
| Alt+1..9 | 切换到第 N 个 tab | 数字对应位置 |
| Alt+0 | 切换到最后一个 tab | — |
| Alt+[ | 上一个 pane | 在同 tab 的 pane 间切换 |
| Alt+] | 下一个 pane | 同上 |
| Alt+R | Pane 切换器 | 列出所有 pane 按名搜索跳转 |
| Alt+P | 命令面板 | VSCode 风格 |
| Alt+Shift+Q | 退出 | 关闭所有，退出应用 |

所有快捷键可通过 `config.toml` 的 `[[keybindings]]` 自定义。

---

## 六、设计理念

### 为什么用嵌套分割而不是平铺？

平铺分割（如 iTerm2 的 "Use balanced layout"）在多次不同方向分割后会导致 pane 大小不可预测，用户难以理解"为什么这次分割占了整个宽度/高度"。嵌套分割（如 tmux 默认行为）每次只动当前 pane，行为完全可预测：你分割的是当前 pane，不影响其他 pane。

这是用户反复测试后确认的偏好。

### 为什么没有工具栏/按钮？

所有操作通过快捷键或命令面板完成。工具栏/按钮占空间、容易被误触、在不同平台风格不统一。快捷键 + 命令面板可以在所有平台提供一致的体验。

### 为什么命令面板是 VSCode 风格？

命令面板是 VSCode 用户（目标用户群体）熟悉的交互模式。它内置搜索、可扩展、不占固定 UI 空间。相比传统菜单栏，命令面板更适合键盘驱动的工作流。

### 为什么连不连 tmux 体验一致？

tmux 是底层实现细节，不是用户界面。用户不应该感知到"我在 tmux 模式"还是"本地模式"。操作完全相同，只是底层实现不同（本地：spawn shell；tmux：split-window -h/-v）。这是 iTerm2 的设计理念，也是本项目的核心设计原则。

---

## 七、测试策略

### 可测试的（必须有单元测试）
- `core/types.rs`：类型解析和序列化
- `core/config.rs`：配置解析和降级
- `core/tmux/protocol.rs`：消息解析（89+ 测试，已覆盖所有消息类型和边界）
- `core/tmux/command.rs`：命令字符串生成
- `core/tmux/client.rs`：异步流程（mock tmux 进程）
- `platform/*/keymap.rs`：快捷键匹配逻辑
- `platform/*/quick_pick.rs`：模糊搜索逻辑
- `platform/*/command_palette.rs`：命令列表和过滤
- `platform/*/notebook.rs`：PaneNode 树操作（分割/删除/切换）
- `platform/*/tab_bar.rs`：显示名生成
- `platform/*/title_watch.rs`：进程名提取
- `platform/*/theme.rs`：ANSI 颜色映射
- `platform/*/lifecycle.rs`：生命周期流程

### 不可自动测试的（需要人工验证）
- 窗口是否真的显示（Wayland/X11 显示服务）
- 鼠标点击交互
- 快捷键在真实键盘输入下是否生效
- tmux attach 的端到端流程
- 窗口 resize 行为
- 跨平台视觉一致性

当前测试总数：**216 个**（130 个原协议测试 + 86 个新增 UI 逻辑测试），全部在无显示环境下通过。
