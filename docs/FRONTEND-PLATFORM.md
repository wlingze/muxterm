# 前端形态：多前端维护成本与统一框架评估

> 探索记录。核对时间：2026-11-04T11:33–11:47+08:00（Asia/Shanghai）。
> 本文给出判断与决策门，不是已批准的迁移计划，也不授权本轮实现。
> 相关：[`FRONTEND.md`](FRONTEND.md)（Scene / PaneSurface 契约）、[`SURFACE.md`](SURFACE.md)（像素定律）、
> [`AGENT-CHAT-VIEW.md`](AGENT-CHAT-VIEW.md)（对话视图）、[`PRODUCT-DIRECTION.md`](PRODUCT-DIRECTION.md)。

## 1. 结论

**不建议现在做全量重写。建议先用一步低风险改动消掉重复的“模型层”，再在一个明确的技术门后面决定是否统一渲染框架。**

三条判断：

1. **重复的痛点在模型层，不在像素层。** 侧边栏投影、Attention 排序、快速连接、设置模型、
   输入草稿、目录补全这些纯逻辑，macOS 与 Linux 各写了一份。这部分可以直接搬进共享 Rust，
   不影响任何 widget 树，也不碰终端仿真器。
2. **全量重写的主要成本是终端仿真与平台集成，不是布局代码。** macOS 前端现有 43,269 行生产代码，
   其中 `Terminal/` 3,254 行是终端视图，其余是窗口、侧栏、面板、快捷键与 IME 边界处理。
   重写要用 Rust 的 VT 加渲染器替换 SwiftTerm，同时重建 23,482 行 Swift 测试的等价覆盖。
3. **统一框架有一个决定性技术问题**：新框架能否在同一窗口里承载一个原生终端子视图。
   如果必须连 VT 一起重写，那就是“全量重写”；如果只统一 chrome，风险小得多。

## 2. 现状数据

以下数字取自当前 `main`（`cd1e9d7c`）的已跟踪文件，用 `git ls-files` 与 `wc -l` 统计。

| 区域 | 文件数 | 行数 |
| --- | --- | --- |
| macOS 生产代码（Swift） | 68 | 43,269 |
| macOS 测试（Swift） | 90 | 23,482 |
| Linux 前端（Rust） | 87 | 28,685 |
| 共享 Rust 前端（`src/frontend/*.rs`） | 9 | 2,000 |
| Core（Rust） | 146 | 84,685 |
| 集成测试（`tests/`） | 98 | 35,222 |

macOS 内部分布：`App/` 22,840 行，`Chrome/` 9,576 行，`UI/` 4,773 行，`Terminal/` 3,254 行，
`CoreBridge/` 2,622 行。

技术栈事实：macOS 前端 **0 个文件使用 SwiftUI**，77 个文件 `import AppKit`；
终端视图基于 SwiftTerm（`Package.swift` 要求 1.15.0 起）。这与
[`PRODUCT-VISION-STRATEGIC-REVIEW.md`](PRODUCT-VISION-STRATEGIC-REVIEW.md) 记录的“AppKit，非 SwiftUI”一致。

### 2.1 改动分布

近 90 天，共 1,243 个提交：

| 区域 | 触及提交数 | 改动行数（已剔除 4 个目录搬迁提交） |
| --- | --- | --- |
| Core | 469 | 223,990 |
| macOS | 193 | 45,169 |
| Linux | 36 | 11,397 |

近 30 天，共 85 个提交；触及 macOS 的 54 个，其中 46 个不触及 Core。

读法：

- “前端改动越来越多”这个判断对 macOS 成立。近 30 天有 46 个提交只改 macOS。
- Linux 的改动量约为 macOS 的四分之一。这是**移植成本**，也是统一框架唯一能直接消掉的部分。
- 全量统一后，macOS 这 45,169 行的改动不会消失，只会从 Swift 变成 Rust。

### 2.2 重复的具体内容

按功能名统计两侧的命中文件数：

| 功能 | macOS 文件 | Linux 文件 |
| --- | --- | --- |
| Sidebar | 33 | 22 |
| Attention | 45 | 26 |
| QuickConnect | 31 | 33 |
| Settings | 22 | 17 |
| Search | 48 | 20 |
| Panel | 59 | 22 |

两侧的模型层已经是同构的：macOS `Chrome/` 9,576 行与 Linux `chrome/` 6,688 行描述同一批投影。
`FRONTEND.md` §2 已经把词表定义为平台无关，但**代码仍有两份**。

## 3. 三个候选方案

| 方案 | 做法 | 优点 | 代价 | 结论 |
| --- | --- | --- | --- | --- |
| A. 维持现状 | 继续每平台一份 | 零迁移风险；macOS 可继续领先 | 每次新交互都要移植一次 | 现状，不可持续 |
| B. 统一模型层，保留双 VT | 纯逻辑搬进共享 Rust，经 FFI 给 Swift / GTK；终端仍用 SwiftTerm / vte4 | 消掉主要重复；不动像素路径；可增量 | 仍需两套 widget 树 | **推荐先做** |
| C. 全量 Rust 重写 | 一个 Rust 前端跑 macOS + Linux | 一份代码 | 替换 VT 与渲染器；重建测试；框架风险 | 有条件再做 |

### 3.1 方案 B 的边界

搬进共享 Rust 的对象满足三个条件：无 widget 依赖、无平台 API、可用纯数据测试。

可以搬：侧边栏投影、聚合槽、Attention 排序与去重、状态栏文本、面板模型、快速连接选项、
设置字段模型、目录补全、快捷键映射、草稿与高亮、输入编码规则。

不搬：窗口、菜单栏、NSView / GtkWidget、IME 会话、拖拽、剪贴板、通知、字体枚举、终端视图。

收益按行数估算：两侧共约 16,000 行模型代码可收敛成约 8,000 行共享 Rust 加两端薄适配。
这是**估算**，未做原型验证。

### 3.2 方案 C 的前提

方案 C 只有在满足下面两条时才划算：

1. 方案 B 之后，剩余的前端代码仍足够多，值得再统一一次。
2. 有一个 Rust 框架能在同一窗口内承载终端视图，或者能替代 VT 而不牺牲输入正确性。

## 4. 框架候选

以下版本与许可证在 2026-11-04 直接读取 crates.io 与官方仓库确认。

| 框架 | 最新版本 | 许可证 | 渲染 | 与本项目相关的观察 |
| --- | --- | --- | --- | --- |
| iced | 0.14.0（2025-12-07） | MIT | wgpu，可退 tiny-skia | 下载量最大；x11 / wayland 后端；accessibility 有未合并的 draft |
| gpui（Zed） | 0.2.2（2025-10-22） | crate 声明 Apache-2.0 | Metal / Vulkan / DX | 仓库 Cargo.toml 已依赖 accesskit 三平台实现；README 声明 pre-1.0，版本间会破坏 |
| slint | 1.18.1（2026-09-21） | 三选一：免版税 / GPLv3 / 商业 | 自绘 | 免版税许可排除嵌入式；changelog 有 accessibility 修复记录 |
| egui | 0.37.0（2026-10-15） | MIT / Apache-2.0 | wgpu / glow | 即时模式，AccessKit 已集成；适合工具，控件密度与原生观感需要额外工作 |
| floem | 0.2.0（2024-11-14） | MIT | 自绘 | 最新发布距今约两年；accessibility 仍是 issue |
| libcosmic | 无 crates.io 发布，git 依赖 | MPL-2.0 | 基于 iced | COSMIC 桌面体系；`cosmic-term` 是可用参照 |

现有外部先例值得注意：`undivisible/herdr-gui` 是 macOS 上的 GPUI 客户端，内嵌
libghostty 作为终端，范围写明“只用 Herdr socket API”。这说明“Rust UI 加 libghostty 终端”
在 macOS 上已被走通一次。

终端仿真候选：`alacritty_terminal` 0.26.0、`vte` 0.15.0、`libghostty-vt` 0.2.2（2026-09-28）。
`cosmic-term` 的组合是 `alacritty_terminal` + `cosmic-text` + wgpu（`glyphon`），失败时退 softbuffer。

## 5. 决定性技术问题

下面每条都是可检查的，不是风格偏好。spike 必须逐条给出结论。

1. **原生子视图承载。** 框架能否把 `NSView`（SwiftTerm）或 `GtkWidget`（vte4）嵌进自己的窗口？
   多数自绘框架（gpui、egui、iced、slint）拥有整个窗口与渲染循环，不支持外部原生子视图。
   若不能，就必须同时替换 VT。
2. **中文输入法。** 现有实现有约 90 行专门处理：候选窗锚定到下一字符位置、
   过滤输入法占位符 U+FFFF、`hasMarkedText` 期间不改写按键。新框架必须提供等价的 marked text
   与候选窗定位能力。这是最容易在重写中回归的功能。
3. **无障碍。** 现状是 164 处 `setAccessibilityIdentifier`，用于测试定位；
   没有实现 AX 角色与值（`NSAccessibility` 用法为 0）。也就是说今天没有可用的屏幕阅读器支持。
   统一框架若提供 AccessKit 树，是净增能力；若没有，不能声称“保持现状”。
4. **测试能力。** macOS 现有 23,482 行 Swift 测试，包含 XCUITest 级别的窗口行为验证。
   新框架需要等价手段，否则关键交互会失去自动覆盖。
5. **打包与签名。** macOS 需要 app bundle、签名、公证、更新；Linux 需要各发行版依赖。
   `Package.swift` 目前直接用 SwiftPM；换成 Rust 需要重建这条流水线。
6. **终端渲染指标。** 需要 damage 重绘、字形图集、同步输出（synchronized output）支持。
   [`RENDERING-OPTIMIZATION.md`](RENDERING-OPTIMIZATION.md) 已记录现有引擎的实测情况，
   新方案必须复测同一批指标。
7. **许可证兼容。** 本项目是 MIT。gpui 的 crate 声明 Apache-2.0 可用；slint 免版税许可需逐条核对；
   libcosmic 是 MPL-2.0，属于文件级 copyleft，需确认分发方式。

## 6. 建议的路径

### 阶段 1：统一模型层（可立即开始，风险低）

1. 新建 `src/frontend/model/`，放纯数据结构与纯函数，不依赖任何 widget。
2. 先搬三个收益最高、争议最小的：侧边栏投影、Attention 排序、目录补全。
   这三处两侧都有测试，可以对照搬。
3. 经 FFI 暴露给 Swift 与 GTK。Swift 侧只保留视图代码。
4. 每搬一块，两侧行为必须保持逐项一致；用现有测试当回归网。

退出条件：`Chrome/` 与 `chrome/` 里不再有重复的业务判断；两侧同名模型只剩薄适配。

### 阶段 2：决策门（spike，限时）

用一个限时 spike 回答 §5 的七个问题，其中第 1 与第 2 条是硬门：

- 硬门 1：能否在框架窗口里嵌一个原生终端子视图。
- 硬门 2：中文输入的候选窗定位与占位符处理是否达到现有水平。

两条都过，方案 C 变成“替换 chrome，保留双 VT”，风险大降。
任一条不过，方案 C 只剩“连 VT 一起换”，必须先证明新 VT 在输入正确性上不倒退。

spike 交付物是一份对照记录，不是可合并代码。

### 阶段 3：按结论执行

- 硬门全过：先在一个二级页面（例如设置或快速连接）用新框架实现，与旧实现并存，逐步替换。
- 硬门不过：维持方案 B，把统一范围限定在模型层。

## 7. 不做什么

- 不在没有 spike 结论前启动全量重写。
- 不为了统一而重写已经稳定的终端像素路径。
- 不同时更换框架与 VT。
- 不把 `swiftui` 当作迁移目标。现状是 AppKit，换到 SwiftUI 本身也是一次重写。
- 不用“减少代码行数”当唯一理由。判据是每新增一个交互需要改几处。

## 8. 未确认

- iced 的 accessibility 合并状态。检索到 issue #552 与 draft PR #3111 / #1849，未取到当前状态（接口限流）。
- floem 与 slint 的 accessibility 完整度，只看到 issue 与 changelog 条目。
- 各框架在 macOS 上嵌入原生子视图的实际可行性，未做原型。
- 统一后 macOS 侧代码量的真实下降幅度（§3.1 的行数是估算）。
- 是否需要在 Linux 上保持 GTK 观感。若需要，统一框架的收益会明显降低。

## 9. 证据

1. 本地统计（`main` `cd1e9d7c`）：`git ls-files` + `wc -l`；`git log --since=90.days -M --numstat` 剔除目录搬迁提交后的改动行数。
2. crates.io API：iced 0.14.0、gpui 0.2.2、slint 1.18.1、egui 0.37.0、floem 0.2.0、alacritty_terminal 0.26.0、vte 0.15.0、libghostty-vt 0.2.2。
3. iced `Cargo.toml`：默认启用 wgpu 与 tiny-skia；x11 / wayland 后端。
4. Zed 仓库 `Cargo.toml`：`accesskit` 0.24 与 `accesskit_macos` / `accesskit_unix` / `accesskit_windows`。
5. gpui `README.md`：pre-1.0，版本间有破坏性变更；macOS 使用 Metal，需要 `font-kit` 才有字形。
6. slint `LICENSE.md`：免版税 / GPLv3 / 商业三选一，免版税排除嵌入式。
7. libcosmic `README.md` 与 `LICENSE`：基于 iced，MPL-2.0，git 依赖。
8. `cosmic-term` `README.md` 与 `Cargo.toml`：`alacritty_terminal` + `cosmic-text`，wgpu 失败退 softbuffer。
9. `undivisible/herdr-gui` `README.md`：macOS GPUI 客户端，内嵌 libghostty，只用 Herdr socket API。
10. 仓库内：`src/frontend/macos/Package.swift`（SwiftTerm 1.15.0 起）、`src/frontend/macos/Terminal/TerminalView.swift`（IME 处理）、
    `src/frontend/mod.rs`（共享模块清单）、`src/frontend/macos/Chrome/` 与 `src/frontend/linux/chrome/`（重复模型）。
