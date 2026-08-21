# 产品愿景 vs 实现 vs 测试（2026-08-17）

> 对照：`PRODUCT.md`、`docs/PRODUCT-VISION-STRATEGIC-REVIEW.md` §2.14 / §2.15 / §6 / §9。
> 修订：`2026-08-17T15:11:19+08:00`。W17 / W18 已绿。Linux 本轮关账；后续 macOS 开发。

愿景 1.0 = **A（日用）+ B（可见性）+ C（搜索）**，外加负责人这轮点名的：真 SSH attach、上次看到这里、命令刻度、pane/工作区/全局搜索、回底 +N。D/E、Herdr、ET、手机、账号不是本轮。

---

## 一句话

Linux tmux 本地路径 W17 门禁已锁。W18 把 SSH 当成和本地同一套断言（ssh 到本机隔离 sshd），并把 scrollback 地标补齐；门禁已独立复跑全绿。

---

## 阶段 A — 日常终端

| 愿景项 | 代码 | 测试 | 判据 |
|---|---|---|---|
| 本地 attach / 切 tab / Surface | 有 | attach/live/render e2e | 够用 |
| 客户端 scrollback | `capture-pane -S -N` + VTE | `linux_attach_history_e2e` | W16a 绿 |
| 回底按钮 | `muxterm-jump-latest` | 同上 | W16a 绿 |
| 回底 +N | 离开底部累计新行 | `linux_jump_count_e2e` | W18e 绿 |
| Scroll lock | feed 不拽回 | `linux_scroll_lock_e2e` | W17b 绿 |
| 断线水印 | overlay | `linux_disconnect_e2e` | W16b 绿 |
| 自动重连（本地） | swap_runtime | `linux_reconnect_e2e` | W17a 绿 |
| 真 SSH attach | LoopbackSshd + `new_ssh_attach` | `tmux_ssh_feature_contract` / `linux_ssh_*` | W18a–d 绿 |
| 粘贴剥控制字符 | `sanitize_paste` | 单测 | 够用 |
| ET / 托盘 / 多窗口 | 无 | — | 不是本轮 |

---

## 阶段 B — 可见性

| 愿景项 | 代码 | 测试 | 判据 |
|---|---|---|---|
| BEL / OSC 133 | 引擎 + live | feature / 1.0 e2e | W17d 绿 |
| blocked 看见不熄 | 转移表 | `linux_attention_semantics_e2e` | W16c 绿 |
| peek + 一行答复 | 小 VTE | W15e | 够用 |
| 命令刻度 | OSC 133 B–C 文本 + `D;<n>` | emulate 单测 + `linux_command_marks_e2e` | W18h 绿 |

---

## 阶段 C — 搜索与地标

| 愿景项 | 代码 | 测试 | 判据 |
|---|---|---|---|
| `search_all` 内存 | PaneBuf | feature e2e | 够用 |
| 跳到命中 + 高亮 | seq + overlay | `linux_search_highlight_e2e` | W17c 绿 |
| pane / workspace / all 范围 | 面板开关 | `linux_search_scope_e2e` | W18f 绿 |
| pane 内查找条 | `muxterm-pane-find` | 同上 | W18f 绿 |
| 上次看到这里 | `muxterm-last-seen` | `linux_last_seen_e2e` | W18g 绿 |

---

## SSH / 以后

W18 用 **测试自启的 loopback sshd**（随机端口），不是用户 22。合盖一小时仍是人手狗食。ET 仍是可选 Transport，本轮不做。Herdr 以后。
