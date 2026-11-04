# gpui 前端 parity 计划

> 基于 macOS 前端源码的清点。核对时间：2026-11-04T15:29–17:10+08:00（Asia/Shanghai）。
> 分支 `feature/union-frontend`。详细清点见本目录四份分解文档。

这份文档回答一个问题：**macOS 前端有的东西，gpui 要按什么顺序、用什么标准复刻。**

它不是已完成的实现报告。每一行都标了当前状态。

## 1. 清点范围与证据

| 区域 | 文件 | 行数 | 详情 |
| --- | ---: | ---: | --- |
| `src/frontend/macos/App/` | 20 | 22,840 | [app.md](app.md) |
| `src/frontend/macos/Chrome/` | 36 | 9,576 | [chrome.md](chrome.md) |
| `src/frontend/macos/UI/` | 7 | 4,773 | [ui.md](ui.md) |
| `src/frontend/macos/Terminal/` | 2 | 3,254 | [terminal.md](terminal.md) |
| `src/frontend/macos/CoreBridge/` | 1 | 2,622 | [terminal.md](terminal.md) §8 |
| **生产代码合计** | **66** | **43,065** | |
| 测试（AppE2ETests / ChromeTests / UITests） | 90 | 23,482 | [tests.md](tests.md) |

测试方法 **852 个**，按主层级归属统计：

| 层级 | 文件 | 方法数 | 移植手段 |
| --- | ---: | ---: | --- |
| pure-logic | 10 | 264 | 直接 Rust 单测 |
| model-projection | 21 | 213 | Rust 单测 + `ViewStore` 投影断言 |
| pixel-or-layout | 25 | 267 | `#[gpui::test]` 无头 + 布局/命中断言 |
| real-runtime-e2e | 31 | 84 | 隔离 tmux / named Herdr + 挂载前端 |
| ffi-integration | 2 | 2 | 隔离配置根 + FFI |
| xcuitest-drives-real-app | 1 | 22 | 需要真 GUI 驱动 |
| **合计** | **90** | **852** | |

## 2. 三条可行性结论

**第一，IME 不是障碍，之前的判断是错的。** [`FRONTEND-PLATFORM.md`](../FRONTEND-PLATFORM.md) 把中文输入法列为硬门。实测 gpui 0.2.2 提供完整 `EntityInputHandler`，包含 `marked_text_range`、`replace_and_mark_text_in_range`、`unmark_text`，以及 `bounds_for_range`，其注释明确写着用于定位输入法候选窗。macOS 侧的三条规则都能照搬：候选窗锚定下一字符、末列时折到 `x=0, y-=height`、过滤 U+FFFF/U+FFFE 占位符（`Terminal/TerminalView.swift:106-186`）。

**第二，真正的障碍是无障碍和通知。** gpui 0.2.2 的 `Cargo.toml` 没有 accesskit 依赖，也没有通知接口。macOS 现状是 164 处 `setAccessibilityIdentifier`，只用于测试定位，没有实现 AX 角色与值，所以今天也没有屏幕阅读器支持。这意味着 gpui 在这两项上「保持现状」是可能的，「净增能力」需要单独提案。

**第三，Linux 那边的 Rust 模型不能直接复用。** `src/frontend/linux/chrome/` 的 7 个「纯逻辑」文件仍然 import `crate::frontend::linux::*`，并且用 `Rc<RefCell<>>`。清点结果：36 个 Chrome 模型里 7 个在 Linux 无对应实现，6 个是子集，2 个规则冲突。gpui 需要自己的模型层。

## 3. Parity 矩阵

按依赖顺序排列。状态列只有三种：`已有`、`待建`、`不移植`。

### 3.1 终端 Surface

| 能力 | macOS 依据 | gpui 现状 | 波次 |
| --- | --- | --- | --- |
| CellGrid / VT 解析 / SGR | SwiftTerm | 已有（1,100 行 + 42 测试） | 已完成 |
| 按键编码与 xterm 修饰键参数 | `TerminalView.swift:131-152` | 已有 | 已完成 |
| 查询应答 OSC 10/11/12 + DA1 | `Chrome/TerminalQueryReply.swift` | 待建 | 2 |
| 括号粘贴包裹 | `TerminalView.swift:681-694` | 待建 | 2 |
| 滚轮路由四分支 + 整格合并 | `TerminalView.swift:451-543` | 待建 | 2 |
| 鼠标上报 SGR / X10 | `TerminalView.swift:544-612` | 待建 | 3 |
| 宽字符与组合字符 | `TerminalView.swift:141-160` | 待建 | 3 |
| 本地 scrollback + viewport offset | `TerminalView.swift:888-928` | 待建 | 3 |
| 渐进式词选（双击/三击/拖动） | `TerminalView.swift:315-450` | 待建 | 3 |
| IME marked text + 占位符过滤 | `TerminalView.swift:123-186` | 待建 | 4 |
| 候选窗锚定 | `TerminalView.swift:106-120` | 待建 | 4 |
| 剪贴板图片粘贴 | `TerminalView.swift:8-31,614-657` | 待建 | 4 |
| per-pane 颜色回报 Core | `TerminalView.swift:1039-1069` | 待建 | 4 |
| seed 分块 + 后台输出上限 | `TerminalManager.swift:102-127,780-795` | 待建 | 5 |
| committed paint（同步帧防撕裂） | `TerminalView.swift:700-760` | 待建 | 6 |

### 3.2 Chrome 模型

36 个文件全部零 AppKit import，清点出 33 条可比较的模型规则（一个文件可含多条）。按 [chrome.md](chrome.md) 的 delta 分类：

| 类别 | 条数 | 处理 |
| --- | ---: | --- |
| Linux 行为一致 | 2 | 复制规则到 gpui 模型层，附差分测试 |
| Linux 是子集 | 13 | 补齐缺失规则，缺失项在表格里逐条点名 |
| 规则冲突 | 6 | 以 Swift 为准，冲突点写进注释和测试 |
| Linux 无对应 | 12 | 净新增 |

净新增里最关键的几条：`AggregateWorkspaceModel` 的 Shells/Agents 聚合与延迟激活、`AttentionModel` 的前端隐藏身份（与 Core acknowledge/mute 分离）、`StructuredPaneAgent` 的版本与注册表、`SceneStack` 的 rename/rekey、`PaneResizeMath` 的拖拽像素数学、`WorkspaceReplicaID` 的 `name:path@transport` 构造。

### 3.3 窗口与面板

`App/` 清点结论：61 项需要 gpui widget 工作，6 项需要原生桥，4 项是纯逻辑，**没有判定为不可行的项**（[app.md](app.md):37 明确说明证据不足以得出不可行结论）。

需要原生桥的 6 项：

| 项 | gpui 现状 |
| --- | --- |
| 原生主窗口（透明标题栏、隐藏红绿灯、最小尺寸、背景拖动关闭） | 部分可用：`WindowOptions.titlebar`、`window_min_size`；红绿灯位置有 `traffic_light_position` |
| 单实例激活 | 无，需要 bundle 层或 LaunchServices 处理 |
| 原生通知 | 无接口 |
| 开发者工具授权状态与请求 | 无，需要系统 API |
| 无障碍标识与可读状态 | 无 accesskit 依赖 |
| 关于窗口里用 `gh` 提交 issue | 可用 `std::process`，不属于原生桥 |

### 3.4 UI 组件

7 个文件、4,773 行，[ui.md](ui.md) 里逐组件列出了布局规则、视觉状态、交互和 design token。其中 `PaneLayout.swift` 的尺寸数学是两条实现最容易漂移的地方，已单独抽成常量表。

## 4. 波次计划

每一波都以「可验证单元」结束，自带测试和提交。

| 波次 | 内容 | 验收标准 |
| --- | --- | --- |
| **1** | 纯模型层：Chrome 里无 UI 依赖的规则 | `cargo test` 覆盖 264 个 pure-logic 场景中的可移植子集；规则逐条对照 Swift |
| **2** | 纯编码层：查询应答、括号粘贴、滚轮路由、修饰键 | 字节级单测，与 Swift 期望值逐字节相等 |
| **3** | Grid 与事件：宽字符、鼠标上报、scrollback、词选 | `#[gpui::test]` 里送鼠标/滚轮事件，断言 grid 与发出的字节 |
| **4** | IME 与剪贴板：marked text、候选窗、图片粘贴 | 无头 IME 事件序列测试；真机手工验证候选窗位置 |
| **5** | Chrome 组件：侧栏、tab、状态栏、面板、overlay | `#[gpui::test]` 挂载真实 `GpuiApp`，断言投影与命中 |
| **6** | 二级窗口：设置、目标配置、composer、SSH 端口、更新提示 | 逐窗口验收 |
| **7** | 真实运行时 e2e：隔离 tmux / named Herdr 场景 | 复用 `tests/gpui_surface_e2e.rs` 的隔离模式 |
| **8** | GUI 驱动层：XCUITest 等价物 | 需要真窗口与 OS 输入，单独评估 |

前两波不依赖 gpui 窗口，是唯一能完整、快速验证的部分。第三波起需要无头 harness 挂载真实前端。

### 4.1 进展

| 波次 | 内容 | 提交 |
| --- | --- | --- |
| 1a | 编码与几何模型（42 测试） | `c4f483f8` |
| 1b | attention / sidebar / target / search / shortcuts / status bar / panel / aggregate | `c7b197b3` `e62448ed` `47b3e9f6` `4f7f5bbe` `20544279` |
| 2 | 喂字节规则：query reply、bracketed paste、滚轮路由、coalesce | `2e584660` |
| 3 | Grid 与事件：宽字符、鼠标报告、历史滚动、渐进选词 | `c734f6bc` |
| 4 | IME marked text、占位符过滤、候选窗锚定、图片粘贴阶段 | `76ea1b94` |
| 5 | Chrome：侧栏、tab 活动灯、状态栏徽章、Cmd-P 面板 | `fccdd4e2` `66c0579b` `7f668945` |
| 6 | 次级窗口：Cmd-I 输入框、设置窗口、快速连接与新建项目 | `c6990a3d` `89c3297f` `70c29c51` `236f3ee3` |
| 7 | 真机验收：隔离 tmux + named Herdr | `58789dc5` |
| 8 | 收尾：`.app` 构建验证与文档终态 | 本波提交 |

每波落地后 `cargo test --no-default-features --features gpui --lib` 必须全绿；
无头 harness 用例在 `tests/gpui_surface_events.rs`。

## 5. 测试策略

gpui 自带 `TestAppContext`，已验证可用（`tests/gpui_harness_smoke.rs`）。它能建视图、送按键、模拟点击/拖动/resize、读剪贴板、抓 opened URL。

对应关系：

| Swift 层 | Rust 手段 |
| --- | --- |
| pure-logic（264） | 普通 `#[test]` |
| model-projection（213） | `#[test]` + `ViewStore` 投影断言 |
| pixel-or-layout（267） | `#[gpui::test]` + `simulate_resize` / `simulate_click` / bounds 断言 |
| real-runtime-e2e（84） | 隔离 tmux（`-L`）或 named Herdr + 挂载前端 |
| xcuitest（22） | 需要真 GUI 驱动，本轮不覆盖 |

[tests.md](tests.md) Summary 2 列出了必须复刻的验收场景，Summary 3 列出了可以丢弃的测试及理由，Summary 4 列出了缺失的基础设施。

## 6. 明确不移植

以下测试对应的是 Swift/AppKit 内部机制，或与当前架构冲突。丢弃的是**测试**，不是用户可见结果。

| 来源 | 理由 |
| --- | --- |
| `ChromeTests/GuiLogFilePolicyTests` | 只看 stderr 重定向谓词 |
| `ChromeTests/FlatChromeTests::MuxtermConfigTests::testPoolMaxSlotsDefaultAndParse` | 前端连接池槽位设置，与 resident scene 架构冲突 |
| `ChromeTests/FlatChromeTests::TabSwitchGateTests::*` | 前端等待 Core 确认，与「切换零 Core 调用」冲突 |
| `ChromeTests/SceneStackTests` 的容量/TTL/内存压力驱逐 | 前端管理后台槽位驱逐，不应实现 |
| `AgentRenderE2ETests` 里 4 个 SwiftTerm cache/bitmap 断言 | 规定实现而非行为，改为 GPUI 侧的等价测量 |
| `I18nResourceBundleTests` 的 5 个 SPM bundle 查找测试 | 测的是 SwiftPM 打包路径，gpui 打包单独测 |

## 7. 已知的源测试缺陷

清点时发现原测试自身有弱点，移植时不要照抄断言。

| 位置 | 问题 |
| --- | --- |
| `ChromeTests/ProjectConnectFlowTests.swift:81` | `XCTAssertEqual(local.directory, local.directory)` 恒真 |
| `AppE2ETests/AgentInputRenderRegressionTests.swift:150,219,252` | 只断言 `count == 1`，所有输入行不可见也能过 |
| `AppE2ETests/NotifyBackgroundCommandE2ETests.swift:8` | 没证明通知来自后台 pane，也没做前台抑制断言 |
| `AppE2ETests/AttentionBadgeE2ETests.swift:8` | 手动调 `setAttention`，没走生产事件路径 |
| `AppE2ETests/ConnectTimeoutE2ETests.swift:8` | 等最终错误的分支没被断言 |
| `MuxtermAppUITests.swift:218` | 拖拽分隔条没有对比前后位置 |
| `MuxtermAppUITests.swift:281,730` | 断言的标记可能已出现在回显的输入里 |

## 8. 风险

| 风险 | 影响 | 缓解 |
| --- | --- | --- |
| gpui pre-1.0，版本间破坏性变更 | 升级会打断实现 | 锁定 0.2.2，升级作为独立提交并跑全量测试 |
| 无 accesskit | 无障碍能力与现状持平 | 单独立项，不在 parity 范围内 |
| 无通知接口 | 后台任务通知缺失 | 需要原生桥或自建，单独立项 |
| committed paint 需要保留渲染帧 | gpui 不暴露 | 不阻塞主路径，最后处理 |
| 43k 行规模 | 单轮无法完成 | 按波次推进，每波独立验收 |

## 9. 复核方式

```sh
# 规模
find src/frontend/macos/App src/frontend/macos/Chrome src/frontend/macos/UI \
     src/frontend/macos/Terminal src/frontend/macos/CoreBridge \
     -name '*.swift' | xargs wc -l | tail -1

# 测试方法数
grep -rhoE 'func test[A-Za-z0-9_]+' src/frontend/macos/AppE2ETests \
     src/frontend/macos/ChromeTests src/frontend/macos/MuxtermAppUITests | wc -l

# Chrome 是否全部纯逻辑
grep -lE '^import (AppKit|Cocoa|SwiftUI)' src/frontend/macos/Chrome/*.swift | wc -l

# gpui 无头 harness
cargo test --no-default-features --features gpui --test gpui_harness_smoke
```
