# 前端改进：Linux 接续实现清单

2026-09-19，分支 `feature/frontend-improvements`，基于 main `e299b580`。
本文描述本分支已完成的 macOS 行为及共享 Core 改动，作为 GTK 对齐清单；
不表示所有行为已在 Linux 验收。先检查 GTK 已有实现，再补差异。

## 共享边界

- Core/FFI 已包含 agent 进程识别、屏幕状态分类、短命令过滤、未读 Done 保留、
  muted 导出及计数、tmux 新 tab 继承项目目录、输出/重绘开销优化等改动。
- Herdr 打开逻辑覆盖本地目录展开和已运行远端 session 的 workspace 创建；
  缺失 namespace 时要求明确选择，不假定 SSH 上存在 `default`。
- Linux 复用现有 CoreBridge/FFI 与 Activity 数据，不能在 GTK 重新识别 agent、
  创建连接池或构造 WorkspaceSpec。每个 pane 保持一个常驻 Surface。
- 结构与约束以 `WORKSPACE.md`、`FRONTEND.md`、`SURFACE.md` 为准。

## GTK 对齐项目

| 主题 | 期望行为 | macOS 参考位置（`src/frontend/macos/` 下） |
| --- | --- | --- |
| Shell / Agent 聚合 | 固定置顶 S、A 两个前端视图，不占真实 Workspace 编号；Agent 按源 tab 聚合，包含同 tab 的 shell pane，复用完整布局与 Surface；记住上次 Agent tab，失效才回退 | `Chrome/AggregateWorkspaceModel.swift`、`App/MainWindow.swift` |
| 聚合操作 | Agent 视图不能创建 tab，但可以操作源 pane，包括切分、关闭、放大；隐藏投影不等于关闭源 tab | `App/MainWindow.swift`、`AppE2ETests/AggregateWorkspaceE2ETests.swift` |
| Workspace 导航 | 侧栏、快速面板使用同一顺序、标识、选中态与数据；macOS Cmd-Ctrl-A/S 与数字切换，Linux 按现有 Ctrl-Alt 约定映射；面板打开时也能快捷切换并关闭面板 | `Chrome/KeyBindings.swift`、`App/UnifiedPanelController.swift` |
| 打开与关闭 | Project 打开立即进入可切走的加载页面，不用遮住 tab 的浮层；新 tab 继承 Workspace path；快速面板可关闭 Workspace，命令面板 detach 当前 Workspace | `App/MainWindow.swift`、`App/QuickConnectController.swift` |
| 主窗口 | 终端为主体，侧栏按钮与 tab 集成到底部状态栏；左右控制区保留空间，中部显示 tab；按钮可点并展示 Workspace、Attention、连接等信息 | `App/ContentView.swift`、`UI/StatusBarView.swift` |
| Tab | 可设置等宽铺满中部或按标题宽度紧凑显示；始终提供关闭按钮；working 显示旋转标记，其他活动状态与侧栏一致 | `UI/TabBar.swift`、`Chrome/WorkspaceSidebarModel.swift` |
| 聚合外观 | Shell、Agent 各有明确图标、颜色与状态栏身份，容易看出当前处于哪个聚合视图 | `Chrome/StatusBarModel.swift`、`UI/StatusBarView.swift` |
| Pane 标题与焦点 | 单 pane 无标题栏，多 pane 有固定高度标题栏，显示 title；标题可拖动，右侧菜单支持切分/关闭；鼠标与键盘选择同步，放大始终作用于输入 pane | `UI/PaneLayout.swift`、`Terminal/TerminalView.swift` |
| 尺寸 | 标题栏从终端可用高度扣除；1→多 pane、上下/左右切分、放大中切 pane 都重新计算网格并同步 PTY；默认窗口规整，用户可持续放大缩小，tab 不反向限制窗口宽度 | `UI/PaneLayout.swift`、`Terminal/TerminalManager.swift`、`AppE2ETests/ChromeE2ETests.swift` |
| 高流量隔离 | 一个 agent 高频刷新不能饿死其他 pane 的输入与 UI；分批消费输出，限制每轮渲染工作，后台 pane 也保持正确尺寸 | `App/MainWindow.swift`、`Terminal/TerminalManager.swift` |
| 历史与选择 | 文本选择不能触发窗口拖拽；向下滚动不立即吸到底；Latest 右下角稳定可点，Last Seen 右上角仅在相关切换场景短暂出现；历史播种后恢复 live tail | `Terminal/TerminalView.swift`、`App/MainWindow.swift` |
| Attention | 共用 agent 投影，按 done、blocked、working、idle 排序/呈色；支持隐藏和静音；标题保留 workspace、process、machine、path，跨 Workspace 跳转按完整身份定位 | `Chrome/AttentionModel.swift`、`Chrome/WorkspaceSidebarModel.swift` |
| 面板与设置 | 快速浮层支持 Ctrl-N/P 上下选择；搜索 pane/workspace/all 范围切换复用结果；设置分组、导航、搜索与说明重整，界面文案走全局 i18n | `App/SettingsWindow.swift`、`App/UnifiedPanelController.swift`、`App/I18n.swift` |

## Linux 验收重点

1. 同时打开本地与 SSH Workspace，存在重复 pane 数字 ID，搜索/Attention 跳转不串台。
2. 反复从真实 Workspace 切到 A/S 再回来，Agent tab 选择、输入、布局与源 tab 一致。
3. 高频输出 pane 旁另开交互 pane，持续输入、拖动分隔线、切 tab，不丢输入或卡住全窗。
4. 单 pane 切为多 pane、上下/左右切分、放大后换 pane，检查底部提示符、换行列数与 PTY 尺寸。
5. 大量 tab、长 title 和反复缩放窗口时，中部 tab 与左右控制按钮均保持可用。
6. 后台任务 done/blocked/working、静音、隐藏与重新查看时，tab、铃铛、侧栏、面板状态一致。

GTK 构建和运行测试需要在 Linux 完成。真实 tmux 测试必须使用独立 `-L` socket；
Herdr 测试只用独立命名 session，不操作用户默认 server。
