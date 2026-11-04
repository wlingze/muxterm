# Agent Chat 视图：结构化渲染、输入与侧边栏

> 探索记录。核对时间：2026-11-02T15:07–17:12+08:00、2026-11-04T11:33–11:47+08:00（Asia/Shanghai）。
> 本文定义目标与边界，不是完成验收，也不授权本轮实现。
> 相关：[`OUTPUT-READING.md`](OUTPUT-READING.md)（只读阅读的边界）、[`FRONTEND.md`](FRONTEND.md)（Scene / PaneSurface）、
> [`SURFACE.md`](SURFACE.md)（像素定律）、[`PRODUCT-DIRECTION.md`](PRODUCT-DIRECTION.md)。
> 前端框架取舍见 [`FRONTEND-PLATFORM.md`](FRONTEND-PLATFORM.md)。

## 1. 结论

**把 pane 里已经在运行的 agent 渲染成类 Codex 的对话界面，是可行的。条件是 agent 自己提供结构化数据源。Muxterm 只做呈现与投递，不做 transcript 解析。**

四条判断：

1. **不做 Core transcript lane。** 每个 agent 一套文件解析，路径、配置目录和格式版本一变就失效。
   Alma 的 issue 已证明：按默认路径找 `~/.claude/projects` 会因 `CLAUDE_CONFIG_DIR` 失效 [1]。
2. **agent 是宿主，Muxterm 是呈现客户端。** 事件由 agent 进程内的 adapter 产生，不另起 agent，不双写会话。
3. **绑定用进程身份，不用文件名猜测。** `docs/OUTPUT-READING.md` §4 已禁止按 cwd 或最新文件猜会话。
4. **能力显式上报，缺能力就回到 Terminal。** 没有 adapter 的 pane 不显示 Chat 入口，不显示空对话。

本功能同时交付 agent 聚合：侧边栏的 agent 列表用同一套渲染，点击进入该 pane 的 Chat。

## 2. 术语

| 术语 | 含义 |
| --- | --- |
| adapter | agent 进程内的扩展，向本机暴露会话事件与小命令接口 |
| Presentation | 一份可渲染的会话快照，带序号 `seq` |
| 绑定 | pane 与 adapter 之间按进程身份建立的关联 |
| 能力 | adapter 声明的功能位：流式粒度、能否发送、能否回答对话、能否列命令 |
| Terminal 视图 | 已有 `PaneSurface`，VT 渲染，原样转发输入 |
| Chat 视图 | 新增的只读投影加 composer，不占第二个 VT，不改 pty 尺寸 |

## 3. 目标与非目标

**目标**

- 同一 pane 内切换 Terminal 视图与 Chat 视图。
- Chat 视图显示：用户消息、assistant 回复、thinking、工具调用卡片、状态行。
- Chat 视图可以发送消息，可以停止正在运行的轮次。
- 侧边栏按 agent 聚合，使用对话列表样式，点击进入对应 pane 的 Chat 视图。
- 新建 agent 对话时，走 Runtime 能力启动，启动后进入同一套 Chat 视图。

**非目标**

- 不从 ANSI 画面推断对话轮次。见 [`OUTPUT-READING.md`](OUTPUT-READING.md) §3。
- 不新建 agent 进程，不复制会话历史，不改写 agent 的会话文件。
- 不承诺所有 agent 都支持。支持范围由 adapter 决定，逐 agent 增加。
- 不做内嵌浏览器侧边栏。这是独立的产品决策，见 [`PRODUCT-DIRECTION.md`](PRODUCT-DIRECTION.md) §6。
- 不做跨机器会话同步、账号体系、多 agent 群发。

## 4. 两条路线

| 路线 | 做法 | 对“已在跑的 TUI”有效 | 判定 |
| --- | --- | --- | --- |
| A. Core 解析会话文件 | Core 逐 agent 读 JSONL / SQLite | 有效 | **不采用**。维护成本随 agent 数增长，路径与版本漂移即失效 |
| B. agent 侧 adapter | agent 进程内的扩展暴露事件与命令 | 有效 | **采用**。协议由 Muxterm 定义，agent 侧各写一份 |

路线 A 的现实成本有直接证据。Ovis 为每个 agent 维护一套 reader：Claude Code、Codex、omp、omo、gjc、pi、
Command Code、OpenCode、Devin、Grok、Hermes、Muse、agy、Letta、Copilot 等，
并且对每个 agent 注明“哪些菜单只能用 Terminal” [2]。这是可持续的工程量，不是一次投入。

路线 B 的协议可以照搬成熟实现。`pi-idle-supervisor` 0.1.10 是 pi 的扩展，跑在活 TUI 内，
提供 `GET /v1/snapshot`、`GET /v1/events`（SSE，带 `seq` 与 `Last-Event-ID`）、
`GET /v1/commands`、`POST /v1/prompt`。它在 `/new`、`/resume`、`/fork` 和 reload 后仍然存活 [3]。
Muxterm 的协议按这份设计收敛，不另造一套。

**否掉的两条路线**

- **ACP**：协议要求客户端启动 agent 子进程。`session/load` 是重放历史，不能附着已在运行的 TUI [4]。
- **Pi Web 类宿主**：它在服务端建 `AgentSession` 再 `prompt`，是 agent 的宿主，不是观察者 [5]。

## 5. 绑定：进程身份

adapter 启动时把登记信息写到本机固定目录，内容包括进程 id、端点、会话 id 和能力。
Core 用 pane 的前台进程身份匹配登记信息。

| Runtime | 进程身份来源 | 现有代码 |
| --- | --- | --- |
| tmux | pane pid 与前台进程组 | `src/core/runtime/tmux/pane_process.rs` |
| Herdr | `pane process-info` 的 `shell_pid` 与 `foreground_processes[]` | `src/core/runtime/herdr/` |
| 本地 shell | 子进程树 | `src/core/protocol/terminal/process.rs` |
| SSH | 远端执行同一条查询 | 同上，经 Transport |

`PaneAgentInfo.session`（`source` / `agent` / `kind` / `value`）已经存在，并已通过
`StateChange::PaneAgentChanged` 送到前端。它只作加速与展示，不作唯一真相。

必须处理的绑定失效：

- agent 退出，登记信息过期：按进程身份校验，校验失败即丢弃。
- 同一 pane 更换会话（`/new`、`/resume`）：adapter 上报 `session_replaced`，Chat 视图清空并重新取快照。
- 同一台机器多个同类 agent：按进程 id 区分，不按目录。
- SSH 远端：只读远端的登记信息，不读本机同路径的文件。

## 6. 数据契约

第一版只要四组接口。

| 接口 | 作用 | 要点 |
| --- | --- | --- |
| `snapshot` | 取当前会话全量 | 返回 `{seq, items[], status, model, queue}` |
| `events` | 增量事件流 | SSE 或等价的本地流；断开后用 `Last-Event-ID` 续传；无法续传时发 `resync` |
| `commands` | 列出可发送的命令 | 只列真正可执行的项；不含 agent 的内建 TUI 菜单命令 |
| `prompt` | 发送一条消息 | `{text, images, deliverAs}`，`deliverAs` 取 `now` / `steer` / `followUp`；返回是否被接受 |

事件类型沿用已验证的集合：`message_start` / `message_delta` / `message_end`、
`tool_start` / `tool_update` / `tool_end`、`status`、`settled`、`queue`、`model`、
`session_replaced`、`dialog_open` / `dialog_closed`、`notice`。

渲染单元的 `kind` 固定为一小组：`user`、`assistant`、`thinking`、`tool`、`notice`、`status`。
工具单元带 `{id, name, summary, input, output, state}`。未知事件类型必须忽略，不得报错。

`snapshot` 与 `events` 走同一条 FFI 边界。Core 只做转发与序号校验，不解释业务字段。

## 7. 视图与交互

**7.1 切换**

Chat 与 Terminal 是同一 pane 的两种投影。切换是前端状态，不经过 Core，不重建 `PaneSurface`，
不改 pty 尺寸，不改布局。这保持 `FRONTEND.md` 的“每 pane 恰好一个常驻 `PaneSurface`”定律。

切换失败的处理：Chat 取不到数据时保留草稿，显示原因，并允许一键回到 Terminal。

**7.2 composer**

- 多行、中文输入、粘贴、按目标保存草稿。复用 `PRODUCT-DIRECTION.md` §5.1 的编辑框，不新建第二套输入组件。
- 发送走 adapter 的 `prompt`。这条路径不会把文字误投到菜单或 shell 提示符。
- adapter 没有 `prompt` 能力时，退回 `WriteRaw` + `SendKeys`，并且界面必须标注“已写入终端”。
  终端写入不等于 agent 接受，这是 §5.1 的硬要求。
- 图片：复用现有图片投递通路，adapter 支持时随 `prompt` 一起发送。

**7.3 `/` 命令补全**

命令来源只有一个：adapter 的 `commands`。

- 不抄竞品的硬编码命令表。Alma 的 `builtinSlashCommands` 为 claude / codex / pi 各写一张表 [6]；
  这类表随 agent 版本变化，会给出错误建议。
- pi 的 `get_commands` 明确只返回扩展命令、prompt 模板和 skill，**不含**内建 TUI 命令 [7]。
  因此 Chat 里输入 `/model` 可能失败。界面必须区分“可发送”和“只在终端可用”，并在失败时给出提示。
- Muxterm 自己的动作（例如“在 Chat 里截取选区引用”）单独列出，来源标记为 `muxterm`。

**7.4 `@` 文件引用**

`@` 是 Muxterm 自己的文件选择器，不是 agent 协议的转发。理由：Alma 的 `@` 选择器选的是它自己的
reference 对象（thread / snippet / scratch / goal 等），不是文件系统 [6]。

实现用现有能力：`muxterm_list_dir_json` 与 `DirectoryPathModel`（`src/frontend/macos/Chrome/DirectorySuggestion.swift`）。
选择后插入纯路径文本，不加引号，不加 Markdown 语法，与现有编辑框的粘贴语义一致。

**7.5 停止与队列**

- `cancel` 停止当前轮次。界面在运行期间显示停止按钮。
- 运行中发送的消息按 `deliverAs` 处理：`steer` 进入当前轮次，`followUp` 排在轮次之后。
- 队列项在 UI 可见，可以撤销。这条能力依赖 agent 侧支持，能力位为 false 时不显示。

## 8. 侧边栏与聚合

侧边栏的 Agents 槽改成对话列表样式：每行一个会话，显示 agent 名、会话标题、状态、最近活动时间。
点击一行打开该 pane 的 Chat 视图。已有实现是 `AgentSidebarItem` 与 `WorkspaceSidebarProjection.agents`，
本轮只改渲染与数据字段，不新建第二套侧边栏。

硬约束：

- 不为 Herdr 单独做侧边栏或 agent 列表（`FRONTEND.md` §8）。
- 侧边栏数据来自 `ViewStore` 与 `Attention`，不新增事件泵。
- 会话标题来自 agent 上报。没有上报时显示进程名与路径，不猜测标题。
- 状态沿用现有五态：`working` / `blocked` / `done` / `idle` / `unknown`。

## 9. 新建 agent 对话

入口在侧边栏与命令面板。流程是“新建 pane 或 tab → 在 pane 内启动 agent → 进入 Chat 视图”。

现状缺口：

- `Task::NewTab` 与 `Task::SplitPane` 有 `command` 字段。tmux 与本地 shell 后端会使用它，
  **Herdr 后端两处写作 `command: _`，丢弃该字段**（`src/core/runtime/herdr/runtime.rs`）。
- Herdr 提供 `agent.start`，要求 pane 停在交互式 shell 提示符，失败要重试，再降级为发送命令文本。
- pi 的 RPC 与 SDK 入口都是另起进程，不能附着已有实例，因此不作为新建路径的主选。

要做的是补齐 `command` 透传，或在 herdr 走 `agent.start` 并在失败时降级。两者都需要能力位区分。

## 10. 能力矩阵与降级

| agent | 事件流 | 发送 | 列命令 | 回答对话 | 第一版 |
| --- | --- | --- | --- | --- | --- |
| pi（扩展） | token 级 | 是 | 是 | 部分 | 做 |
| Claude Code（hooks） | 轮次级 | 部分 | 未确认 | 否 | 第二个 |
| Codex（app-server） | token 级 | 是 | 是 | 是 | 未确认能否附着已运行的 TUI |
| 其他 / 无 adapter | 无 | 否 | 否 | 否 | 只有 Terminal |

能力位：`streaming: token\|turn\|none`、`prompt: yes\|no`、`commands: yes\|no`、
`dialogs: yes\|no`、`cancel: yes\|no`。界面按能力位降级，不做运行时名字判断。

Claude Code 的现状：hooks 提供会话 id 与 transcript 路径，`Stop` 事件带 `last_assistant_message`，
但 transcript 是异步写入，会滞后 [8]。因此它有轮次级能力，没有 token 级流式。

## 11. 交付顺序与验收

| 阶段 | 交付物 | 退出条件 |
| --- | --- | --- |
| S0 验证 | 最小 pi 扩展 + 命令行打印快照与事件 | 在隔离 socket 的 tmux 与 herdr 各一个 pi pane 上，按进程身份绑定成功；`/new` 与 `/resume` 后仍绑定正确 |
| S1 Core | `Runtime::pane_process()` + Presentation 客户端 + FFI | 单测覆盖序号校验、续传、`session_replaced`、绑定失效 |
| S2 macOS | Chat / Terminal 切换 + composer + 侧边栏列表 | 切换不改 Surface 尺寸；取数失败保留草稿；无 adapter 的 pane 不显示入口 |
| S3 兼容 | Claude Code adapter | 轮次级渲染正确；不支持的能力位隐藏对应控件 |
| S4 其他前端 | Linux、TUI | 同一套词汇表，同一套能力位 |

验收必须在真实环境执行，并记录方法与范围。只用单元测试不能宣称“绑定正确”。

## 12. 未确认

- Codex 已运行的 TUI 是否可被 app-server 观察。官方文档说明 `codex --remote` 让**新的** TUI 连接服务端，
  没有说明能观察已在运行的实例 [9]。
- Herdr 是否上报可用的会话标题与命令列表。当前 schema 只有 `agent_session` 与环境信息。
- Claude Code hooks 的轮次级数据能否稳定覆盖工具输出。
- `tuux` 项目身份。2026-11-04 检索未找到同名终端项目。

## 13. 证据

1. yetone/alma-releases#121：按默认路径找 `~/.claude/projects` 时，`CLAUDE_CONFIG_DIR` 导致失败。
2. Ovis `docs/guide.md` “Supported agents” 表：逐 agent 的原生 transcript 与可用操作，其余退回 Terminal。
3. `pi-idle-supervisor` 0.1.10 `docs/protocol.md`：端点、SSE、`seq`、错误码、`/new` `/resume` 后存活。
4. ACP 规范 `transports` 与 `session-setup`：客户端启动子进程；`load` / `resume` 不接管任意终端进程。
5. agegr/pi-web `AGENTS.md`：服务端 `createAgentSessionFromServices()` 后 `session.prompt()`。
6. Alma Android 打包产物 `/tmp/muxterm-gui-investigation-20261102/alma-App.js`：
   `AgentsStore.prompt`（`agent.prompt` 失败退回 `pane.send_text` + `enter`）、
   `builtinSlashCommands`、`useSlashCommands`、`referencesApiClient`、`KINDS`、`parseChatItem`。
7. pi 1.4.0 `docs/rpc-commands.md` “Discoverable commands”：`get_commands` 不含内建 TUI 命令。
8. Claude Code hooks 文档：公共字段 `session_id`、`transcript_path`；`Stop` 带 `last_assistant_message`。
9. Codex app-server 文档：`thread/start|resume|read|unsubscribe`、`fuzzyFileSearch`、`codex --remote` 的用法。
