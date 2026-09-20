# 2026-09-20 切换、绘制和滚轮回归

检查时间：2026-09-20 14:30 +08:00。输入为本地
`test-2026-0920-1406.log`、`test-2026-0920-1411.log` 及两张用户截图。

## 日志事实与边界

- 1406：25 次 SwitchTab，51 次 ScrollPane；全部 51 次 ScrollPane 的前一条
  都是 SwitchPane，两条时间戳差的中位数为 123.710 ms。
  例如 06:07:12.777539Z focus → 06:07:12.902440Z scroll（pane 6）。
- 1411：8 次 SwitchTab；06:12:15.427748Z pane 5 Control 为 178×50，
  pane 2 为 178×23。06:12:40.682013Z pane 6 Control 为 178×50。
- 日志记录任务和 stream 尺寸，没有完整 PTY/full/diff 原始字节；不能把以下
  ANSI 夹具描述为用户截图的逐字节录制。滚轮数列直接取自日志，其余测试以
  日志中的尺寸与切换路径构造确定性帧，检查生产代码的顺序和网格。

## 已复现的失败

1. 前台 resize 批次将 full 与 output 分组，完整帧之后的 NEW_INPUT 被清掉；
   结构事件分支还可能把旧 diff 重放到较新的 full 之后。
   `SurfaceVisibilityE2ETests.testResizeBatchKeepsFrameAndDiffOrder` 修复前失败。
2. 后台 scene mailbox 没收尺寸事件，把 178×50 帧解析进 80×24；TOP/EDGE
   均丢失。`testBackgroundFrameUsesItsSourceGridBeforePainting` 修复前 4 个断言失败。
3. 每个 server wheel tick 再发一次 SwitchPane，触发同步 SSH focus。
   `AgentRenderE2ETests.testRecordedHerdrWheelBurstDoesNotQueueFocusRpc` 重放全部 51 次。
4. 每次状态栏刷新销毁 tab 按钮，working 动画从头开始。
   `ChromeE2ETests.testWorkingSpinnerSurvivesTabTitleAndSelectionUpdates` 修复前 5 次失败。

## 修复与验证范围

- 前台按原顺序处理 full/snapshot/history/output/resize；resize 前完成该 pane
  已排队的旧网格字节。后台及前台 catch-up 队列保留 resize 边界。
- ScrollPane 自身在 Core 按需切焦点，不由滚轮重复请求 SwitchPane。
- Tab 按 ID 复用按钮，working 动画保持连续，使用线性转速。
- 原生测试验证普通/全屏 cached tab 多次切换的网格与内容保持一致。
- 像素测试比较非整行局部绘制与完整绘制，避免只检查终端文本。
- 独立 named Herdr 测试以 178×50 分别分配两 tab，反复切换并检查所有
  PaneResized，禁止中途出现缩小帧；不触碰用户默认 server。

实际截图中的所有像素异常仍需用新版运行验证；上述测试证明已找到的生产路径
缺陷被修正，不代表日志中缺失的原始终端输出已被完整复现。

## 运行中日志后续追加（14:35 +08:00 读取）

以上 1411 的 8 次切换统计针对首次读取到 06:13Z 的片段；用户继续运行，日志增长。
后半段显示：

- 06:18:26Z、06:27:57Z 等多次 pane 4/6 首帧超时进入 Degraded；
  foreground=true 只重新 reconcile，Degraded 被排除，不能靠切回 workspace 恢复。
  新回归检查切回重新尝试但 takeover 次数仍为零。
- 06:26:21Z `runtime.refresh max_us=2314933`；06:34:50Z 为 1672629。
  mutation dispatch 和 snapshot probe 在该调用栈同步执行 socket RPC。
  假服务端延迟 300 ms，修复前事件泵实测阻塞 308.69 ms；新回归分别覆盖 dispatch
  和 probe。现每 Runtime 至多一个后台请求，并校验 operation/lifecycle，丢弃过期结果。

## 右侧按钮点击与数据刷新

StatusDotButton/AttentionBellButton 把 hitTest 的父视图坐标直接与自身 bounds 比较，
真实点击丢失，而 performClick 测试会绕过这个错误。改用原生命中结果并补实际坐标测试。
连接详情保留主机、状态、收发统计与错误信息，随现有每秒采样更新，支持手动刷新；
铃铛沿原入口打开 Attention。

核实时间：2026-09-20 14:35:23 +08:00（本轮机器时间基准）。
Apple 官方 `NSView.hitTest(_:)` 明确 point 在 superview 坐标系：
https://developer.apple.com/documentation/appkit/nsview/hittest(_:)
使用其官方 JSON 文档验证了该参数约定。

后续审查还发现新 pane 的 `pane.read` 索引播种和事件订阅重建也处于 refresh 调用栈。
两者同样改为 Core worker，分别用延迟 300 ms 的假 API 验证主线程调用不等待网络。
初次 attach 的连接初始化不变；异步索引结果仅进入 Index，不作为 Surface 补帧。

## 1504 日志与鼠标选区回归

检查时间：2026-09-20 15:30:33 +08:00。首次快照为 3831 行，截止
07:30:31.867048Z；之后再次读取正在增长的同一日志。

- SSH yaklang pane 19 有 67 次 SwitchPane，本地 yaklang pane 146 有 15 次。
  例如 07:06:18–07:06:37Z 连续对 pane 19 发焦点命令。选区点击即使目标
  已激活也入队；隔离 tmux 测试的 30 次点击在旧代码中增加 30 条命令。
- Herdr 有 63 次 stream start，但本次快照没有首帧 timeout / Degraded，
  也没有 Runtime/pane failed 日志。不能将上一轮超时原因套用于本次。
- 唯一错误文本为 15:07:12.832 的系统输入法
  `error messaging the mach port for IMKCFRunLoopWakeUpReliable`。
  没有对应的 Runtime 错误栈；本轮不通过屏蔽日志假装修复系统消息。
- 用户随后提供本地 muxterm 工作区的两张截图：拖选时输入区出现黑块、
  混合行，稍后恢复。日志没有该时刻完整 ANSI，以下是生产路径的确定性
  复现，不是截图字节的录制回放。

新增回归及修复：

1. `testTmuxAllocationDoesNotResizeAuthoritativeGrid`：178×50 的模型被
   `setFrameSize` 改成 36×14 / 109×38，右侧 EDGE 内容消失（旧代码 6 个
   断言失败）。SharedClientResize 的 Surface 关闭 SwiftTerm 的像素驱动
   自动网格调整，仍由 Runtime 的 PaneResized 改网格，窗口依然可自由缩放。
2. `testTopologySizeUpdateDoesNotReinterpretQueuedOutput`：拓扑快照改变
   尺寸时旧网格输出仍在队列。所有模型尺寸更新共用 feed 屏障，仅实际改变
   尺寸时排空该 pane；不因普通快照刷新排空其他 pane。
3. `testSelectionDuringSynchronizedOutputKeepsLastCompletePixels`：同步帧
   begin → 清屏/半帧 → 选区请求 draw → end。旧版选区重绘绕过
   SwiftTerm.updateDisplay 的 synchronizedOutputActive 检查，像素比较失败。
   现在每个 Surface 保留最后提交的 Core Graphics 图层，同步帧未结束时
   绘制该图层，结束后更新；普通输出仍只重画 dirtyRect。模型继续消费字节，
   不 reset、不从 Index 补图。保留已有超时释放机制。
4. `testRepeatedMouseSelectionDoesNotQueuePaneFocus`：同 pane 点击保留本地
   focus/选区，不再重复排远端焦点命令；切到另一个 pane 仍正常排队。

原有局部像素/全图像素比较也验证了增加提交图层后 partial paint 的一致性。
图层只缓存该 Surface 的最后画面，不缓存终端历史，随 Surface 销毁释放。

外部事实核查：2026-09-20 15:41:34 +08:00，Apple 官方将 CGLayer 定义为
“An offscreen context for reusing content drawn with Core Graphics.”
https://developer.apple.com/documentation/coregraphics/cglayer
通过其官方 documentation JSON 验证。

验证结果：SurfaceVisibility / AgentRender / Zoom / History 四组 71 项通过；
另加同步帧未结束时相邻 pane 继续输出、1 秒超时释放的回归 1 项通过。
所有真实 tmux 交互测试均使用独立 `-L muxterm-test-*` socket。
