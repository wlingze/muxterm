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

## Linux 接续记录（2026-09-19）

本轮 GTK 已接入：

- S/A 固定入口、按完整 Workspace/Tab 身份投影源布局与常驻 Surface、记忆 Agent tab；
  Agent 禁止新 tab，关闭投影仅隐藏，再选 A 可恢复。Ctrl-Alt-S/A 可直接切换。
- 侧栏与快速面板共享工作区顺序和编号；面板可切换、关闭工作区；detach 只关闭当前工作区。
- 底部侧栏按钮、可滚动 tab 中区、等宽/紧凑配置热应用、独立关闭按钮、活动标记。
- 多 pane 标题栏、菜单关闭/切分、受能力约束的标题拖动拆出 tab；标题高度参与终端尺寸计算，
  鼠标焦点同步到 pane 快捷键目标。
- 侧栏正文 14px、说明 12px；快捷面板正文 15px、说明 13px；设置正文 14px、说明 13px，
  调整间距、圆角、选中态与分隔线。终端字体继续遵循已有配置。
- Ctrl-N/P 面板导航、Attention 四态排序与静音过滤、每次定时 feed 限制 64KiB，保留全部剩余字节。

以上是首个提交的范围；下方补齐记录覆盖当时留下的异步打开、i18n、Last Seen 与集成验收。

验证：GTK chrome、pane/聚合、快速面板、设置保存、侧栏/隔离 tmux Agent 生命周期共 13 个
集成用例通过；chrome 模型 61、Surface 10、i18n 4 个定向单测通过；架构 25 项检查通过。
`cargo clippy --features gtk,test-harness --lib -- -D warnings` 通过。
`scripts/build-linux.sh` 输出 `build/linux/muxterm`；普通 gtk 构建仍有既有 shell/daemon.rs
unused-import warning，未改动无关 Runtime。

布局 API 已核对 [GTK ScrolledWindow 官方文档](https://docs.gtk.org/gtk4/class.ScrolledWindow.html)：
中部滚动容器不传播内容的自然宽度，避免长标题扩大窗口最小宽度。
本轮时间校验：`2026-09-19T15:25:35+08:00`（Asia/Shanghai）。

## Linux 完整对齐与性能补齐（2026-09-19）

- Project / Worktree / Existing / Recent 的统一打开请求经非阻塞 C FFI 执行；Core 后台仅拥有
  待连接 Workspace 和连接引用快照，不移动 resident pool，不共享裸 FFI handle。完成后在
  owner 线程收编，复用已有 Workspace。GTK 立即显示独立加载页，可切回终端，后台完成不抢焦点。
  失败保留错误说明与返回入口；同时重复打开不会创建第二个并发任务。
- Last Seen 使用完整 Workspace 身份、pane 与稳定行号，离开后有新输出才提示；显示四秒，
  临时行映射失败有一秒容错；点击或超时消费本次提示。搜索、命令刻度与 Last Seen 统一将
  Core 的距尾部偏移转换到 VTE adjustment，不再按重复命令文本或错误的距顶部偏移定位。
- 搜索 pane/workspace/all 范围复用同一次查询的结果；本地和 SSH 的重复数字 pane ID 不串台。
- 状态栏、聚合校准、命令标记、侧栏等辅助展示按 100ms 刷新，结构变化和主动导航立即刷新；
  输入、输出、拓扑与尺寸仍逐帧消费。状态栏一次刷新复用一个 Activity snapshot。
- 输出初次合并窗口维持 25ms，每次最多 feed 64KiB；积压块让回 GTK 后按 1ms 继续，避免
  每块再等待 25ms 造成限速积压。显式 flush 取消旧定时器，保留字节顺序且不 reset Surface。
- 放大中切换 pane 会更新实际显示的 leaf 与输入焦点；后台异步打开创建常驻隐藏 Scene。
- 设置分类、标题、说明、选项、项目/快捷键编辑器及 action 文案接入全局中英文 catalog；
  侧栏分类、活动状态和历史提示也使用全局文案。搜索框垂直居中，辅助文字与正文保持字号层级。
- 浅/深主题使用不同聚合强调色，改善浅色背景下对比度；细化分隔线、标签选中态、关闭按钮，
  成功/失败命令标记不再重叠。保持用户已有终端字号，不覆盖个人配置。

### 本地验证证据

- 10 个 GTK integration targets，共 20 个用例通过：chrome、context menu、panel、preferences、
  titlebar/sidebar、render、search scope、search jump、SSH、Existing Herdr。
- 新增异步打开导航用例；新增隔离 tmux 持续输出、邻 pane 逐字输入、侧栏切换、GLib 心跳与
  Last Seen 点击回看用例；SSH 测试用独立 loopback sshd 和两个隔离 tmux socket，让本地与远端
  pane ID 相同，通过真实搜索面板双向跳转验证身份。Herdr 使用测试命名服务，不操作默认服务。
- Chrome 模型 63、Surface 10、i18n catalog 4、Core 异步打开 1 个定向单测通过。
- `cargo clippy --features gtk,test-harness --lib -- -D warnings`、架构 25 项检查及 diff 检查通过。
- `bash scripts/build-linux.sh` 生成 `build/linux/muxterm`；普通 gtk 构建仍有原有
  `core/runtime/shell/daemon.rs` unused-import warning，不影响构建。
- 实际 GTK 截图：`build/linux/frontend-preview-wide.png`、`build/linux/preferences-zh-preview.png`。
  本机默认 Xvfb 屏幕只有 640×480；正常尺寸验证明确使用
  `GDK_BACKEND=x11 xvfb-run -a -s '-screen 0 1600x1000x24'`，同时保留窄窗口回归。

这些是本地 Linux/X11、隔离 tmux/Herdr 与 loopback SSH 证据，不代表远端 CI、真实 WAN SSH、
Wayland/多屏缩放或 macOS 构建已重新验证。没有推送分支。

## 实际使用反馈修复（2026-09-19）

- 快速面板取消独立的 S/A/数字按钮区：聚合入口和真实 Workspace 共用列表、搜索、
  键盘导航及选中样式；已打开目标不再以 Recent/Existing 重复显示。侧栏隐藏 Shell
  backing workspace，再为真实 Workspace 编号。Shell 青色、Agent 紫色贯穿侧栏、面板和状态栏。
- 空 Shells 经统一异步 target FFI 打开本地 shell，创建默认 Tab；“＋”定位本地 Shell
  源 Workspace 新建 Tab，Agent 仍禁止新建。启动和切换到 shell 时同步聚合身份。
- Project 保存/重载同步 Core ProjectsService 和 GTK 候选列表；保留未变更 Worktree 的
  live Workspace 关联。编辑保留 ID，新目标同名时分配不同 ID，保存失败展示错误。
- 不再把有界原始输出尾缓存当作首屏。新建 shell/tmux 明确发布空基线，attach 等待权威
  Snapshot；History 在首屏之前排队，后续 Snapshot 正常投递，不重置常驻 Surface。
- 修复每个任意字节包后追加鼠标控制序列造成的 UTF-8/CSI/OSC 截断；现在仅在完整鼠标
  模式 CSI 结束处插入。历史回填保留原生光标/模式，并在清可见屏之后清旧历史，避免
  输入框跳行以及旧尾屏排在最早历史前面。
- 打包并在进程内注册 JetBrains Mono 和 Noto Sans Symbols2。后者补 Braille 字形，
  修复 Astra 星光显示为整格空心点；不安装系统字体、不改用户字号。来源与许可证见
  `assets/fonts/README.md`。

验证：7 个 GTK integration targets 共 16 个用例通过（panel、render、context menu、
titlebar/sidebar、attach history、SSH history、config hot apply），覆盖 Project 保存后
实际打开、Shell 默认/新增 Tab、逐字节中文/符号/CSI、输入框原位更新与 SSH 历史回看。
tmux backend 145、terminal 27、Project store 4 个单测通过；Clippy、架构 25 项检查及
`scripts/build-linux.sh` 通过。普通 gtk 构建仍有上述既有 unused-import warning。
截图：`build/linux/quick-panel-preview.png`、`build/linux/symbol-render-preview.png`。

核对时间 `2026-09-19T17:54:34+08:00`：
[Codex 官方 sparkle_field.rs](https://github.com/openai/codex/blob/main/codex-rs/tui/src/bottom_pane/chat_composer/sparkle_field.rs)
中的 DOTS 使用八个单点 Braille 字符，不是方块。用户真实 yaklang/muxterm/timepulse
会话尚未用新二进制重新打开验收；没有停止或重启用户默认 tmux/Herdr 服务。

## 重启反馈与真实打开路径回归（2026-09-19 晚间）

- 侧栏用实际分配尺寸变更更新折叠分割位置，不再监听不存在的 `height` 属性通知；
  Commands / Hidden Commands 折叠后留在底部，Workspace/Agents 使用剩余高度。
  S/A 和普通 Workspace 共用中性行/徽标/选中样式，不再单独染色。
- 历史插入恢复当前屏时，从 VTE 自身 HTML 导出恢复颜色和常用文字样式，不再用纯文本
  重画可见屏；插入历史先恢复默认 SGR，避免继承 Pi 当时的黄色。回归测试要求在应用
  再次重绘之前就保留原屏 RGB 色。这里只修当前屏的回填恢复；既有离屏 PaneHistory
  协议仍为纯文本，没有扩展跨平台的彩色历史协议。
- 隐藏 SSH 场景继续合并并按序消费字节，不再 pause 后以 RequestPaneSnapshot 追帧；
  避免聚合场景切换引发日志里的反复 `frontend-surface-overflow` 和重新抓屏。
- tmux Project 经指定 socket 发现已有同 session/name 目标后 attach；确实缺失才创建。
  未填写 session 时使用 Project 名称，命名创建也传首个 pane 的 cwd。修复被旧测试漏掉的
  `panel-project|/home/wlz`，现在严格断言 `panel-project|/tmp` 以及重开不增加实例。
- Herdr 明确区分已有和新建，不在 attach 后重套创建模板；指定 session/socket 时直接
  查询目标服务，失效的显式 workspace ID 不退回同名创建。本地 named socket 和发现路径
  遵循 XDG_CONFIG_HOME。编辑 Project 保留 session/socket/workspace ID，并提供可选身份字段。

验证覆盖：tmux Project 创建/关闭/重新 attach；Herdr Project 本地和 loopback SSH
创建/输入/关闭/重新 attach（检查 cwd 和服务端 Workspace 数量）；Existing 面板真实点击
Herdr；折叠后的实际高度、颜色恢复、SSH 历史、连续输出及相邻输入。
9 个 GTK targets 的 18 个用例及单独 GTK 编辑器/快捷键综合用例通过；Catalog 36、tmux
backend 146、terminal 28、i18n 4、Herdr identity 4 个用例通过。

VTE 导出接口与 HTML 标签核对：
[get_text_format](https://gnome.pages.gitlab.gnome.org/vte/gtk4/method.Terminal.get_text_format.html)、
[官方实现](https://github.com/GNOME/vte/blob/master/src/vte.cc)，时间
`2026-09-19T20:57:16+08:00`。隔离测试不替代用户真实远端验证。
最新用户日志 `test_2026-0919-2025.log` 仍报告字体文件缺失；分发时必须保留完整
`build/linux/assets/fonts`，不能只复制可执行文件后依赖开发机源码路径。

## Codex 黑底输入框（2026-09-19）

- tmux 的颜色能力原先默认 false 且未执行探测，用户日志因此一直跳过 OSC 10/11
  上报。改为现有控制通道查询 server version，未知时暂存最新颜色，旧版本不发无效命令。
- Linux 按 workspace + pane 排队上报，覆盖新 tab、后台 workspace 和主题切换，
  不再经 active-workspace FFI 同步上报到错误目标。
- 旧 agent 仍可能保留黑色输入框：Linux Surface 对默认前景与显式背景冲突增加显示侧
  对比度保护，正常显式文字色不改，背景/隐藏属性不改，不写回 tmux 色板。
  只维护属性而非另一份屏幕，完整 CSI 边界插入，跨包 UTF-8/OSC 保持原样。
- 验证：147 个 tmux backend 测试、4 个对比度测试、真实 VTE HTML 黑底白字/恢复默认色；
  GTK Project 测试内实际 OSC 查询验证新 pane 和后台 workspace 黑白主题往返。
  Clippy lib `-D warnings` 通过。尚不等于 archmini 上用户已有 Codex 会话的人工验收。

本轮官方资料核对（时间基准 `2026-09-19T21:51:34+08:00`）：
[tmux refresh-client -r](https://github.com/tmux/tmux/blob/master/tmux.1)、
[sRGB relative luminance](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html)。
对比度阈值是终端可读性保护策略，不声称整个终端达到 WCAG 合规。
