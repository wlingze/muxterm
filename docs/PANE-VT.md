# PANE-VT.md — 工作区池（讨论稿，已收口）

> **2026-08-15 22:50：本文降为讨论记录。** 权威命名与结构：[`WORKSPACE.md`](WORKSPACE.md)。
> 施工：[`WORKSPACE-PLAN.md`](WORKSPACE-PLAN.md)。映射：[`LAYER-MAPPING.md`](LAYER-MAPPING.md)。
> 像素：[`SURFACE.md`](SURFACE.md)（F 已冻结）。
>
> 讨论修订：2026-08-15 22:35 CST（`2026-08-15T22:35:07+08:00`）
> **产品层没有 Session**；没有虚拟 Window `w1`。Window 只是 Workspace 的体现。池在 core。

**一句话：** 池里一格 = **一个 Runtime 封装成的工作区**（一条 tmux / 一个 shell / 以后一个 Herdr）。里面是 tab → pane。**pane 是最小渲染单位**，带有界 buffer、上翻、滚动位置。中间层像 tmux 的客户端模型，**自己不养 session**，只吃 `-CC` / Herdr socket。

「Virtual terminal」容易和 GTK 的 VTE、以及「一个 pane 一台仿真器」搅在一起。池里那格的产品名已经有了：**Workspace**。

---

## 1. 粒度（这次对齐）

```
Runtime（输入：tmux | shell | herdr）
    一条连接、一种后端协议
    不进用户切换器
         │  封装
         ▼
Workspace（池里的一格 ← 你刚才说的 VT）
    = 一个 Runtime 的客户端侧模型
    有多个 Tab（tmux window → muxterm Tab）
    每个 Tab 有多个 Pane
         │
         ▼
Pane（最小 output / 最小渲染）
    网格 + 有界 buffer
    上翻历史、记住滚动位置
    搜索的最小坐标
```

和 [`WORKSPACE.md`](WORKSPACE.md) 一致（旧四层已删）：

| 池 / 结构 | muxterm | tmux | 渲染 |
|---|---|---|---|
| 池里一格 | Workspace（一个 Runtime） | 一个 `tmux session` | 不整棵树一起画 |
| GUI | Window（平台窗口，很少） | （无） | 用户开的那扇窗 |
| 组合 | Tab | tmux window `@N` | 当前只画 **一个** tab |
| **叶子** | **Pane** | tmux pane `%N` | **最小块**：buffer、上翻、位置、搜索命中 |

Tab / 整窗都是 **pane 拼起来的**。没有「tab 自己的终端」；终端状态只在 pane 里。

多个 tmux = 多个 Workspace 进池。前端仍是少量窗口：当前只绑定 **一个** Workspace 的 **一个** Tab 里那些 pane 去画。

---

## 2. 中间层不是第二个 tmux

它 **不** `new-session`、不替代远端布局。它：

- 接 Runtime 的字节和拓扑（tmux `-CC` / Herdr observe）
- 在每个 pane 上维持「最近」：当前屏 + 有界历史
- 给前端：快切（换绑 workspace/tab）、提醒、peek、搜索
- 按键仍回对应 Runtime

远端 tmux 继续是事实源。这边是 **一直在更新、但大部分不画** 的镜子。

---

## 3. Pane buffer、上翻、位置

没有 pane buffer，快切就要么重放全部 output，要么向 tmux 再要整段历史。有 buffer 就可以：**只画最近一屏（加一点余量）**。Codex 刷一万行也不在切过去时重放一万行。

同一套状态还要：

- **上翻 / 下翻**：看比当前屏更早的行（有界，超出丢最旧）
- **滚动位置**：停在某处，切走再回来还在那儿（每个 pane 自己记；属于 Workspace 客户端状态，不是 tmux 的 copy-mode）
- 第一次填满一屏：内部还空时可以向 tmux 要一帧可见区；之后靠持续 feed。再切回来用 **本 pane 的 buffer + 记下的位置**，不要全历史

Linux 现在 dump `visible_ansi` + 切 tab 扔掉 VTE，位置保不住。F 路径要把「当前 tab 的 pane 常驻」做对，位置才能留。

---

## 4. 搜索三层

| 范围 | 扫什么 |
|---|---|
| pane | 这一个 pane 的 buffer / scrollback |
| workspace（一个 VT） | 这个 Runtime 下所有 pane |
| 全局 | 池里所有 Workspace |

命中坐标 = `(workspace, tab, pane, 行)`。跳转 = 切到该 workspace + tab，把该 pane 滚到记下的位置。愿景面板 Search 就是全局这一层。

---

## 5. 别人怎么做（2026-08-15 核查）

来源：iTerm2 官方 tmux 文档、WezTerm Workspaces 文档、克隆树 `wezterm/mux`、ivyTerm、Muxterm macOS `WarmConnectionSlot`。

| | 一格是什么 | 很多 tmux 时 GUI | pane 状态在哪 | 没画的时候 |
|---|---|---|---|---|
| **iTerm2** | 一次 `-CC` attach = 一个 `TmuxController` | tmux window → **原生窗口或 tab**，铺开画。≥N 个 window 出 Dashboard；hidden/buried **不建** VT100 | 每个打开的 pane 一个 `PTYSession` | hidden 无仿真；打开的一直吃 `%output`。2.9 起可多 session |
| **ivyTerm** | 一个 GTK 窗 = **一条** tmux | 第二条 session = 再开窗 | 每个 pane 一个 VTE widget | 该 session 里 widget 都在，只是非当前 tab 不画 |
| **WezTerm mux** | `Domain`（local/ssh/tmux/mux-server） | GUI 只盯 **active workspace 标签**；切 workspace 时把 GUI 窗口内容和 mux 里的 Window **对换**。tmux 是一种 Domain，pane 走假 PTY | `Mux` 里全局 `panes`/`tabs`/`windows`，每 pane 一份 `wezterm_term::Terminal` | mux 对象常驻；像素只画当前 workspace |
| **Muxterm macOS（已落地）** | `WarmConnectionSlot` = `CoreBridge` + **自己的** `TerminalManager`（SwiftTerm 视图） | 一扇窗；Cmd-P 切 slot；旧 slot 后台 poll，视图留在 slot 里 | 每个 workspace 一套 TerminalManager | 后台仍 `handleOutput`，不做 `displayIfNeeded` |
| **Muxterm Linux（HEAD）** | `ConnectionPool` 里只有 `CoreBridge`；**一个** `ReplicaStore` + **一个** `LayoutHost` 在窗口上 | 同样一扇窗切连接 | 无头 `TerminalState` 在 ReplicaStore；VTE 只在当前 layout | 后台只 feed replica；`apply_layout` 的 `panes.retain` 会扔掉其它 tab 的 VTE |

WezTerm 的 workspace 是 **mux 窗口上的标签**（一组 GUI 窗），不是「一条 tmux」。Muxterm 的 workspace = **一条 tmux session**（愿景已写死）。更像：WezTerm 的 Domain +「只画 active」+ macOS 已经在做的「每 slot 一套终端视图」。

iTerm2 明确反例：连上就把 tmux window 铺成很多 OS 窗口。你们要少入口、多工作区，不要抄这个。

---

## 6. Muxterm 已经落地的（HEAD `975a94d`，2026-08-15 22:38）

Codex 刚把 Linux F2–F6 提交了（`vte.feed`、capture 门、`send-keys -H`、VTE scrollback、`%` pane id）。**不要改那条像素路径。** 下面是和「工作区池」相关的存量：

**Core（跨平台）**

- `Backend` trait：一个实例 = 一个 session 来源。注释写了可以多个，但 `TerminalModel` / `MuxtermHandle` **各持一个** `Box<dyn Backend>`。
- `RuntimeMode`：shell/tmux × local/ssh。Herdr 还没有类型。
- `LAYER-MAPPING`：tmux session → 一个 Workspace；tmux window → Tab；tmux pane → Pane。无产品 Session、无虚拟 `w1`。
- `ReplicaStore`：`(workspace_id, pane_id) → TerminalState`。后台 Linux 已往里 feed。`search_all` 已有（跨 workspace+pane），命中 **没有 tab_id**。`raw_bytes` 目前是上一截，不是有界 ring。
- `buffer_cap::MAX_PANE_OUTPUT_BYTES` = 2MiB，用在 backend 累计输出，还没接到 ReplicaStore ring。
- 注意力状态机已有；面板 Search/Attention 在 Linux 接过一层。

**macOS（更接近目标）**

- 池的一格 = bridge **加** TerminalManager。切走：换绑视图，旧 SwiftTerm **还在 slot 里**继续吃 output。滚动位置跟着视图走。
- 这已经是「Runtime 封装成 workspace、前端只换绑」。

**Linux（池和渲染还分家）**

- 池的一格 = 只有 `CoreBridge`。ReplicaStore 是窗口上的全局表。LayoutHost 只有一份，切 tab 会 `retain` 掉不在当前布局里的 VTE → 位置和「最近一屏」的像素端丢掉，只剩无头 replica。
- 后台 poll 已把 `%output` 喂进 replica（提醒/搜索有数据），但 **没有** 每 workspace 一套常驻 VTE。

缺口对照你的设计：缺的不是「要不要内部终端」，而是 **Workspace 类型还没在 core 里立住**；Linux 的像素还没像 macOS 那样挂在 slot 上；pane 视口没有一等字段；搜索缺 tab。

---

## 7. 怎么落地（F 路径之后，不插队）

目标类型（名字可再磨，形状如下）：

```text
WorkspacePool
  Workspace            // 池里一格 = 一 Runtime
    runtime: Box<dyn Backend>   // tmux/shell/herdr，只负责连和字节
    tabs / layout               // 已有 State
    panes: HashMap<PaneId, PaneVt>
      PaneVt
        state: TerminalState    // 有界网格 + scrollback
        byte_ring               // 有界，替代 last_raw_bytes
        viewport                // 滚动位置，切走再回来
```

顺序已收进 [`WORKSPACE-PLAN.md`](WORKSPACE-PLAN.md) W1–W8，**不要按本节另开施工**。形状备忘：

1. **Core `Workspace` 包一层**  
   现有 `TerminalModel` 放进 Workspace。`ReplicaStore` 的那一层 HashMap 收进 Workspace，不要按 window 全局再扫一遍。单测：两个 Workspace 同时 feed，互不污染。

2. **`WorkspacePool` 替代 GUI 侧「只有 bridge 的池」的语义**  
   逻辑从 `quickconnect/pool.rs` 升到 `src/core/`。Linux/macOS 的 ConnectionPool 变成对它的适配，或 macOS 继续持视图、core 只持 Runtime+PaneVt。LRU/TTL 已有，可原样搬。

3. **Pane 视口**  
   `PaneVt.viewport`：行偏移或 VTE 自己的 adj。切 tab/切 workspace 禁止 reset 网格。macOS 已因视图留在 slot 而近似成立；Linux 要 LayoutHost **按 workspace 存 pane 视图**，或切 tab 不 `retain` 掉其它 tab（F 计划里写过，若 Codex 没做就这一步补）。

4. **Buffer = 最近**  
   把 `last_raw_bytes` 改成 `append_capped` ring。搜索/上翻走 `TerminalState.scrollback`。不要把 ring 重放进 VTE 当 TUI 快切。

5. **搜索三层**  
   `search_pane` / `search_workspace` / `search_all`。`SearchHit` 补 `tab_id`。跳转：pool.activate(ws) → switch tab → 恢复 viewport。Linux Search tab 已能打 replica，接跳转即可。

6. **Herdr**  
   新 `Backend` 实现，产出同一套 Pane output 事件。Workspace 不改。现在不要做。

前端接口保持三个：快切（activate workspace + 当前 tab 的 pane 列表）、渲染（订阅那些 pane 的 live 字节或画网格）、搜索/提醒（读 PaneVt，不画全树）。

验证：两个隔离 tmux session 进池，切过去 VTE 非空且 reset 不涨；后台 session 的 token 能搜到；滚到非底部、切走切回位置还在。
