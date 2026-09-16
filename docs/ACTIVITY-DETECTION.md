# Activity 识别与回归验证

## 数据流

Runtime 的前台进程事实 → Muxterm 按 WorkspaceId + PaneId 合并 → Core Index
的可见屏/OSC 标题/进度 → AttentionEngine → FFI 快照 → 前端投影。

进程身份和工作状态分别判断：Agent 进程仍在不代表正在生成；Working、Blocked、
Done/Idle 由 Core 屏幕规则或 Runtime 结构化状态决定。前端不解析 argv、不识别 Agent，
也不能把收到的进程事件再异步写回 Core。

## 2026-09-16 排查

输入：`test-2026-0916-1458.log`、此前日志，以及 Cursor 对话
`72885cc8-ee4c-4172-846b-5192d54a089a`。这些日志没有完整屏幕内容和分类依据，
不能宣称逐帧重放了当时每个 Agent；回归测试复现的是代码中确认的失败路径。

- 进程事件迟于首屏到达时，旧代码只设置身份、不分类屏幕；同批到达时，是否
  取屏幕快照又是在设置身份之前决定，导致初始 Working 被输出为 Idle。
- `latest_seq()` 是 scrollback 行号；TUI 原地重绘通常不改变它和最后一行。
  用它们跳过屏幕分类会漏掉实际状态变化。现在按完整分类输入缓存并按 pane 去重。
- 旧 argv 匹配扫描所有参数，`rg codex src`、Cursor 提示词中的 codex 都可能抢占身份。
  现在只识别可执行名或已知脚本宿主的入口；完整的普通 node 脚本不继承旧 Agent。
- tmux `#()` 的缓存 argv 不得覆盖当前明确的 shell/具名命令。
  OSC command mark 也不得覆盖 Runtime 已确认的 Agent 身份。
- macOS 两条消费路径会把进程事件异步回写 Core；现已移除整个 Swift 回写接口，
  架构检查禁止其重新出现。Agents 只显示 Core 的身份、状态和名称。
- 本地 ShellRuntime 原来不持续观察前台进程。现在通过 Transport 提供的独立
  观察器在后台每 500ms 采样，只上报变化；关闭 pane 即取消。Runtime 不拿远端 PID
  查本机，且不会在 UI/poll/输入通道锁内启动 ps。SSH 原始 shell 的观察器尚不提供，
  仍可使用 OSC 命令信号；SSH tmux 继续使用远端订阅。

## 测试契约

Core 单测覆盖 argv 正反例、陈旧缓存、状态原地更新和重复进程观察；FFI 测试覆盖
Runtime → Workspace Index → Activity → JSON，验证事件先后顺序、同批事件、命令
刻度优先级、原地重绘、跨 Workspace 同 PaneId 隔离以及退出回 shell。

ShellRuntime 测试分别走直接 PTY 和 Transport ByteChannel：在独立测试 shell 中
以 sleep 的 `codex` 别名模拟 Agent，验证启动/退出；不调用真实模型、不依赖 OSC
或 frontend。macOS `PaneCmdE2ETests` 验证移除回写后仍能消费 Core 结果，tmux 使用
独立 `-L muxterm-test-*` socket。

```sh
cargo test --no-default-features --features ffi,test-harness --lib
cargo clippy --no-default-features --features tui,test-harness -- -D warnings
bash scripts/check-architecture.sh
```

## 外部契约核验

核验时间：2026-09-16T15:10:58+08:00。
[tmux 官方手册源码](https://github.com/tmux/tmux/blob/master/tmux.1) 与
[OpenBSD tmux(1)](https://man.openbsd.org/tmux.1#FORMATS) 确认：`#()` 不等待新命令
完成，而返回此前结果/占位符；`refresh-client -B` 至多每秒报告变化，`%*` 的作用域
为 attached session 的所有 pane。修复不会把缓存结果当作当前进程的更高优先级事实。
