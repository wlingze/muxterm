# 待接协议

> 只记录，还没实现。核对时间：2026-10-14T11:00:03+08:00（CST）。
> Runtime 词表：[`RUNTIME.md`](RUNTIME.md)。1.0 范围：[`PRODUCT-1.0-REMAINING.md`](PRODUCT-1.0-REMAINING.md)。

用户要求把下面两类补上。实现前先写清楚和 tmux 的差别，避免把 Herdr 侧信道当成 tmux 没有的一整套新功能。

## 1. Herdr 观察流丢掉的消息

`src/core/runtime/herdr/observe.rs` 只消费 `Terminal`、`Shutdown`、`MouseCapture`。
协议 19 的其余 `ServerMessage` 在 `wire_compat.rs` 里落到 `Ignored`。

tmux **没有**同样一份侧信道清单。tmux 控制模式的通知更多，而且 muxterm 已经解析
（`src/core/runtime/tmux/protocol.rs` 的 `Message`）：`output`、`extended-output`、
`layout-change`、`window-add/close/renamed`、`window-pane-changed`、
`session-changed/renamed`、`session-window-changed`、`sessions-changed`、
`pane-mode-changed`、`unlinked-window-*`、`subscription-changed`、`pause`、
`continue`、`exit`，以及 `%begin/%end/%error`。

铃、标题、剪贴板、桌面通知在 tmux 里**不单独成消息**，而是夹在 `%output` 字节里。
Core 的 VT（`src/core/protocol/terminal/emulate.rs`）已经从这些字节里取出：

| 字节里的序列 | 现有产品语义 |
| --- | --- |
| BEL | `AttentionSource::Bel` |
| OSC 0 / 2 | pane 标题 |
| OSC 52 | 待写入系统剪贴板的文本；查询 `?` 忽略 |
| OSC 9（非 `9;4`）、OSC 99、OSC 777 notify、OSC 1337 `RequestAttention=yes/once/fireworks` | `AttentionSource::OscNotify` |
| OSC 9;4 | 进度，只留给屏幕规则，不当成通知 |

Herdr 的 VT 把其中一部分拆成独立消息。所以「支持 Herdr 这些消息」多数是接到**已经有的**
产品事件上，不是新造一套。图形是例外。

| Herdr 消息 | tmux 里对应什么 | muxterm 今天 | 以后接到哪 |
| --- | --- | --- | --- |
| `TerminalBell` | `%output` 里的 BEL | Herdr 路径丢掉；tmux 路径已是 Attention | 同一条 `AttentionSource::Bel` |
| `WindowTitle` | `%output` 里的 OSC 0/2，或 `#{pane_title}` 订阅 | Herdr 路径丢掉 | 现有 pane 标题，不发明产品 Window |
| `Clipboard` | `%output` 里的 OSC 52 | Herdr 路径丢掉 | 现有 OSC 52 写剪贴板 |
| `Notify`（Sound / Toast / SystemToast） | `%output` 里的 OSC 9 / 99 / 777 / 1337 | Herdr 路径丢掉 | Attention / 系统通知；先看 kind，不要把声音当 toast |
| `KittyKeyboardReportAll` | tmux extended-keys / kitty keyboard，在字节流里 | 丢掉 | 键盘状态，不另做 UI |
| `PrefixInputSource` | 无。Herdr 前缀键切输入法 | 丢掉 | 只在 Herdr prefix 时有意义 |
| `ReloadSoundConfig` | 无 | 丢掉 | 无界面可接，可继续忽略 |
| `Frame` | 无。语义帧；我们协商的是 `TerminalAnsi` | 丢掉 | 正常不应出现；出现要打日志，不要静默 |
| `Graphics` / `GraphicsFile` / `GraphicsTransmissionRetired` | **没有**控制消息。程序若发 kitty / iTerm / sixel，字节在 `%output` 里，由前端 VT 自己画 | 丢掉。herdr 0.8.0 默认 `[experimental] kitty_graphics = false`，默认配置下也不发 | 和 [`PRODUCT-1.0-REMAINING.md`](PRODUCT-1.0-REMAINING.md) 的「不渲染 Sixel/Kitty、不做内嵌 bitmap」冲突。用户要求补齐，但实现前要单独定呈现，不能顺手塞进 Attention |
| 客户端 `ClipboardImage` | 无。tmux 没有「把图片交给 server 再贴进 pane」 | muxterm 不发。现在是上传文件再把路径写入 pane | 另案。herdr 自己的图片粘贴和「路径粘贴」不是同一条路 |

`Graphics*` 只在打开 `kitty_graphics` 时才有意义。2026-10-13 在隔离的 herdr 0.8.0
session 里，`TerminalAttach` 和 `AppDirectGraphics` 都没有收到 `GraphicsFile`。

## 2. OSC 7501 Program Status Protocol

程序用一条 OSC 告诉终端自己在干什么：空闲、工作、等用户、完成或失败，外加一行原因。
协议只共享状态，不规定铃、角标或焦点。它从 Superlogical（Rex）和 Ghostty 的工作里长出来，
规范本身不带产品名。

提出人是 Mitchell Hashimoto。容易和别的提案混：

- **就是这一条**：OSC 7501。不是 Otty 的 OSC 26 Terminal Agent Protocol。
- 也不是 OSC 9;4 进度条。9;4 只描述「还在跑、百分之几」；7501 描述「在干什么、要不要人」。

### 规范（revision 0.4，2026-10-10）

核对来源（同一时刻）：

- Mitchell 的说明：<https://mitchellh.com/writing/program-status-osc7501>（文内日期 2026-10-06）
- Superlogical Rex 文档：<https://www.superlogical.com/rex/docs/build/program-status>
- 提案页：<https://control-codes.page/proposals/osc-7501/>
- 规范文本：<https://gist.github.com/mitchellh/7acae3abd8355c1c00287d67e96c913a>
- Ghostty 实现 PR：<https://github.com/ghostty-org/ghostty/pull/11499>（2026-10-12 合并，进 1.3.0，不在 1.2.3）
- Herdr 跟踪：<https://github.com/herdrdev/herdr/pull/5172>（核对时仍是开放 PR）

序列：

```text
OSC 7501 ; key=value ; key=value … ST
```

`ST` 是 `ESC \`，也可以用 BEL。键值是 token，大小写敏感，顺序无关。

| 键 | 含义 |
| --- | --- |
| `state` | 必填。`idle` / `working` / `blocked` / `done` / `error` |
| `title` | 谁在报告。不发就沿用上一次；空值清除 |
| `status` | 一行原因。省略表示没变；空值清除 |
| `ttl` | 秒。超时后终端把状态当空闲。省略表示由终端自己决定 |
| `sticky` | `1` 或 `0`。完成后留着等用户看见。省略表示没变；空值清除。只有 `done` / `error` 允许为 `1` |
| `clear` | `1` 表示清除。可以单独发。和 `state` 一起出现则整条无效 |
| `id` | 同一条屏幕上多个状态的标识。不透明字符串 |

状态只允许往「更需要人看」的方向盖住别人：`idle` < `working` < `done` = `error` < `blocked`。
`done` 和 `error` 互不覆盖。终端可以响铃、亮角标，但那是终端自己的事，程序不能要求。

失效规则：非法则整条作废，保留上一份状态；`title` 最长 64 字节，`status` 最长 127 字节，
都按 UTF-8 截断。`clear=1` 只清 `title` 和 `status`，不清 `ttl` / `sticky`。
`ttl=0` 表示不过期。

程序不该把 7501 写进日志或管道。规范建议用 `isatty(3)` 判断，**不要**包在
`OSC 133 ; P` / `OSC 133 ; A` 的 prompt 标记对里，否则 tmux 会把它收进 capture。

### tmux 和 Herdr 各自怎么碰到它

- **tmux**：没有 `%program-status`。序列和其他 OSC 一样走 `%output`。muxterm 只要在
  现有 VT 里识别 7501，tmux workspace 就有。tmux 不会替我们解析。
- **Herdr**：本机 `herdr 0.8.0` 二进制里没有 `program_status` / `Program Status` / `OSC7501`。
  上游 PR 5172 还开着，所以 0.8.0 不会把 7501 收成自己的 agent 状态。程序若直接写 7501，
  它会留在 pane 字节里，和 tmux 一样要靠我们的 VT。Herdr 以后若在服务端吃掉 7501、改发
  自己的状态事件，再对着那个事件接，不要两套各算各的。

### 接到 muxterm 的哪

不新造状态机。`working` 对齐现有 activity working，`blocked` 对齐「在等用户」，
`done` / `error` 对齐完成（`error` 必须能和成功分开），`idle` 和 `clear` 对齐回到空闲。
`sticky=1` 才适合进 Attention；普通 `working` 不要亮红点。
`ttl` 到期视同 `idle`，避免 agent 崩溃后红点一直亮。

和屏幕刮取的关系：7501 是程序自己说的，优先于用标题、进程名、画面规则猜。
没有 7501 的程序继续走现在的规则。Herdr 文档里举过反例：只为认出 Claude Code 就维护了
十几条规则；7501 就是用来换掉这种猜测的。
