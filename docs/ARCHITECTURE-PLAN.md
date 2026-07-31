# Muxterm 全平台架构方案

> **本文档保留历史 C ABI 拆分方案与平台前端选型记录。**
>
> **分层架构（权威）**：见 [`docs/TRANSPORT-PROTOCOL-ARCHITECTURE.md`](TRANSPORT-PROTOCOL-ARCHITECTURE.md) —
> 主链 **Frontend → Core Protocol → Runtime → Transport**，Config 横切，Discovery 连接前查询。
>
> **目录结构（权威）**：见 [`docs/PROJECT-STRUCTURE.md`](PROJECT-STRUCTURE.md) —
> 当前与目标目录职责，`[proposed]` 标记尚未创建的目录。
>
> **ID 体系**：见 [`docs/ID-SYSTEM.md`](ID-SYSTEM.md) —
> `s{name}` / `wN` / `tN` / `pN`，tmux 真实 ID 只在 TmuxRuntime adapter 内部。
>
> **层级映射**：见 [`docs/LAYER-MAPPING.md`](LAYER-MAPPING.md) —
> tmux window → muxterm Tab；muxterm Window 虚拟固定 1 个。
>
> **交互模型**：见根目录 `ARCHITECTURE.md` —
> 窗口/tab/pane 生命周期、嵌套分割、焦点管理、快捷键表。

---

## 一、C ABI 拆分方案（历史记录，已部分实现）

### 1.1 核心原则

`src/` 顶层核心模块（protocol/runtime/transport/terminal/config/discovery/types/buffer_cap）编译为 `staticlib` + `cdylib`，导出 C ABI（`protocol/ffi/`）。
`src/platform/` 各平台前端通过 FFI 调用核心。

> **更新**：FFI 的权威设计见 `TRANSPORT-PROTOCOL-ARCHITECTURE.md` §9（FFI ABI 草案），
> 包含 `muxterm_open(MuxtermOpenSpec)` + mode 枚举 + Discovery + 内存所有权约束。
> 本节保留历史方案记录，实际实现以新设计为准。

### 1.2 当前问题

- `config/` 已无 `use gtk4`（PR #11 移除，ModifierType 转换移到 platform 层）
- `Backend` trait 有 `async fn` → C ABI 不支持 async，需要改用同步 + 轮询
- `StateChange` 含 `Vec<u8>` → C ABI 需要 `*const u8 + len`

### 1.3 FFI 模块设计

```
src/core/protocol/ffi/
├── mod.rs          # FFI 根模块
├── types.rs        # C 友好类型（CStateChange, CTask, CPaneId 等）
├── api.rs          # 导出函数（muxterm_connect, muxterm_execute, muxterm_poll_events）
└── callbacks.rs    # 回调注册（on_output, on_state_change）
```

> **更新**：目标结构见 `PROJECT-STRUCTURE.md`，`ffi/` 已并入 `protocol/ffi/`（扁平布局），补
> `CSession`/`CWindow`/`CPane.title`/`MuxtermOpenSpec`/`discover_*`。

### 1.4 C ABI 接口

```c
// ── 生命周期 ──
muxterm_handle* muxterm_new(const char* backend_type, const char* socket, const char* session);
void muxterm_free(muxterm_handle* h);
int muxterm_connect(muxterm_handle* h);      // 返回 0=ok, -1=err
int muxterm_shutdown(muxterm_handle* h);

// ── 命令执行（平台 → 核心）──
int muxterm_execute(muxterm_handle* h, const CTask* task);

// ── 事件轮询（核心 → 平台）──
// 非阻塞拉取事件，返回数量。每个事件通过 CStateChange* 数组返回。
int muxterm_poll_events(muxterm_handle* h, CStateChange* out, int max_count);

// ── 回调注册（可选，替代轮询）──
typedef void (*on_output_fn)(uint32_t pane_id, const uint8_t* data, uintptr_t len);
typedef void (*on_state_change_fn)(const CStateChange* event);
void muxterm_set_callbacks(muxterm_handle* h, on_output_fn output, on_state_change_change state);

// ── 状态查询 ──
int muxterm_get_tabs(muxterm_handle* h, CTab* out, int max_count);
int muxterm_get_panes(muxterm_handle* h, uint32_t tab_id, CPane* out, int max_count);
int muxterm_get_layout(muxterm_handle* h, uint32_t tab_id, CLayoutNode* out);
int muxterm_get_pane_output(muxterm_handle* h, uint32_t pane_id, uint8_t* buf, uintptr_t buf_len);

// ── 输入（平台 → 核心）──
int muxterm_send_input(muxterm_handle* h, uint32_t pane_id, const uint8_t* data, uintptr_t len);
int muxterm_resize(muxterm_handle* h, uint32_t pane_id, uint16_t cols, uint16_t rows);
```

> **更新**：新设计用 `muxterm_open(MuxtermOpenSpec)` 替代 `muxterm_new` 字符串参数，
> 增加 `muxterm_discover_*` / `muxterm_get_sessions` / `muxterm_last_error` 等。
> 详见 `TRANSPORT-PROTOCOL-ARCHITECTURE.md` §9。`muxterm_new` 保留为 wrapper。

### 1.5 C 友好类型

```c
struct CStateChange {
    uint32_t type;  // 0=PaneOutput, 1=TabAdded, 2=LayoutChanged, ...
    uint32_t pane_id;
    uint32_t tab_id;
    uint32_t window_id;
    // PaneOutput 时使用
    const uint8_t* data;
    uintptr_t data_len;
    // LayoutChanged 时使用
    const CLayoutNode* layout;
    // 名称（TabRenamed 等）
    const char* name;
};

struct CTask {
    uint32_t type;  // 0=SplitPane, 1=NewTab, 2=SwitchTab, ...
    uint32_t target_pane;
    uint32_t target_tab;
    uint32_t dir;   // 0=horizontal, 1=vertical
    const char* name;
    // ... 可扩展
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
};

struct CLayoutNode {
    uint32_t type;  // 0=leaf, 1=split_h, 2=split_v
    uint32_t pane_id;  // leaf 时
    uint32_t ratio;    // split 时 (0-1000)
    const CLayoutNode* first;   // split 时
    const CLayoutNode* second;  // split 时
};
```

> **更新**：新设计增加 `CPane.title`、`CSession`、`CWindow`、`MuxtermOpenSpec`。
> 内存所有权约束见 `TRANSPORT-PROTOCOL-ARCHITECTURE.md` §9.5（borrowed vs owned）。

### 1.6 Cargo.toml 改动

```toml
[lib]
name = "muxterm"
crate-type = ["rlib", "staticlib", "cdylib"]

[features]
default = ["gtk"]
gtk = ["dep:gtk4", "dep:vte4"]
tui = ["dep:crossterm"]
# C ABI 导出（不依赖任何 GUI）
ffi = []
```

### 1.7 实现步骤

1. 把 `config.rs` 里的 `gtk4::gdk::ModifierType` 转换移到 `src/platform/linux/`
2. 新建 `src/core/protocol/ffi/` 模块（已实现）
3. 定义 C 友好类型（types.rs）
4. 实现导出函数（api.rs）—— 内部持有 `TerminalModel`，包装同步调用
5. async fn connect/shutdown 改为同步（内部 tokio runtime block_on）
6. 添加 `#[cfg(feature = "ffi")]` gate
7. 测试：C 程序 link staticlib，验证基本流程

> **更新**：上述步骤已部分完成（`src/core/protocol/ffi/` 已存在并工作）。
> 后续 FFI 扩展计划见 `TRANSPORT-PROTOCOL-ARCHITECTURE.md` §13（阶段 5）。

---

## 二、各平台前端方案

### 2.1 Linux — GTK4 + VTE（已有）

| 项 | 选择 | 理由 |
|---|---|---|
| UI 框架 | **GTK4** | 已有，原生 Linux，Wayland/X11 支持 |
| 终端渲染 | **vte4**（GTK4 widget） | 已有，libvte 终端模拟器，GTK4 原生集成 |
| GPU 加速 | GTK4 内置（OpenGL/Vulkan backend） | GTK4 默认 GPU 渲染，Broadway/NGL renderer |
| 字体 | Pango + fontconfig | GTK4 原生 |
| 颜色 | GTK4 ColorDialog + VTE palette | 原生 |

**状态**：`src/platform/linux/` 已有完整实现（notebook, tab_bar, pane_view, keymap, theme 等）。

**改进方向**：
- 确保 `core/` 零 GTK 依赖（移除 config.rs 的 gtk4 引用）
- 通过 FFI 调用核心（可选，当前直接用 Rust 绑定也可以）
- VTE 4.8+ 支持 hyperlinks, Synchronized output, etc.

**速度**：GTK4 + NGL renderer 已经 GPU 加速。VTE 用 cairo/pango 绘制，在大量输出时可能 bottleneck。如果需要更快可以自建 OpenGL renderer，但 VTE 够用。

### 2.2 macOS — Swift + AppKit + Metal

| 项 | 选择 | 理由 |
|---|---|---|
| UI 框架 | **Swift + AppKit** | macOS 原生，不是 SwiftUI（终端需要精确控制） |
| 终端渲染 | **SwiftTerm** 或自建 Metal renderer | SwiftTerm 是 MIT，纯 Swift 终端模拟器 |
| GPU 加速 | **Metal** | macOS 原生 GPU API，最快 |
| 字体 | Core Text | macOS 原生字体引擎 |
| 颜色 | NSColor + Color Kit | 原生 |

**方案 A：SwiftTerm（推荐起步）**
- MIT 许可证，兼容
- 纯 Swift 终端模拟器，SwiftUI/AppKit 集成
- 已有 ANSI 解析、256色、truecolor、links、themes
- 可以 fork 后定制 tmux -CC 集成
- 风险：SwiftTerm 自己管理 pty，需要改成从 FFI 拿 output

**方案 B：libghostty（高性能但复杂）**
- MIT 许可证，兼容
- Ghostty 的终端渲染引擎（Metal 加速）
- 极快，但需要 Zig 交叉编译，集成复杂
- 适合 Phase 2 性能优化

**方案 C：自建 Metal renderer**
- 完全控制，最高性能
- 需要实现 ANSI 解析 + 字体度量 + glyph cache + Metal pipeline
- 工作量大，但最灵活
- 参考 alacritty 的 OpenGL renderer 移植到 Metal

**推荐**：先 SwiftTerm 起步，如果性能不够再换 libghostty 或自建 Metal。

**架构**：
```
macos/
├── MuxtermApp.xcodeproj
├── Sources/
│   ├── App/              # AppDelegate, WindowController
│   ├── TerminalView/     # SwiftTerm 定制 / Metal renderer
│   ├── TabBar/           # NSTabView 定制
│   ├── PaneLayout/       # NSView 分割布局
│   └── Bridge/           # Rust FFI 桥接（C ABI → Swift）
├── libmuxterm.a          # Rust staticlib（从 protocol/ffi 编译）
└── Package.swift
```

> **更新**：macOS 前端已有 `src/platform/macos/` 实现（SwiftUI + CoreBridge）。
> FFI 桥接共用边界见 `TRANSPORT-PROTOCOL-ARCHITECTURE.md` §10。

### 2.3 Windows — WinUI 3 + ConPTY + Direct2D/DirectWrite

| 项 | 选择 | 理由 |
|---|---|---|
| UI 框架 | **WinUI 3 (C#)** | Windows 原生，替代 WPF/UWP |
| 终端渲染 | **自建 Direct2D + DirectWrite** | 最快，GPU 加速 |
| 字体 | DirectWrite | Windows 原生字体引擎，ClearType |
| GPU 加速 | **Direct2D**（D3D11 backend） | Windows 原生 GPU 2D API |
| 伪终端 | **ConPTY**（Windows pseudo-console） | Windows 10+ 原生 |
| 颜色 | Windows.UI.Colors | 原生 |

**方案 A：WinUI 3 + 自建终端渲染（推荐）**
- WinUI 3 做 UI 框架（窗口、tab 栏、分割布局）
- 终端字符绘制用 Direct2D + DirectWrite（参考 Windows Terminal）
- ConPTY 管理本地 pty（但 muxterm 用 FFI，不用 ConPTY 直接）
- GPU 加速：Direct2D 自动用 D3D11

**方案 B：Avalonia UI（跨平台 .NET）**
- 但不是原生，性能不如 WinUI 3
- 好处是和 Linux/macOS 共享 .NET 代码

**方案 C：Qt6（C++）**
- 原生跨平台，但许可证复杂（GPL/商业）
- 不推荐

**推荐**：WinUI 3 + Direct2D，参考 Windows Terminal 的渲染架构。

> **更新**：Windows 不在 v1 范围（见 `TRANSPORT-PROTOCOL-ARCHITECTURE.md` §1.2）。

---

## 三、终端渲染方案对比

| 平台 | 渲染方案 | GPU | 速度 | 开发量 | 许可证 |
|------|----------|-----|------|--------|--------|
| Linux | VTE4 (GTK4 widget) | GTK4 NGL | 中 | 已有 | LGPL |
| Linux | 自建 OpenGL (alacritty 风格) | OpenGL/Vulkan | 极快 | 大 | MIT |
| macOS | SwiftTerm | CoreGraphics | 中 | 中 | MIT |
| macOS | libghostty | Metal | 极快 | 大 | MIT |
| macOS | 自建 Metal | Metal | 极快 | 大 | - |
| Windows | Direct2D + DirectWrite | D3D11 | 极快 | 大 | - |
| Windows | Windows Terminal 模块 | D3D11 | 极快 | 大 | MIT |

**推荐起步**：
- Linux: VTE4（已有，够用）
- macOS: SwiftTerm（快速起步，MIT 兼容）
- Windows: Direct2D + DirectWrite（参考 Windows Terminal）

> 详见 `docs/RENDERING-OPTIMIZATION.md`（Linux 渲染优化调研）。

---

## 四、FFI 数据流

```
平台前端                    Rust 核心
┌──────────────┐           ┌──────────────────────┐
│  UI 事件      │           │  TerminalModel        │
│  (键盘/鼠标)  │  execute  │  ┌─────────────────┐ │
│  → CTask     │ ────────→ │  │ Backend trait    │ │
│              │           │  │ (Shell/Tmux)     │ │
│  ┌──────────┐│           │  └─────────────────┘ │
│  │ 轮询/回调 ││  poll     │                       │
│  │ ← events ││ ←───────  │  StateChange 事件流   │
│  └──────────┘│           │                       │
│              │           │  pane_output (bytes)  │
│  渲染        │  get_     │  layout tree          │
│  ← state     │  state    │  tab/pane list        │
│  ← output    │ ←───────  │                       │
│  ← layout    │           └──────────────────────┘
└──────────────┘
```

> **更新**：权威数据流见 `TRANSPORT-PROTOCOL-ARCHITECTURE.md` §2.2（主链数据流）。

---

## 五、实施计划

### Phase 1a: C ABI 导出（立即开始）
1. 移除 config/ 的 gtk4 依赖（已完成）
2. 新建 src/core/protocol/ffi/ 模块
3. 定义 C 类型 + 导出函数
4. Cargo.toml 加 crate-type = ["staticlib", "cdylib"]
5. 写 C 测试程序验证 FFI

### Phase 1b: macOS 探索（并行）
1. 新建 macos/ 目录
2. Xcode 项目 + SwiftTerm 集成
3. link libmuxterm.a
4. 实现 FFI 桥接
5. 基础 UI：窗口 + tab 栏 + 终端

### Phase 1c: Windows 探索（并行）
1. 新建 windows/ 目录
2. WinUI 3 项目
3. P/Invoke Rust cdylib
4. Direct2D 终端渲染原型
5. 基础 UI：窗口 + tab 栏 + 终端

### Phase 1d: Linux 打磨
1. 确保 core/ 零 GUI 依赖
2. GTK4 前端打磨
3. TUI 前端保留（开发/CI 用）

### Phase 2:
1. HerdrBackend
2. Agent 感知层
3. 性能优化（自建 renderer）

> **更新**：权威分阶段实施计划见 `TRANSPORT-PROTOCOL-ARCHITECTURE.md` §13。
> 上述 Phase 1/2 保留为历史记录；实际执行以新计划为准。
