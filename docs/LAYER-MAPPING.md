# Muxterm ↔ Tmux 适配表

> **只给 `src/core/runtime/tmux/` 看。** 产品结构见 [`WORKSPACE.md`](WORKSPACE.md)。
> Core Protocol / FFI / CLI / GUI **禁止**出现 tmux 的 session/window/pane 类型。
> Herdr 对照在 [`RUNTIME.md`](RUNTIME.md) §6.2 与 [`HERDR-PLAN.md`](HERDR-PLAN.md)，不要写进本表。
> 修订：2026-08-15 23:41 CST（`2026-08-15T23:41:41+08:00`）；Herdr 指针 2026-08-17。

TmuxRuntime 是一个 **适配器**：把 tmux 控制模式填进 Muxterm 已经定好的 **Workspace → Tab → Pane**。不是反过来让产品层去迁就 tmux。

## 产品结构（Core 拥有，前端也画这个）

```
WorkspacePool
  └── Workspace
        └── Tab*
              └── Pane*
```

GUI Window **不是**这棵树上的节点，只是某个 Workspace 的体现。

## tmux 原生（出不了 runtime/tmux）

```
session `$N` → window `@N` → pane `%N`
```

## 适配（仅 TmuxRuntime 内部）

| tmux | 填进 Muxterm |
|------|----------------|
| 一条 session（按**名字** attach/create） | **一个** Workspace + 一个 TmuxRuntime 实例 |
| window `@N` | Tab |
| pane `%N` | Pane（字节进 `PaneOutput` / PaneBuf） |
| `$N` / `%session-changed` | 内部 `TmuxSessionId` + 显示名；**不**出现在 FFI |

ShellRuntime **不走这张表**，自己维护同一套 Tab/Pane。

### 操作（内部命令，产品 API 见 WORKSPACE.md §6）

| 产品动作 | TmuxRuntime 内部可以发 |
|---------|-------------------------|
| Pool.open / create | `new-session` / `attach-session -t <name>` |
| `NewTab` | `new-window` |
| `SwitchTab` | `select-window` |
| `CloseTab` | `kill-window` |
| `SplitPane` | `split-window` |
| `ClosePane` | `kill-pane`（仅隔离测试 socket） |
| Pool.close | `detach`（不杀用户默认 server） |

### 举例

tmux session 名 `demo`，两个 window：

```
Workspace "demo"          ← 产品；GUI Window 体现它
  ├── Tab  (← tmux @0)
  │     ├── Pane (← %0)
  │     └── Pane (← %1)
  └── Tab  (← tmux @1)
        └── Pane (← %3)
```

外面的 list-workspaces / `muxterm_workspace_list` 看到的是 Workspace，不是 `$N`。

## 旧模型（作废）

`Session → 虚拟 w1 → Tab → Pane`。不要实现。
