# Muxterm ↔ Tmux 层级映射（权威定义）

> **此文档是 muxterm 项目的架构基石，所有代码实现必须严格遵守。**
> **任何 Backend 实现都不能违反这个映射。**

## Muxterm 层级模型（TerminalModel 对外暴露的 4 层）

```
Session → Window → Tab → Pane
```

| 层级 | 说明 | ID 格式 | 数量关系 |
|------|------|---------|----------|
| Session | 一个终端会话（可后台） | `$N` | 1 个 tmux session = 1 个 muxterm Session |
| Window | 窗口（类似浏览器窗口） | `wN` | **固定 1 个**，绑定到 Session |
| Tab | 窗口内的标签页 | `tN` | **多个**，用户可新建/关闭 |
| Pane | Tab 内的分割区域 | `@N` | 多个，一个 Tab 内可分割 |

## Tmux 层级模型（tmux 原生 3 层）

```
session → window → pane
```

| 层级 | 说明 | ID 格式 |
|------|------|---------|
| session | tmux 会话 | `$N` |
| window | tmux 窗口（可多个） | `@N`（-CC 模式） |
| pane | tmux 分割区域 | `%N`（-CC 模式下映射为 `@N`） |

## 映射规则（TmuxBackend 内部必须遵守）

```
tmux session  ──→  muxterm Session     （1:1）
tmux window   ──→  muxterm Tab         （1:1）  ← 关键！tmux 的 window 不是 muxterm 的 Window
tmux pane     ──→  muxterm Pane        （1:1）
（无）         ──→  muxterm Window      （虚拟，固定 1 个，绑定 Session）
```

### 具体映射表

| muxterm 概念 | tmux 对应 | 说明 |
|-------------|----------|------|
| Session `$N` | tmux session `$N` | 直接映射 |
| Window `w1` | （无直接对应） | TmuxBackend 虚拟创建，固定 1 个，绑定 Session |
| Tab `tN` | tmux window `@N` | tmux 的每个 window 映射为 muxterm 的一个 Tab |
| Pane `@N` | tmux pane `%N`→`@N` | tmux -CC 模式下 pane id 映射 |

### 操作映射

| muxterm 操作 | tmux 操作 | 说明 |
|-------------|----------|------|
| new-session | `new-session` | 创建新 session |
| attach-session | `attach-session -t <name>` | 连接已有 session |
| new-window（muxterm）| `new-window`（tmux）| 创建新 tab（不是 window！）|
| switch-window | `select-window -t :N` | 切 tab（不是切 window）|
| split-pane -h | `split-window -h` | 水平分割 pane |
| split-pane -v | `split-window -v` | 垂直分割 pane |
| close-window | `kill-window` | 关闭 tab |

### 举例

一个 tmux session 有 2 个 window（@0 有 3 pane，@1 有 1 pane），attach 后：

```
muxterm 状态：
Session $1 "demo"
└── Window w1            ← 虚拟，固定 1 个
    ├── Tab t0 "window0" ← tmux window @0
    │   ├── Pane @0      ← tmux pane %0（左半）
    │   ├── Pane @1      ← tmux pane %1（右上）
    │   └── Pane @2      ← tmux pane %2（右下）
    └── Tab t1 "window1" ← tmux window @1
        └── Pane @3      ← tmux pane %3
```

**list-windows** 应该返回 **1 个 Window**（w1）
**list-tabs**（如果有）应该返回 **2 个 Tab**（t0, t1）
**list-panes** 应该返回 active tab 的 pane

## 常见错误（必须避免）

❌ 把 tmux 的 window 映射成 muxterm Window
❌ 一个 session 有多个 Window
❌ TmuxBackend 把 tmux 3 层定义泄漏到 TerminalModel
❌ list-windows 返回 tmux 的 window 数量

✅ 一个 session 永远只有 1 个 Window
✅ tmux 的 window 是 muxterm 的 Tab
✅ TmuxBackend 内部做转换，TerminalModel 只看到 4 层