# Muxterm — Native UI terminal for tmux control mode (Linux)

> 仓库：`~/Project/muxterm/`（bare：`~/Project/muxterm.git`）
> 状态：MVP 开发中
> 创建：2026-07-18
> 原始产品计划：`~/Documents/think/product/tmux-mobile.md`

## 一句话

Linux 桌面终端工具，通过 **tmux 控制模式 (`-CC`)** 与本地/远程 tmux 通信，把 tmux 的 session/window/pane 渲染成自己的原生 **tab pane UI**（类似 iTerm2 的 `-CC` 集成，但搬到 Linux + GTK4），而不是黑框终端 + `Ctrl+B` 操作。

## 核心痛点

- tmux 前缀键 `Ctrl+B` + 方向键反人类
- 现有终端（GNOME Terminal、Alacritty、Kitty）都只是"黑框"，tmux 操作方式没变
- Linux 上没有产品把 tmux pane 渲染成原生 GUI tab

## 技术栈

- **Rust 2021**（核心引擎 + UI）
- **GTK4 + gtk4-rs**（Linux 原生 UI）
- **vte crate**（ANSI 转义序列解析，终端样式渲染）
- **tokio**（异步，处理 tmux 进程 I/O）
- 本地实测版本：rustc 1.97.1 / tmux 3.7b / gtk4 4.22.4

## 架构（MVP）

```
┌──────────────────────────────────────────┐
│  UI 层 (GTK4 + gtk4-rs)                  │
│  ┌────────────────────────────────────┐ │
│  │ Notebook tab 列表 (每个 pane=tab)  │ │
│  │ 输出流渲染 (ANSI 颜色/样式)          │ │
│  │ 输入框 (发送到当前 pane)             │ │
│  └────────────────────────────────────┘ │
├──────────────────────────────────────────┤
│  核心引擎 (Rust)                         │
│  ┌────────────────────────────────────┐ │
│  │ tmux 控制协议客户端                  │ │
│  │  ├ 协议解析器 (parser)              │ │
│  │  ├ 命令发送器 (sender)              │ │
│  │  └ 事件分发器 (dispatcher)          │ │
│  ├────────────────────────────────────┤ │
│  │ 终端模拟 (vte crate)                │ │
│  ├────────────────────────────────────┤ │
│  │ 传输层 (本地 spawn tmux -CC)        │ │
│  └────────────────────────────────────┘ │
└──────────────────────────────────────────┘
```

## tmux 控制协议要点（实现核心）

当 tmux 以 `-CC` 启动（`tmux -CC new-session` 或 `tmux -CC attach`），输出结构化消息（每行 `%` 开头），客户端通过 stdin 发命令：

**通知消息（tmux → 客户端）：**
```
%output <pane_id> <content>          ← pane 新输出（含 ANSI 样式转义）
%layout-change <pane_id> <layout>    ← 布局变化
%window-add <window_id>              ← 新窗口
%window-close <window_id>            ← 窗口关闭
%window-renamed <window_id> <name>   ← 窗口重命名
%session-changed <session_id>        ← session 切换
%session-renamed <session_id> <name> ← session 重命名
%sessions-changed                     ← session 列表变化
%pane-mode-changed <pane_id> <mode>  ← pane 模式变化
%exit                                 ← tmux 退出
%begin <tmux_version> <...>           ← 命令响应开始边界
%end <tmux_version> <...>             ← 命令响应结束边界
%error <tmux_version> <...>           ← 命令出错边界
%extended-output <pane_id> <...>      ← tmux 3.3+ 扩展输出（OSC 8 超链接等）
```

**命令（客户端 → tmux stdin）：**
```
send-keys -t <pane_id> <keys>        ← 发送按键（Enter/C-c/Tab/BSpace/Up/Down 等）
send-keys -t <pane_id> -l "text"     ← 逐字发送（不解释转义，用于粘贴）
resize-pane -t <pane_id> -x <w> -y <h>
list-windows -t <session_id>
list-panes -t <window_id>
display-message -p -t <pane_id> '#{pane_current_command}'
new-window -t <session_id> -n <name>
kill-pane -t <pane_id>
```

**关键实现细节：**
- pane ID 格式：`@1`, `@2`（`@` 前缀 + 数字）
- window ID 格式：`@1`（`@` 前缀，但与 pane 区分靠上下文）—— 注意 session/window/pane 都可能用 `@` 前缀，靠消息类型区分
- session ID 格式：`$1`, `$2`（`$` 前缀 + 数字）
- `%output` 内容用双引号包裹，支持 C 转义（`\n`, `\e`, `\\`, `\"` 等），内含 ANSI 转义序列需原样传给终端模拟器渲染
- tmux 已处理光标移动、滚动区域、备用屏幕缓冲区，我们只需渲染**样式**（颜色/粗体/下划线）+ **文本**（Unicode/emoji）
- 目标 tmux 版本：3.0+（本地实测 3.7b）
- **参考实现**：iTerm2 的 tmux integration 源码（`iTerm2/sources/TmuxController*`）+ tmux 源码 `control.c`

## MVP 功能清单

- [ ] 本地 spawn `tmux -CC new-session` 或 `attach`
- [ ] 协议解析器：解析所有 `%` 消息类型
- [ ] pane 列表 + 每个 pane 独立输出缓冲
- [ ] GTK4 Notebook tab：每个 pane 一个 tab
- [ ] ANSI 颜色/样式渲染（vte crate）
- [ ] 输入框：`send-keys -t <pane_id> <text>` Enter
- [ ] pane 切换、关闭
- [ ] 自动 reattach / 状态同步

**后续 Phase（不在 MVP 范围）：** SSH 传输、Mosh、多端同步、AI agent 检测、通知推送、文件浏览器（见原计划文档）。

## 代码结构（建议，Codex 可调整）

```
src/
├── main.rs              # 入口，GTK app 启动
├── tmux/
│   ├── mod.rs
│   ├── protocol.rs      # % 消息解析器（line-oriented parser）
│   ├── client.rs        # tmux -CC 进程管理 + stdin/stdout 通道
│   └── command.rs       # send-keys 等命令构造器
├── terminal/
│   ├── mod.rs
│   └── screen.rs        # pane 屏幕缓冲（基于 vte）
├── ui/
│   ├── mod.rs
│   ├── app.rs           # GTK Application
│   ├── notebook.rs      # Notebook tab 管理
│   ├── pane_view.rs     # 单个 pane 的渲染视图
│   └── input_bar.rs     # 底部输入框
└── state/
    └── mod.rs           # session/window/pane 状态树
```

## 开发约定

- Rust 2021 edition
- 错误处理：`anyhow` (应用层) + `thiserror` (库层错误类型)
- 日志：`tracing` / `tracing-subscriber`
- 测试：每个协议解析函数都要有单元测试（`#[cfg(test)] mod tests`）
- commit 信息：`feat:` / `fix:` / `docs:` / `test:` / `refactor:`
- **先写协议解析器和单元测试，再接 GTK UI** — 核心逻辑可独立验证
- Codex 在主 worktree `/home/wlz/Project/muxterm/` 工作，逐步提交到 main 分支

## 分支策略（worktree 形式）

- bare 仓库：`/home/wlz/Project/muxterm.git`
- 主 worktree：`/home/wlz/Project/muxterm/`（main 分支，Codex 工作目录）
- 后续功能分支：`git worktree add /home/wlz/Project/muxterm-<feature> -b <feature>`
