# 2026-09-16 卡顿与 CPU 诊断

## 证据与边界

- `test-2026-0916-1556.log`：15:58:45（+08:00）开始 reader-output-lane
  gap，pane 92 随后反复 deadline；16:00:17 多次输入集中出队。
- `test-2026-0916-1600.log`：16:06:29 pane 84/8 gap，pane 8 到
  16:06:46 才完成恢复。日志不能单独区分远端响应慢和本地消费慢。
- `test-2026-0916-1616.log`：本地 shell pane 2 在 16:20:52–16:21:36
  收到 40,238 个块、32,474,820 字节；其中 27,423 个块为 1024 字节。
  在 Muxterm shell 内手动 SSH/tmux 仍然经过 ShellRuntime、Index 和 SwiftTerm，
  并不是排除了本地渲染的独立对照。
- 16:03:18 与 16:25 左右对旧版进程的短时 sample 都看到主线程 Core
  `snapshot_trimmed`；调用路径包括 Activity 尾行和 history chrome 偏移查询。
  采样不是卡死瞬间，不宣称它是全部原因。
- 16:24:36 只读 SSH 检查即时返回；远端 tmux server 的 ps CPU 约 1%，
  legion 的 pane 存活。这只是检查时的状态，不排除之前的远端阻塞。

## 修改与回归

追溯 `bdfef22e`（2026-09-16 14:56:54 +08:00）可见 Cursor 输出合并块
由小块扩大到 256 KiB；原有 pump 的 4ms/2048 事件预算只涵盖取队列，
不涵盖之后的 Index 和 Surface 工作。新测试在旧代码中复现单次交付
1,425,408 字节，超出单轮预算。

- tmux 增加 256 KiB 输出预算，允许最后一个完整事件越界；不任意切断 ANSI，
  不丢弃剩余输出，也不改变 control watermark 次序。
- ShellRuntime 原先无界追着 producer 排空。现在按 256 KiB / 1024 消息
  让出执行权，合并相邻同 pane 输出；减少逐块日志、Index feed 和事件分配。
- Activity 尾行查询只构造命中的一行；屏幕快照直接裁剪单元格尾部，避免双重
  字符串分配。history offset 只数行，不序列化屏幕；直播 viewport=0 不再计算它。
- 2000 次 240×80 尾行查询，修改前约 2.49s，修改后约 15ms；这是特定本机
  microbenchmark，不是端到端输入延迟保证。

`tests/samples/activity-stall-2026-0916.json` 保存三份日志的精简元数据，
没有 SSH 地址、提示词或键盘文本。两条 `logged_*` 测试根据受影响的 pane 集合
和 1KiB 分片构造 ANSI 洪峰，验证单轮预算和最终逐字节一致；合成输出不是原始
日志中的完整屏幕录像。已有真实 tmux 洪峰、SGR、snapshot/resync、Activity
和 Surface 回归仍须通过。不能为性能绕过 gap fence 或恢复旧的任意后缀重放。

## 异步性能监控

debug 日志启用时，Core 日志初始化启动一个独立的 `muxterm-performance` 线程。
无需前端设置任务、锁或解析状态。

- 每 2s 通过 `getrusage(RUSAGE_SELF)` 计算本进程 user+system 时间增量。
  100% 表示占满一个核，不按整机核数归一化，也不包含子进程 CPU。
- 大于 100% 时记录 `performance CPU threshold exceeded` 和阶段计时。
  阶段包括 FFI poll、Runtime refresh、Index feed、Activity batch/screen、
  进程观察；累计/最大值是 wall time，嵌套阶段有重叠，不能相加当 CPU 百分比。
  单次阶段大于 100ms，即使 CPU 没超限也记录。
- macOS 每 30s 最多一次 `/usr/bin/sample <本进程PID> 1 10`，采样 1s，
  间隔 10ms；报告包含 Rust 和 Swift 线程。sample 的实现会短暂挂起线程采样，
  并非零开销，因此限频。15s 超时只结束本次诊断子进程。
- debug 日志记录有限调用树/叶子栈摘要和完整报告路径；等待线程也会出现在
  样本中，样本次数不是 CPU 占比。私有临时目录每次运行只保留最近三份报告。
  报告可能含本机路径/函数名，不记录终端文本；不是 Go pprof HTTP 服务。
- 非 macOS Unix 提供 CPU 和阶段指标，暂不实现原生线程栈抓取。
  非 debug 模式不启动观察线程，阶段入口只有禁用检查。

手动验证：

```sh
cargo test --no-default-features --features ffi,test-harness --lib logged_
cargo test --no-default-features --features ffi,test-harness --lib attention_tail_avoids_full_screen_allocation -- --ignored --nocapture
cargo test --no-default-features --features ffi,test-harness --lib sample_current_process_without_ui_or_core_locks -- --ignored --nocapture
```

外部契约核验时间：2026-09-16T16:28:32+08:00。
[Apple getrusage(2)](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/getrusage.2.html)
确认 SELF 与 user/system 计时含义；
[Apple Performance Tools](https://developer.apple.com/library/archive/documentation/Performance/Conceptual/PerformanceOverview/PerformanceTools/PerformanceTools.html)
确认 sample 的执行采样用途。具体参数和短暂停线程行为另核对本机 `man sample`。

## 修饰方向键

真实 AppKit 事件回归复现旧版 legacy 模式下四个 Shift+方向键均发送零字节；
SwiftTerm 将它们交给 AppKit 的扩展选区命令，却未实现对应终端编码。
应用快捷键仍优先匹配，其余修饰方向键由 Surface 编码发送一次，不在窗口
monitor 中手动分发事件。文本/IME、未修饰方向键和 Kitty 模式保留原路径。
回归覆盖四个方向、七种 Shift/Option/Control 组合及 Kitty Shift 方向键，
并验证普通文字、中文提交和 Enter 不双写、Cmd-Enter 仍执行应用动作。

编码契约核验时间：2026-09-16T16:38:19+08:00；
[xterm Control Sequences](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)
定义修饰键参数 2–8（Shift/Alt/Control 组合），修饰方向键使用 CSI，
例如 Shift-Left 为 `ESC [ 1 ; 2 D`。

## 1651 日志：Codex 条目出现 / 消失

用户报告的是 Agents 整条记录消失，不是仅 Working/Idle 切换。检查时间
2026-09-16T17:38:56+08:00 起；`test-2026-0916-1651.log` 此时仍在追加。
09:27:21Z 起本地 muxterm pane 130 多次记录 `live_agent=true`；09:33 起
反复出现 Idle → Working，但旧日志不记录进程身份清除边沿，因此不能仅凭
这些行断言是排序，也不能断言输入框星星动画就是原因。

只读 `list-panes` 与本机 ps 核对：该 pane 报告 node，前台进程组 leader
是 node 启动的 Codex。身份来自进程订阅，而非星星/提示符图案。

发现并回归修复一个可真实删除 agent 身份的冲突路径：

1. 订阅当前命令为 node/npx 等 wrapper。
2. 同条 format 的 `#(ps …)` 是异步缓存，或本机两次 ps 补查遇到进程切换。
3. 旧代码不验证 argv 一致性，若取得旧 shell argv，Attention 将其视为
   agent 已退出，清掉 process_is_agent；之后 Codex argv 到来才重新加入。

现在 wrapper 只接收匹配 wrapper 家族的 argv；冲突缓存被忽略，本机可补查，
远端绝不拿 PID 查本机。信息不足时保留当前 wrapper，不伪造 shell 退出。
当前命令真正为 zsh 时仍立即移除 agent；完整 node server.js 也会释放旧身份。
合成竞态/动画回归覆盖连续 60 次更新身份不丢和真正退出，**不是日志逐帧录像**。

新增 `activity agent identity changed`，记录 workspace/pane、前后 agent 布尔值、
可执行名与 authority；不记录完整 argv/提示词。`ignored conflicting cached
foreground argv` 标记冲突缓存。需要新版日志验证此次用户现场是否命中此路径，
不能把这个已复现的缺陷当作全部消失事件的既定解释。

外部契约核验时间：2026-09-16T17:44:01+08:00，
[tmux 官方手册 FORMATS](https://github.com/tmux/tmux/blob/master/tmux.1)
明确 `#()` 不等待命令完成，会使用先前结果或占位符。
