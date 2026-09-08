# Muxterm 统一 ID 体系

> 契约冻结：2026-09-08。权威：[`WORKSPACE.md`](WORKSPACE.md) §6。
> tmux `$N` `@N` `%N` 只在 `runtime/tmux`。Herdr `w2:p1` 只在 `runtime/herdr`。
> 全部产品 ID 用 newtype，不要裸 `u64` / `String` 互换。

## 现行产品路径

```text
workspace/{id}
workspace/{id}/tab/{tab}
workspace/{id}/tab/{tab}/pane/{pane}
activity/{id}
project/{id}
project/{id}/worktree/{id}
```

CLI：`-s <工作区名>` / `-t <tab>` / `-p <pane>`。没有 `-w`。没有产品 Session。

### WorkspaceId

稳定字符串五段：`transport / alias / session / runtime / identity`。

- tmux/shell 没有独立 target-side workspace id 时，第五段使用 path
- Herdr 第五段优先独立 `workspace_id`，项目 `path` 只是元数据
- 第五段即使暂名 `path`，也不能反向当成 Project 目录

详见 [`HERDR-RUNTIME-STABILITY.md`](HERDR-RUNTIME-STABILITY.md) §7。

frontend / Recent / 重连禁止从五段字符串反向猜 socket / workspace id。只读 Core 给出的
Candidate / WorkspaceInfo。

### 其它 newtype

| 类型 | 用途 |
|------|------|
| `TabId` / `PaneId` | 产品拓扑。Runtime 内部 id → 产品 id 的映射停在 adapter |
| `ActivityId` | Core 分配，稳定。frontend 只维护 `ActivityId → ActivityRecord` |
| `RuntimeId` / `TransportId` | provider 注册表键 |
| `TargetId` | `local` 或 SSH alias（connect name） |
| `CandidateRef` | Core 能重新找回的 key：project / (project, worktree) / existing / recent |
| `TemplateName` | `[[templates]]` 引用 |

### FFI / JSON

对外只出现产品 ID。不要 `{ "sessions": [ { "id": "$4" } ] }`。
`CStateChange.window_id` 保留为 0 直到旧 ABI 清掉。

---

## 附录：旧 `s{name}/wN/tN/pN`（不要实现）

2026-08-15 已废弃。下文只为考古。

```text
s{name}     → 当时的 Session
w{n}        → 当时的 Window（产品层已删除）
t{n}        → Tab
p{n}        → Pane
```

当时 CLI 还有 `-w`、`list-windows`、`new-session`。现行命令见 [`WORKSPACE.md`](WORKSPACE.md) §6.3。
