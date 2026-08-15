# Muxterm 统一 ID 体系

> **2026-08-15 23:41：产品路径是 Workspace / Tab / Pane。**
> 权威：[`WORKSPACE.md`](WORKSPACE.md) §6。废弃 `s{name}/w1/t2`。
> tmux `$N` `@N` `%N` 只在 `runtime/tmux`。
>
> 新路径（W7 落地）：
> `workspace/{name}` · `workspace/{name}/tab/{id}` · `workspace/{name}/tab/{id}/pane/{id}`
> CLI：`-s <工作区名>` / `-t <tab>` / `-p <pane>`。没有 `-w`。

## 设计目标（历史）

下文 `s{name}/wN/tN/pN` 是旧方案，**不要按它实现**。保留以免考古断档。

## ID 格式

```
s{name}     → Session（按名字）
w{n}        → Window（数字编号，从 1 开始）
t{n}        → Tab（数字编号，从 1 开始）
p{n}        → Pane（数字编号，从 1 开始）
```

### 组合格式

```
s{name}                 → 指定 session
s{name}/w1              → 指定 session 内的 window 1
s{name}/w1/t2           → 指定 session 内 window 1 的 tab 2
s{name}/w1/t2/p3        → 指定 session 内 window 1 tab 2 的 pane 3
s{name}/t2/p1           → 省略 window（默认 w1）
s{name}/p2              → 省略 window 和 tab（默认 w1 + active tab）
```

### 简写

- `-s test` → session 名为 test
- `-t @1` → 当前用 tmux 格式，要改成 `-t 1` 或 `s{test}/w1/t1/p1`
- `list-panes -s test -t 1` → 列出 session test 的 tab 1 的 panes
- `split-pane -s test -t 1 -p 2 -v` → 对 session test tab 1 的 pane 2 垂直分割
- `send-keys -s test -p 2 "echo hello"` → 向 session test 的 pane 2 发送命令

## CLI 命令参数规范

所有命令支持：
- `-s <name>` → session 名
- `-L <socket>` → tmux socket（有 -L 就是 tmux 模式）
- `-w <n>` → window 编号（默认 1）
- `-t <n>` → tab 编号（默认 active tab）
- `-p <n>` → pane 编号（默认 active pane）

### 示例

```bash
# 创建 session
muxterm new-session -s test [-L socket]

# 列出 session
muxterm list-sessions [-L socket]

# 列出 window（永远只有 1 个）
muxterm list-windows -s test [-L socket]

# 列出 tab
muxterm list-tabs -s test [-L socket]

# 列出 pane（需要指定 tab）
muxterm list-panes -s test -t 1 [-L socket]
muxterm list-panes -s test -t 2 [-L socket]

# 分割 pane
muxterm split-pane -h -s test -t 1 [-L socket]
muxterm split-pane -v -s test -t 1 -p 2 [-L socket]

# 切 tab
muxterm select-tab -s test -t 2 [-L socket]

# 发送命令
muxterm send-keys -s test -t 1 -p 1 "echo hello" [-L socket]

# 抓取 pane 输出
muxterm capture-pane -s test -t 1 -p 2 [-L socket]

# detach / attach
muxterm detach -s test [-L socket]
muxterm attach-session -t test [-L socket]

# 关闭
muxterm kill-session -s test [-L socket]
```

## 内部映射

TmuxBackend 内部维护映射表：
- muxterm session name → tmux session name
- muxterm tab number (1-based) → tmux window index (0-based)
- muxterm pane number (1-based) → tmux pane id (%N)

LocalBackend 内部：
- session name → daemon socket path
- tab/pane number → 内部 Vec 索引

## 输出格式

JSON 输出用 muxterm 自己的编号，不用 tmux 的 @N/$N：
```json
{"tab": 1, "pane": 2, "size": {"w": 40, "h": 12}, "active": true}
```

text 输出：
```
Session: test
  Window: w1
    Tab 1: "zsh" (3 panes)
      Pane 1: 40x24
      Pane 2: 39x12 [active]
      Pane 3: 39x11
    Tab 2: "zsh" (1 pane)
      Pane 1: 80x24 [active]
```