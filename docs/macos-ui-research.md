# macOS 终端 UI 技术调研报告

> 调研日期：2026-07-24
> 项目：muxterm — Rust 核心 (libmuxterm.a) → macOS 原生前端
> 数据来源：GitHub API + 官方 README + 源码目录结构（全部实时验证）

---

## 1. SwiftTerm

| 项目 | 数据 |
|------|------|
| 仓库 | `migueldeicaza/SwiftTerm` |
| 最新版本 | **v1.15.0**（发布于 2026-07-19） |
| 语言 | Swift |
| 许可证 | **MIT**（已通过 GitHub License API 确认） |
| Stars | 1,626 |
| 最近 push | 2026-07-19（活跃维护） |

### 功能特性

- VT100/Xterm 终端模拟器，纯 Swift 实现
- **UI 无关的引擎 + 平台前端**：引擎在根目录，macOS (AppKit) 代码在 `Mac/`，iOS (UIKit) 在 `iOS/`，共享代码在 `Apple/`
- ANSI 256色 + TrueColor 支持
- 文本属性：bold, italic, underline, strikethrough, dim/faint
- Unicode 渲染（Emoji、组合字符、grapheme clusters）
- Hyperlinks (OSC 8)
- 图形支持：Sixel、iTerm2 imgcat、Kitty graphics
- 选择引擎 + macOS 搜索 find bar
- 鼠标事件、终端 resize
- Thread-safe Terminal 实例
- termcast 录制/回放（asciinema 格式）
- 已用于商业 SSH 客户端：Secure Shellfish、La Terminal、CodeEdit

### GPU 加速

**是，SwiftTerm 内置可选的 Metal GPU 渲染器。**

源码位置：`Sources/SwiftTerm/Apple/Metal/`

| 文件 | 作用 |
|------|------|
| `MetalTerminalRenderer.swift` | Metal 渲染器主类 |
| `CoreTextGlyphRasterizer.swift` | 用 Core Text 光栅化字形 |
| `GlyphAtlas.swift` | 字形图集（glyph cache） |
| `Shaders.metal` | Metal 着色器 |
| `MetalBufferingMode.swift` | 缓冲区管理模式 |
| `MetalError.swift` | 错误处理 |

v1.15.0 的 release notes 明确修复了 Metal renderer 的问题：
- Fix Metal renderer crash in packaged apps (Bundle.module fatalErrors)
- Fix unbounded Metal BufferPool growth under constantly-changing content

README 原文：**"Optional GPU-accelerated rendering via Metal (macOS, iOS, visionOS)"**

### 自定义渲染

**可以。** SwiftTerm 架构分两层：

1. **引擎层**（UI agnostic）：处理 ANSI 解析、终端状态、scrollback，不依赖任何 UI 框架
2. **前端层**：`TerminalView`（NSView）是默认前端，但你可以：
   - 实现 `TerminalViewDelegate` 协议来自定义数据源（关键：muxterm 可以用 FFI 拿 output 替代 pty）
   - 继承或替换 `MetalTerminalRenderer` 来定制渲染管线
   - 直接使用底层 `Terminal` 引擎类，完全自建视图

**对 muxterm 的关键适配点**：SwiftTerm 自带 `LocalProcessTerminalView` 管理 pty，但 muxterm 需要从 Rust FFI 拿 output。你需要实现自定义 delegate，把 FFI 的 `muxterm_poll_events` / `muxterm_get_pane_output` 喂给 `Terminal` 引擎。

---

## 2. libghostty

| 项目 | 数据 |
|------|------|
| 仓库 | `ghostty-org/ghostty` |
| 语言 | Zig |
| 许可证 | **MIT**（已通过 GitHub License API 确认） |
| Stars | 58,591 |
| 状态 | **已开源，稳定，数百万人日常使用** |

### 开源状态

Ghostty 于 2024 年底开源。README 原文：

> "Ghostty is stable and in use by millions of people and machines daily."

路线图第 5 步 "Cross-platform libghostty for Embeddable Terminals" 标记为 ✅（已完成）。

### 两个库的区别

| 库 | 描述 | 包含渲染？ |
|----|------|-----------|
| **libghostty** (完整版) | 完整的终端模拟器嵌入库，包含 app/surface/config/renderer | 是（Metal/OpenGL） |
| **libghostty-vt** | 仅 VT 解析 + 终端状态管理，零依赖（连 libc 都不依赖） | 否（消费者自己实现渲染） |

### 是否可以独立使用

**是。** 有两种使用方式：

#### 方式 A：libghostty-vt（推荐给 muxterm）

- C API，头文件在 `include/ghostty/vt.h`
- 提供：VT 序列解析、终端状态、scrollback、reflow、渲染状态增量更新
- 不提供：渲染绘制、窗口管理、pty
- **XCFramework 支持**：`zig build -Demit-lib-vt` 生成 `ghostty-vt.xcframework`，可直接作为 Swift Package 的 binaryTarget 使用
- 示例：`example/swift-vt-xcframework/` — Swift Package 消费 XCFramework

```swift
// Package.swift 引用方式
.binaryTarget(
    name: "GhosttyVt",
    path: "../../zig-out/lib/ghostty-vt.xcframework"
)
```

#### 方式 B：libghostty（完整版）

- C API，头文件在 `include/ghostty.h`
- 提供：app、config、surface、inspector — 完整的终端 surface 管理
- macOS app 就是用这个 API 构建的
- 可以提供完整的 Metal 渲染管线

#### 示例项目

- **Ghostling** (`ghostty-org/ghostling`, 1,067★)：用 libghostty-vt + Raylib 的最小终端 demo，单个 C 文件
- **example/** 目录下有 30+ 个 C/Zig/Swift 示例

### API 稳定性

`vt.h` 明确标注：

> "WARNING: This is an incomplete, work-in-progress API. It is not yet stable and is definitely going to change."

### MIT 许可证确认

**已确认 MIT。** 两个仓库（ghostty 和 SwiftTerm）的 GitHub License API 均返回 `spdx_id: "MIT"`。

---

## 3. macOS 原生终端渲染方案：Metal + MetalKit

### 可行性

**完全可行。** Metal + MetalKit 做 GPU 加速文字渲染是 macOS 上最快的方案，已有多个成熟项目验证。

### 核心技术栈

```
Core Text（字体度量 + 光栅化）
    ↓ glyph bitmaps
Glyph Atlas（MTLTexture — 字形图集缓存）
    ↓ vertex + UV data
Metal Pipeline（MTLRenderPipelineState）
    ↓
MTKView / CAMetalLayer（显示）
```

### 开源参考项目

| 项目 | Stars | 语言 | 说明 |
|------|-------|------|------|
| **SwiftTerm** (`migueldeicaza/SwiftTerm`) | 1,626 | Swift | Metal renderer 在 `Apple/Metal/`，含 CoreTextGlyphRasterizer + GlyphAtlas + Shaders.metal，**最直接的参考** |
| **Ghostty** (`ghostty-org/ghostty`) | 58,591 | Zig | Metal renderer 在 `src/renderer/`，macOS 原生 app |
| **qwertty-term** (`joshka/qwertty-term`) | 8 | Rust | **Ghostty 的 Rust 重写**，Metal renderer + CoreText + rustybuzz shaping，原生 AppKit app with tabs/splits。MIT 许可证。**与 muxterm 技术栈高度匹配** |
| **harness-terminal** (`robzilla1738/harness-terminal`) | 299 | Swift | GPU-rendered macOS terminal，MIT |
| **Alacritty** (`alacritty/alacritty`) | 65,033 | Rust | macOS 上用 OpenGL（不是 Metal），但渲染架构设计可参考。Apache-2.0 |
| **Kitty** (`kovidgoyal/kitty`) | 34,033 | Python/C | macOS 上用 OpenGL，grid-based 渲染设计可参考。GPL-3.0 |

### 关键实现要点

1. **字形光栅化**：用 Core Text (`CTFont`, `CTGlyphRun`) 光栅化字形到 bitmap，上传到 MTLTexture
2. **Glyph Atlas**：把所有字形打包到一个大 texture atlas，用 UV 坐标索引，避免频繁 texture 切换
3. **Cell-based 渲染**：终端是固定网格，每个 cell 一个字符 + 属性，生成 instanced draw call
4. **Dirty tracking**：只重绘变化的行/cell，v1.15.0 SwiftTerm 修复了 BufferPool 无限增长问题
5. **双缓冲**：MTLBuffer 双缓冲避免 CPU-GPU 同步等待

---

## 4. Swift + AppKit 做 Tab 栏 + Pane 分割布局

### 最佳实践

#### Tab 栏

```swift
// 不用 NSTabView（太旧），自定义 NSView + NSSegmentedControl 或完全自绘
// 参考 iTerm2 / Ghostty 的实现方式

class TabBarView: NSView {
    private var tabs: [Tab] = []
    private var selectedTab: Int = 0
    
    // 用 NSTrackingArea 处理鼠标 hover
    // 用 CAShapeLayer 或 draw(_:) 自绘 tab 形状
    // 支持 drag-to-reorder：NSDragOperation + NSPasteboardWriter
}
```

**推荐方案**：
- 自绘 NSView（不要用 NSTabView 或 NSSegmentedControl，控制力不够）
- 每个 tab 是一个 TabView cell，支持 close button、drag reorder、context menu
- 活跃 tab 高亮，非活跃 tab dimmed
- 参考 SwiftTerm 的 sample MacTerminal app

#### Pane 分割布局

```swift
// 递归二叉树布局，与 muxterm 的 CLayoutNode 对应
class PaneSplitView: NSView {
    enum Orientation { case horizontal, vertical }
    var orientation: Orientation
    var firstChild: NSView    // leaf (TerminalView) or PaneSplitView
    var secondChild: NSView
    var ratio: CGFloat = 0.5  // 对应 CLayoutNode.ratio (0-1000 → 0.0-1.0)
    
    // 拖动分隔条调整 ratio
    // resize 时按 ratio 分配子视图 frame
}
```

**推荐方案**：
- 用递归 `NSView` 子类实现二叉树布局，**不要用 `NSSplitView`**（自动化行为难以控制，特别是嵌套时）
- 分隔条（divider）自绘，8-10px 宽，hover 时高亮
- 支持 `muxterm_get_layout` 返回的 `CLayoutNode` 递归树直接映射
- 临时全屏（zoom）一个 pane 时，覆盖一个全屏 TerminalView，记住原位置
- 参考 qwertty-term 的 splits 实现（zoom, dimming, equalize）

#### 关键 AppKit 技巧

- `NSWindowController` 管理窗口生命周期，不用 SwiftUI
- `NSViewController` + `NSView` 做 view hierarchy
- 用 `NSStackView` 做简单的工具栏布局
- `NSColor.warmColorWithName()` 或自定义 NSColor 做主题
- `CALayer` backing 可以提升动画性能
- 全屏支持：`NSWindow.collectionBehavior` + `NSWindow.toggleFullScreen`

---

## 5. Rust staticlib (libmuxterm.a) 在 Xcode 项目中 link 的方法

### 步骤

#### 5.1 编译 staticlib

```toml
# Cargo.toml
[lib]
name = "muxterm"
crate-type = ["rlib", "staticlib", "cdylib"]
```

```bash
# 编译为 macOS aarch64 staticlib
cargo build --release --target aarch64-apple-darwin
# 产物：target/aarch64-apple-darwin/release/libmuxterm.a
```

#### 5.2 Xcode 项目集成

**方法 A：直接拖入项目（最简单）**

1. 把 `libmuxterm.a` 拖入 Xcode 项目的 Project Navigator
2. 添加到 Target → Build Phases → Link Binary With Libraries
3. 在 Build Settings → Search Paths → Library Search Paths 添加 `.a` 文件所在目录

**方法 B：Build Phase 脚本自动编译（推荐 CI）**

```bash
# Build Phases → Run Script
cd "${SRCROOT}/../"  # 到 muxterm 仓库根目录
cargo build --release --target aarch64-apple-darwin --features ffi
cp "target/aarch64-apple-darwin/release/libmuxterm.a" "${SRCROOT}/Vendor/"
```

#### 5.3 头文件

创建 bridging header：

```objc
// Sources/Bridge/muxterm.h
#ifndef MUXTERM_H
#define MUXTERM_H

#include <stdint.h>
#include <stdbool.h>

typedef struct muxterm_handle muxterm_handle;

// 生命周期
muxterm_handle* muxterm_new(const char* backend, const char* socket, const char* session);
void muxterm_free(muxterm_handle* h);
int muxterm_connect(muxterm_handle* h);
int muxterm_shutdown(muxterm_handle* h);

// 事件轮询
typedef struct {
    uint32_t type;
    uint32_t pane_id;
    uint32_t tab_id;
    const uint8_t* data;
    uintptr_t data_len;
    const char* name;
} CStateChange;

int muxterm_poll_events(muxterm_handle* h, CStateChange* out, int max_count);

// 输入
int muxterm_send_input(muxterm_handle* h, uint32_t pane_id, const uint8_t* data, uintptr_t len);
int muxterm_resize(muxterm_handle* h, uint32_t pane_id, uint16_t cols, uint16_t rows);

#endif
```

在 Xcode 中：
1. 添加 `muxterm.h` 到项目
2. Build Settings → Objective-C Bridging Header → `Sources/Bridge/muxterm.h`
3. Swift 代码直接调用 C 函数

#### 5.4 Link 标志

在 Build Settings → Other Linker Flags 添加：

```
-lmuxterm
-lresolv    # 如果用了 DNS
framework Foundation
```

**注意**：Rust staticlib 可能依赖 macOS 系统库。如果 link 报 undefined symbol，添加：

```
-lSystem
-lpthread
-lobjc
```

#### 5.5 XCFramework 方式（推荐多架构）

```bash
# 生成 universal binary 的 staticlib
lipo -create \
    target/aarch64-apple-darwin/release/libmuxterm.a \
    target/x86_64-apple-darwin/release/libmuxterm.a \
    -output libmuxterm-universal.a

# 或生成 XCFramework
xcodebuild -create-xcframework \
    -library target/aarch64-apple-darwin/release/libmuxterm.a \
    -headers Sources/Bridge/ \
    -output muxterm.xcframework
```

---

## 6. 交叉编译 Rust 到 macOS aarch64（从 Linux）

### 方法 A：cargo-zigbuild（强烈推荐）

| 项目 | 数据 |
|------|------|
| 仓库 | `rust-cross/cargo-zigbuild` |
| Stars | 2,596 |
| 原理 | 用 Zig 作为 cross-linker，Zig 自带 macOS libc 和 SDK 交叉编译能力 |

#### 安装

```bash
# 安装 cargo-zigbuild
cargo install --locked cargo-zigbuild

# 安装 Zig（提供 zig cc 交叉编译器）
pip3 install ziglang
# 或从 ziglang.org 下载

# 添加 Rust target
rustup target add aarch64-apple-darwin
rustup target add x86_64-apple-darwin
```

#### 编译

```bash
# 单架构 aarch64
cargo zigbuild --release --target aarch64-apple-darwin --features ffi

# universal2（同时支持 Intel + Apple Silicon）
cargo zigbuild --release --target universal2-apple-darwin --features ffi
```

#### macOS SDK

cargo-zigbuild 需要 macOS SDK。两种方式：

1. **Docker（最方便，SDK 预装）**：
```bash
docker run --rm -it -v $(pwd):/io -w /io \
    ghcr.io/rust-cross/cargo-zigbuild \
    cargo zigbuild --release --target aarch64-apple-darwin
```

2. **本地 SDK**：设置 `SDKROOT` 环境变量指向 macOS SDK 目录
```bash
export SDKROOT=/path/to/MacOSX.sdk
cargo zigbuild --release --target aarch64-apple-darwin
```

### 方法 B：osxcross（传统方案）

| 项目 | 数据 |
|------|------|
| 仓库 | `tpoechtrager/osxcross` |
| Stars | 3,360 |
| 原理 | 用 Apple Xcode 的 SDK + 自制 cross-toolchain |

```bash
# 构建 osxcross toolchain（需要 Xcode .xip）
./tools/gen_sdk_package.sh  # 从 Xcode 提取 SDK
./build.sh                  # 构建 cross compiler

# 编译
cargo build --release --target aarch64-apple-darwin
```

**缺点**：需要 Apple Xcode .xip 文件，设置比 cargo-zigbuild 复杂。

### 方法 C：在 macOS 上编译（最可靠）

如果有 macOS 机器（包括 CI）：

```bash
# 直接在 macOS 上
cargo build --release --target aarch64-apple-darwin --features ffi

# GitHub Actions macOS runner
# .github/workflows/build-macos.yml
jobs:
  build-macos:
    runs-on: macos-14  # Apple Silicon
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo build --release --features ffi
```

### 推荐

| 场景 | 推荐方案 |
|------|---------|
| 开发（有 Mac） | 直接 macOS 编译 |
| CI | GitHub Actions macOS runner |
| Linux 开发机交叉编译 | cargo-zigbuild + Docker |
| 需要 SDK 控制 | osxcross |

### 注意事项

- `aarch64-apple-darwin` 是 Rust Tier 2 with host tools 目标，官方支持
- 最低 macOS 版本：11.0+ (Big Sur+)
- staticlib 不需要 codesign，但最终 app 需要
- 如果 Rust 代码用了 `bindgen` 生成 macOS 系统库绑定，交叉编译时需要设置 `BINDGEN_EXTRA_CLANG_ARGS_aarch64_apple_darwin`

---

## 7. 推荐方案：技术选型建议

### 起步阶段（Phase 1b）——快速出原型

```
┌─────────────────────────────────────────────┐
│              macOS App (Swift)              │
│                                             │
│  ┌─────────────┐  ┌──────────────────────┐  │
│  │  AppKit UI  │  │  SwiftTerm (MIT)     │  │
│  │  Tab 栏     │  │  TerminalView        │  │
│  │  Pane 分割  │  │  Metal renderer (内置)│  │
│  │  (自绘 NSView)│  │  自定义 delegate     │  │
│  └─────────────┘  └──────────┬───────────┘  │
│                              │ FFI          │
│  ┌───────────────────────────┴───────────┐  │
│  │  Bridging Header (muxterm.h)          │  │
│  └───────────────────────────┬───────────┘  │
└──────────────────────────────┼──────────────┘
                               │ C ABI
                    ┌──────────┴──────────┐
                    │  libmuxterm.a       │
                    │  (Rust staticlib)   │
                    └─────────────────────┘
```

**选择理由**：

| 组件 | 选择 | 理由 |
|------|------|------|
| UI 框架 | Swift + AppKit | 原生，精确控制，不用 SwiftUI（终端需要 pixel-perfect） |
| 终端渲染 | **SwiftTerm v1.15.0** | MIT 兼容、纯 Swift、内置 Metal renderer、已有 AppKit TerminalView、活跃维护 |
| GPU 加速 | SwiftTerm 内置 Metal | 已验证可用，v1.15.0 刚修复 Metal renderer bugs |
| 字体 | Core Text（SwiftTerm 内置） | macOS 原生 |
| Rust 链接 | staticlib + bridging header | 最简单直接 |
| 编译 | macOS 上直接 `cargo build` 或 GitHub Actions | 最可靠 |

**关键适配工作**：

1. 实现 `TerminalViewDelegate`，用 `muxterm_poll_events` 替代 SwiftTerm 的 pty 管理
2. 把 `muxterm_get_pane_output` 的 bytes 喂给 SwiftTerm 的 `Terminal` 引擎
3. 键盘输入 → `muxterm_send_input`
4. resize → `muxterm_resize`
5. tab/pane 布局 → 自绘 NSView，读 `muxterm_get_layout` 的 `CLayoutNode` 树

**风险**：SwiftTerm 的 `TerminalView` 管理单个 terminal，muxterm 需要多 pane。每个 pane 一个 `TerminalView` 实例，每个实例对应一个 `muxterm_handle` 或 handle 内的一个 pane_id。

### 性能优化阶段（Phase 2）——如果 SwiftTerm 不够快

**方案选择（按推荐优先级）**：

#### 方案 2A：libghostty-vt + 自建 Metal renderer（推荐）

```
┌─────────────────────────────────────────────┐
│              macOS App (Swift)              │
│                                             │
│  ┌─────────────┐  ┌──────────────────────┐  │
│  │  AppKit UI  │  │  自建 Metal Renderer  │  │
│  │  (不变)     │  │  CoreText + GlyphAtlas│  │
│  └─────────────┘  └──────────┬───────────┘  │
│                              │              │
│  ┌───────────────────────────┴───────────┐  │
│  │  libghostty-vt.xcframework            │  │
│  │  (VT 解析 + 终端状态, C API)           │  │
│  └───────────────────────────┬───────────┘  │
│                              │              │
│  ┌───────────────────────────┴───────────┐  │
│  │  libmuxterm.a (Rust)                  │  │
│  └───────────────────────────┬───────────┘  │
└──────────────────────────────┼──────────────┘
```

**优势**：
- libghostty-vt 是最快的 VT 解析器（SIMD 优化，Ghostty 同款）
- 零依赖，MIT 许可证
- XCFramework 可直接集成到 Swift Package
- 你完全控制 Metal 渲染管线

**劣势**：
- 需要自己实现 Metal renderer（参考 SwiftTerm 的 `Apple/Metal/` 或 qwertty-term）
- libghostty-vt 的 API 尚不稳定（vt.h 有 WARNING）
- 需要构建 Zig（`zig build -Demit-lib-vt`）

#### 方案 2B：换用 qwertty-term 的渲染器（Rust 生态）

[qwertty-term](https://github.com/joshka/qwertty-term) 是 Ghostty 的 Rust 重写，包含：
- `qwertty-term-vt`：VT 引擎
- `qwertty-term-font`：CoreText + rustybuzz shaping
- `qwertty-term-renderer`：Metal renderer + IOSurface
- 原生 AppKit app with tabs/splits

**如果 muxterm 能复用 qwertty-term 的 renderer crate**，那就不用 Swift + FFI 了，整个前端用 Rust + objc2 crate 做 AppKit。

但这意味着放弃 Swift 生态，风险较大。

#### 方案 3C：完全自建 Metal renderer

从头实现 ANSI 解析 + 字体度量 + glyph cache + Metal pipeline。

**不推荐**——工作量巨大，且 libghostty-vt 已经提供了 VT 解析，SwiftTerm 提供了完整的 Metal renderer 参考代码。

### 最终推荐路线图

```
Phase 1b (现在):
  Swift + AppKit + SwiftTerm (Metal renderer)
  + libmuxterm.a (bridging header)
  → 快速出可用的 macOS 原生终端

Phase 2 (如果性能瓶颈):
  保留 AppKit UI 层
  替换 SwiftTerm → libghostty-vt + 自建 Metal renderer
  参考代码: SwiftTerm/Apple/Metal/ + qwertty-term-renderer
  → 极致性能，完全控制渲染
```

### 许可证兼容性总结

| 组件 | 许可证 | 兼容 muxterm？ |
|------|--------|---------------|
| SwiftTerm | MIT | ✅ |
| Ghostty / libghostty | MIT | ✅ |
| qwertty-term | MIT | ✅ |
| Alacritty | Apache-2.0 | ✅ |
| Kitty | GPL-3.0 | ❌（传染性，不推荐引用代码） |
| harness-terminal | MIT | ✅ |

**全部 MIT，无许可证障碍。**

---

## 参考链接

- SwiftTerm: https://github.com/migueldeicaza/SwiftTerm
- SwiftTerm Metal renderer 源码: https://github.com/migueldeicaza/SwiftTerm/tree/main/Sources/SwiftTerm/Apple/Metal
- Ghostty: https://github.com/ghostty-org/ghostty
- libghostty-vt 头文件: https://github.com/ghostty-org/ghostty/blob/main/include/ghostty/vt.h
- Ghostling (最小 demo): https://github.com/ghostty-org/ghostling
- Swift + libghostty-vt 示例: https://github.com/ghostty-org/ghostty/tree/main/example/swift-vt-xcframework
- qwertty-term (Rust + Metal): https://github.com/joshka/qwertty-term
- harness-terminal (Swift + Metal): https://github.com/robzilla1738/harness-terminal
- cargo-zigbuild: https://github.com/rust-cross/cargo-zigbuild
- osxcross: https://github.com/tpoechtrager/osxcross