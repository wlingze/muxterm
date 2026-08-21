# macOS tmux 卡顿、画面冻结与前后台不一致根因分析

> 调查时间：2026-08-21（Asia/Shanghai）  
> 证据日志：`test-2026-0821-1328.log`、`test-2026-0821-1642.log`  
> 对照实现：本地 tmux `control.c`、iTerm2 `TmuxGateway.m` / `PTYSession.m` / `TmuxController.m`  
> 本文只做问题与根因分析，不包含代码修改。按要求排除 Yazi/TRT 问题。

## 1. 结论

当前最严重的问题不是某一条 ANSI/SGR 解析错误，也不是 tmux 后台状态错了，而是 **Muxterm 的 tmux 控制流读取架构失速**：

- 输入命令通过独立的无界发送队列很快写入 tmux，tmux 和 shell 已经执行；
- tmux 返回的 `%output`、结构通知和命令响应，却必须等 macOS 主线程定时调用 `pollEvents()` 才继续消费；
- 大量 pane 输出和 `capture-pane -S -10000` 的逐行响应会迅速填满只有 32 项的事件 channel，后台 reader 随后停止读取 tmux；
- tmux 控制协议为保证顺序，会让未写完的 `%output` 阻挡其后的非输出通知；因此窗口切换确认、命令响应、pane 状态恢复一起被堵住；
- Muxterm 此时仍允许继续发送按键，所以后台 tmux 继续向前运行，前台画面却停在旧状态，最终形成用户看到的“Muxterm 和 tmux 完全对不上”。

核心故障链如下：

```text
AppKit 输入
  → 无界命令队列
  → tmux stdin
  → shell/TUI 立即执行

tmux control stdout
  → 后台 reader
  → 32 项有界事件 channel
  → macOS 主线程 60 Hz pollEvents
  → Rust 每轮最多 2048 项 / 4 ms
  → FFI 每次最多返回 64 项
  → SwiftTerm feed / GUI 更新

capture-pane 上万行逐行占用 channel
  → reader 停读
  → tmux 后续通知按协议顺序被阻挡
  → GUI 落后几十秒或永久停住
```

因此，以下现象不是互不相关的零散 bug，而是同一个控制流故障的不同表现：

1. Muxterm 卡顿；
2. 后台 tmux 已执行，前台仍显示旧内容；
3. 切 tab 很久才确认；
4. Cmd-D 后 OpenCode 已退出，GUI 仍停在 OpenCode；
5. OpenCode 的终端模式和鼠标状态没有正确恢复，滚轮无效；
6. 自动 resync/capture 反过来制造更多堵塞。

此外还有三个独立问题：macOS 输入/action 重复投递、命令面板 Backspace 被主窗口键盘监视器误吃、Project/new tab 的工作目录语义在不同层丢失。

## 2. 问题分级

| 优先级 | 问题 | 当前结论 |
|---|---|---|
| P0 | Muxterm 严重卡顿 | 已定位：控制协议读取依赖 UI poll，并被小型有界 channel 和大响应堵塞 |
| P0 | 前台 Muxterm 与后台 tmux 状态不一致 | 已定位：写入继续、读取停滞造成的单向进度分叉 |
| P0 | Cmd-D 后 GUI 仍停在已退出的 OpenCode | 已定位：resync 事务无超时，live output 被永久扣留 |
| P0 | 一次操作执行两次 | 已证明在进入 Rust/tmux 前已经产生两次调用；AppKit 侧唯一入口尚未闭环 |
| P1 | OpenCode 不能上下滚动 | 高置信度：Surface 没恢复 TUI mouse/alternate-screen 状态；又受卡死 seed 放大 |
| P1 | 命令面板无法 Backspace 删除 | 高置信度：主窗口全应用 key monitor 错误消费独立 Panel 的事件 |
| P1 | Project/new tab 落在 HOME | 已定位：远端 `~` 被单引号阻止展开；`Task::NewTab` 又直接忽略 `workdir` |

## 3. P0：卡顿和前后台不一致

### 3.1 日志已经证明 GUI 落后几十秒

`test-2026-0821-1642.log` 使用 UTC 时间（文件名是本地时间），其中：

- `08:43:06.821Z` 已发送 `select-window -t @5`；
- 到 `08:43:34.718Z` 才处理对应的迟到切换确认，延迟约 28 秒；
- `08:43:12.314Z` 已发送 `select-window -t @7`；
- 到 `08:44:18.739Z` 才开始为 @7 的 pane 做 initial seed，延迟约 66 秒；
- `08:44:18.739Z` 和 `08:44:18.740Z` 发出两个 `display-message` 后，直到 `08:45:06.364Z` detach，日志里都没有继续进入对应 capture 完成路径。

这不是一般的“渲染慢一帧”，而是控制状态机已经落后真实 tmux 数十秒。

### 3.2 写入和读取完全不对称

`TmuxRuntime` 的命令发送使用独立的 `mpsc::UnboundedSender<String>`。按键一旦进入 Rust，就可以继续排队写入 tmux。tmux 收到 `send-keys` 后会立即把字节交给 pane 中的 shell/TUI。

相反，tmux 返回事件的消费发生在 `TmuxRuntime::pump_events()`，而它并不是一个持续运行的后台协议泵。macOS 主线程的 60 Hz Timer 调用 `pollOnce()`，再经 FFI `muxterm_poll_events()` 触发 Workspace `refresh()`，最后才会进入 `pump_events()`。

单轮还有多重预算：

- Rust `pump_events()`：最多 2048 个事件；
- Rust `pump_events()`：最多占用 4 ms；
- Swift `CoreBridge.pollEvents()`：一次最多取 64 个 `StateChange`；
- Swift 解析、布局和 SwiftTerm feed 也都在同一主线程批次中完成。

这些预算本意是防止输出洪峰独占 UI 线程，但它们同时把 **tmux 控制协议能否继续前进** 绑定到了 UI 是否有空。预算限制的是 UI 工作没有问题；限制协议 reader 排空则是架构错误。

### 3.3 `capture-pane` 响应按行进入 32 项 channel

`TmuxClient` 的默认 `event_buffer` 为 0，创建 channel 时使用 `max(32)`，实际容量只有 32。

命令响应中 `%begin` 到 `%end` 之间的每一行都被转换成一个独立的 `TmuxEvent::ResponseLine`，并使用异步 `send(...).await` 写入这个 channel。

普通查询只有一两行时问题不明显，但当前恢复路径会发送：

```text
capture-pane -e -p -S -10000 ...
capture-pane -a -e -p -q -S -10000 ...
```

一次响应可以有数千到上万行。第 33 个尚未消费的事件就足以让 reader 等待；reader 一旦等待，就不再从 tmux PTY 读取后续字节。

这里的问题不是“32 太小”这么简单。即使扩大到 256 或 4096，只要响应仍以“每行一个跨线程事件”表示，大 capture 仍会迟早占满队列。根本错误是：

- 命令响应的传输单位选成了“行事件”，没有在 reader/协议层聚合为一个有界 response block；
- 高频 pane output、结构通知、响应正文和响应边界共用一个 FIFO；
- 队列没有按控制消息、结构消息、pane 数据做隔离和公平调度。

### 3.4 tmux 的顺序保证会放大 Muxterm 的停读

tmux 本地参考源码 `control.c` 明确说明：一个 `%output` block 未写完时，会阻挡其后的非 `%output` block，以保证控制协议的全局顺序。

这意味着 Muxterm 不能假设“即使输出很多，后面的 `%end` 或 `%session-window-changed` 还能先到”。只要客户端不持续排空，以下内容会一起被堵住：

- pane 实时输出；
- `select-window` 的结构确认；
- `display-message` 的响应；
- `capture-pane` 的 `%begin/%end`；
- layout、window、session 等通知。

因此，日志中的几十秒 tab 确认延迟，与 pane 画面卡住是同一个原因。

### 3.5 为什么会出现“tmux 执行两次，Muxterm 只显示一次”

Muxterm 并不是 tmux 状态的同步镜像，而是在这一时刻变成了一个严重滞后的观察者：

1. 第一次输入已经发送并被 tmux 执行；
2. 对应 echo/输出还堵在控制流上，GUI 没显示；
3. 用户以为没有执行，再次输入；
4. 第二次输入同样立即进入 tmux；
5. GUI 很久以后只处理到部分输出，或被 snapshot 替换，看起来像只执行了一次；
6. 实际 shell 历史和副作用已经发生两次。

所以“前后台不一致”不是 tmux 内部出现两个世界，而是 **命令面继续向前，显示面失去实时性**。这会破坏 shell 语义，确实属于完全不可用级别的问题。

## 4. P0：resync 把临时拥堵固化为永久冻结

当前 pane snapshot/resync 大致执行：

```text
删除该 pane 尚未交给前端的 PaneOutput
  → 标记 resyncing
  → 可选 pause client
  → display-message 查询终端模式
  → capture primary（最多 10k 行）
  → capture alternate（最多 10k 行）
  → 生成 PaneSnapshot
  → 清除 resyncing 并继续交付 live output
```

这个状态机有三个关键缺陷。

### 4.1 没有 deadline、watchdog、取消或代际控制

resync 只有在以下路径才结束：

- 收到完整 `%end` 并完成两份 capture；
- 收到明确 `%error`；
- 发送命令本身立刻失败。

控制流已经积压、reader 停读时，这三种情况都可能不发生。当前没有：

- 超时时间；
- pending query watchdog；
- pane 进程状态变化后的取消；
- 新一代 snapshot 取代旧一代 snapshot 的 generation；
- 超时后交付暂存 live output 的 fallback。

因此一次暂时积压可以让 pane 永久留在 `resyncs` 中。

### 4.2 resync 期间主动扣留 live output

进入 resync 后，该 pane 新到达的 `%output` 不再生成正常 `PaneOutput`，而是暂存在 resync transaction 的 `live` / `post_capture` 缓冲中；开始 transaction 时还会删除已排队但未交付的旧 `PaneOutput`。

这个设计要求 snapshot 必须最终成功，否则前端既拿不到旧增量，也拿不到后续 live 增量。它把“恢复操作成功”变成了 pane 继续显示的唯一条件，却没有给恢复操作加期限。

这直接解释了：

- Cmd-D 已经发到 tmux；
- OpenCode 在后台已经退出，pane 回到 shell；
- 退出后的 shell 输出进入被扣留的 live buffer；
- snapshot 查询又被前面的控制流积压饿死；
- GUI 永久保留最后一张 OpenCode 画面。

### 4.3 snapshot 尺寸截断也不安全

snapshot 超过 `MAX_PANE_OUTPUT_BYTES` 时，当前实现直接从任意字节偏移保留最后一段。这个偏移可能落在：

- UTF-8 多字节字符中间；
- CSI/OSC/DCS 转义序列中间；
- 一次终端模式切换中间。

随后前端会 reset SwiftTerm 并 feed 这段截尾数据。它不是本次“永久卡死”的主因，但属于恢复路径中的额外数据完整性风险，也可能制造无法稳定复现的解析/渲染异常。

## 5. P0：旧恢复策略形成 pause/capture 正反馈风暴

`test-2026-0821-1328.log` 中共有：

- 114 次 `paused pane and requested authoritative state/capture`；
- 127 次 primary `capture-pane -S -10000`；
- 114 次 alternate `capture-pane -a ... -S -10000`。

这形成了明显的正反馈：

```text
pane 输出多
  → UI/事件队列积压
  → Muxterm 判定 backlog 并 pause/resync
  → 同时抓 primary + alternate 两份最多 10k 行历史
  → 控制流数据量进一步暴涨
  → reader/channel/UI 更堵
  → 再次触发 pause/resync
```

恢复机制本应降低压力，当前实现却在最拥堵时主动向同一条有序控制流注入最大的数据查询。这是设计方向上的错误，不是调几个阈值可以解决的问题。

## 6. P0：输入和 UI action 重复投递

### 6.1 已经确定的事实

`test-2026-0821-1642.log` 中：

```text
08:43:28.263428Z  send-keys -t %18 -H 0d
08:43:28.272895Z  send-keys -t %18 -H 0d
```

`l` 和 `s` 各发送一次，但 Enter 在约 9.5 ms 内发送两次。

同一日志中还有：

```text
08:44:01.395354Z  select-window -t @7
08:44:01.402821Z  select-window -t @7
```

两次 `execute task SwitchTab { target: TabId(7) }` 和两次 `select-window` 相隔约 7.5 ms。

由此可以确定：

- 不是 tmux 把同一条 `send-keys` 自己执行了两次；
- 不是 Rust sender 在一次 `Task::WriteRaw` 中重复写入；
- Rust 日志已经收到两个独立的上层调用；
- 重复发生在 FFI 之前的 macOS/AppKit/SwiftTerm/UI action 路由层。

### 6.2 目前不能伪装成已经确定的部分

现有静态代码不足以断定唯一函数：

- `MuxTerminalView` 没有同时重写 `keyDown` 和 `insertText`，代码意图是只走 SwiftTerm 的 NSTextInputClient 路径；
- tab button 的单次 action 路径从静态阅读看也只显式调用一次；
- 键盘 Enter 重复和 tab action 重复也未必是同一个入口造成的。

因此当前准确结论是“范围已经限定到 macOS 事件/action 层，但唯一重复入口尚未闭环”，不能武断写成 SwiftTerm 或某一个 `mouseDown` 的锅。

要闭环必须在 AppKit 边界记录同一个事件的：

- `NSEvent.eventNumber`；
- timestamp；
- window 和 first responder；
- 回调来源（local monitor、menu key equivalent、SwiftTerm delegate、button action）；
- 最终 FFI task id。

只有这样才能区分“系统真的给了两个 event”和“一个 event 被两个入口消费”。

## 7. P1：OpenCode 不能滚轮

当前 `MuxTerminalView.scrollWheel()` 会临时把 `allowMouseReporting` 设为 `true`。但这个布尔值只代表“如果终端模型认为应用启用了 mouse protocol，就允许转发”，它不会主动把 SwiftTerm 的内部 `terminal.mouseMode` 恢复成开启状态。

后台 pane 的轻量 capture 只能恢复网格文本，不能天然恢复：

- alternate screen；
- mouse tracking mode（1000/1002/1003/1006 等）；
- bracketed paste；
- cursor/keypad mode；
- Kitty keyboard mode 等协议状态。

这些模式需要从连续原始字节流建立，或在一次可靠 seed 中显式重放。当前完整 seed 又会被上述控制流积压延迟甚至永久卡住。

所以 OpenCode 滚轮问题的高置信度原因是：

1. GUI 看到的是 capture 出来的旧网格，不是已经同步完成的终端状态；
2. SwiftTerm 模型没有恢复 OpenCode 开启的 mouse mode；
3. 临时设置 `allowMouseReporting=true` 仍无法生成正确 mouse report；
4. pane 退出后因为 resync 卡死，GUI 连回到 shell 的模式变化也看不到。

它主要是 seed/resync 和终端状态恢复失败的派生问题，不应先当作一个孤立的滚轮 handler bug 修。

## 8. P1：命令面板 Backspace 无法删除

主窗口安装了一个全应用 `NSEvent.addLocalMonitorForEvents(matching: .keyDown)`。它没有先限定：

- `event.window` 是否为主终端窗口；
- `NSApp.keyWindow` 是否为主窗口；
- 当前 responder 是否属于主窗口的 view tree。

Backspace 分支检查的是主窗口保存的 `window?.firstResponder`。命令面板是独立 `NSPanel`；搜索框正在输入时，主窗口仍可能保留一个 `MuxTerminalView` 作为自己的 first responder。于是：

1. 用户在 Panel 输入框按 Backspace；
2. 全应用 local monitor 先收到事件；
3. 代码看到主窗口保存的 responder 是 terminal；
4. 把 Backspace 转成 DEL 发给 tmux，并返回 `true` 消费事件；
5. Panel 的文本框永远收不到删除事件。

这是事件所有权边界错误：主窗口快捷键路由器处理了另一个窗口的文本编辑事件。

另有生命周期缺陷：安装 monitor 后没有保存 token，也没有在控制器销毁时 remove。它会增加多实例或窗口重建后的重复路由风险，但现有证据不能直接把它等同于 Enter 双发的唯一原因。

## 9. P1：Project 和新 tab 没有进入选中目录

这里实际有两处不同的工作目录丢失。

### 9.1 SSH Project 的 `~` 被错误引用

日志中生成过：

```text
tmux new-session -d -s 'timepulse' -c '~/Project/self/timepulse'
```

本地创建路径会先展开 `~`；SSH 创建路径则直接对目录调用 `shell_quote(directory)`，得到单引号包裹的 `'~/...'`。POSIX shell 不会展开单引号中的 `~`，远端 tmux 收到的是字面路径，而不是远端用户 HOME 下的目录。

同样的 quoting 问题也存在于远端目录探测命令中。

### 9.2 `Task::NewTab` 直接丢弃 `workdir`

tmux backend 当前匹配：

```text
Task::NewTab { name, .. }
```

其中 `..` 把 `workdir` 完全忽略，最终只构造：

```text
new-window -t $8
```

没有 `-c <selected-directory>`，也没有先查询当前 pane 的 `#{pane_current_path}`。这与日志里的两次 `DEBUG new-window cmd="new-window -t $8\n"` 完全一致。

产品语义在这里没有贯穿：用户选择 Project 目录后，这个目录应该成为新 session/project 的明确上下文；在同一 Workspace 新建 tab 时，至少应该有清晰且一致的规则——继承当前 pane cwd，或继承 Project root。当前实现是在 Task 层携带了 `workdir`，但 tmux adapter 直接把它丢掉。

## 10. 与 tmux 和 iTerm2 对照后的设计问题

### 10.1 tmux 要求控制客户端持续排空

tmux `control.c` 的队列设计明确以协议顺序为优先：未完成的 `%output` 会阻挡后续非输出 block。Muxterm 当前却把控制流进度挂在主线程 Timer 上，违反了这个协议的运行前提。

### 10.2 iTerm2 把协议排空和终端渲染解耦

iTerm2 的结构与当前 Muxterm 有几个关键差异：

- `TmuxGateway` 持续消费和解析控制协议，而不是等 UI 每帧来推动协议 reader；
- `%output` 解码后经 pane 专用路径交给 `PTYSession` / `TaskNotifier`，终端解析和绘制不会阻塞 gateway 继续理解控制边界；
- 命令响应有明确队列，并与 `%begin/%end/%error` flags 配对；
- pending 命令有 unresponsive watchdog，不会无限等待而不改变状态；
- pause 后的屏幕恢复由专门的 opener/resume 流程负责，不会在同一拥堵 FIFO 中无限叠加自动 capture。

关键不在于照抄 Objective-C，而在于它守住了三个边界：

1. 控制协议 reader 必须持续前进；
2. pane 大数据不能阻塞结构与响应状态机；
3. 所有等待远端响应的事务必须有明确失败出口。

## 11. 当前实现违反的核心设计原则

### 11.1 控制面和显示面没有解耦

`pump_events()` 同时承担协议推进、状态更新、输出排队，而它只能被 UI poll 驱动。控制面无法独立维持 tmux 的真实状态。

### 11.2 所有事件共用一个 FIFO

高频 `%output`、上万行响应、结构通知和响应边界相互阻塞。没有独立 lane、优先级、公平性或按 pane 的流控。

### 11.3 背压施加在错误位置

UI 慢时应该限制 pane 数据进入渲染队列，必要时使用 tmux 的 pane pause；不应该让控制协议 reader 停读，因为 reader 停读会连结构事件和命令响应一起冻结。

### 11.4 snapshot 是无限期事务

任何“先停止 live，再等待远端 snapshot”的设计，都必须具备 deadline、cancel、generation 和 fallback。当前四项都缺失。

### 11.5 恢复流量没有成本控制

在 backlog 时自动抓两份 10k 历史，会把恢复机制变成负载放大器。

### 11.6 UI 事件路由没有窗口所有权

主窗口 local monitor 能消费独立 Panel 的文本编辑事件；monitor 生命周期也没有受控制器管理。

### 11.7 工作目录契约没有贯穿层级

Project path、Task `workdir`、远端 shell quoting 和 tmux `new-window -c` 没有形成一致契约，导致 UI 明明有选中目录，runtime 最终却没有使用。

## 12. 不是根因的项目

为避免后续再次走偏，以下内容不能被当成这次 P0 的主修复：

- **不是 tmux 随机执行一条命令两次。** 日志里确实有两条独立 `send-keys`。
- **不是单纯 SwiftTerm 渲染慢。** 在进入 SwiftTerm 前，控制事件已经落后几十秒。
- **不是把 Timer 从 60 Hz 改成更高就能解决。** 错误在于协议推进依赖 UI，不在于 60 这个数字。
- **不是把 channel 从 32 扩到几千就能解决。** 逐行大响应最终仍会填满，只是晚一点发生。
- **不是把 4 ms/2048/64 三个预算简单调大就能解决。** 这可能让主线程更卡，也不能消除队头阻塞。
- **不是多做几次 capture 就能追上。** 当前日志已经证明 capture 风暴只会加重拥堵。
- **不是 Yazi/TRT 问题。** 本文按要求不处理 Yazi；即使完全不运行 Yazi，控制流架构仍会产生同样的卡顿和分叉。

## 13. 后续修复的依赖顺序

本文不是施工计划，但从根因依赖看，顺序不能颠倒：

1. **先让 tmux 控制协议持续排空，并与 macOS UI 帧解耦。** 这是所有问题的地基。
2. **拆分控制/结构/响应与 pane 数据通道；大响应按 block/bytes 管理。** 消除队头阻塞。
3. **重做 resync 的 deadline、cancel、generation、fallback 和成本边界。** 保证任何失败都不会永久扣留 live output。
4. **定位并消除 AppKit 输入/action 双投递。** 一次物理操作必须只产生一个 FFI task。
5. **在可靠 seed 上恢复 alternate screen 和 mouse mode。** 然后再验 OpenCode 滚轮。
6. **收紧命令面板事件所有权，并贯通 Project/new tab 的 cwd 契约。**

在第 1～3 项完成之前，先调滚轮、重绘、Timer 或 capture 参数都只能掩盖症状，无法让 Muxterm 恢复可用。

## 14. 修复后的最低验收标准

- pane 持续高输出时，tmux 控制 reader 仍持续前进，结构通知不能被 pane 数据永久饿死；
- `select-window` 的确认不能再出现 28～66 秒延迟；
- 输入后后台 tmux 与前台 Surface 保持同一有序字节流，不能依靠重新 capture 才偶尔追上；
- Cmd-D 退出 OpenCode 后，GUI 必须可靠显示 shell，任何 snapshot 失败都不能永久冻结旧画面；
- 一次 Enter、一次 tab 点击只能产生一次 Rust task 和一次 tmux 命令；
- OpenCode mouse mode 恢复后，滚轮产生正确的 mouse report；退出 TUI 后恢复 shell scrollback 行为；
- 命令面板的 Backspace 只编辑面板输入框，不向后台 terminal 发送 DEL；
- Project 创建和新 tab 均遵守明确的 cwd 规则，远端 `~/...` 必须按远端 HOME 语义解析。

## 15. 最终判断

这次故障的主线可以概括为一句话：

> Muxterm 把“持续读取 tmux 控制协议”错误地做成了“GUI 每帧有空时顺便拉一点事件”，同时又把上万行 capture 响应逐行塞进一个 32 项 FIFO；写命令不受这个限制，于是后台持续执行、前台停止观察，随后无超时 resync 把暂时落后变成永久旧画面。

这是控制流、背压和恢复事务的架构问题。输入双发、Panel Backspace、Project cwd 是另外三处实现边界错误；它们需要修，但不能替代 P0 控制流架构的修复。
