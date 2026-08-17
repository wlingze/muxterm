# 产品愿景 vs 实现 vs 测试（2026-08-17）

> 对照：`PRODUCT.md`、`docs/PRODUCT-VISION-STRATEGIC-REVIEW.md` §2.14 / §2.15 / §6 / §9。
> 修订：`2026-08-17T12:07:06+08:00`。W15/W16 已绿。当前执行：[`W17-PLAN.md`](W17-PLAN.md)。

愿景 1.0 = **A（日用）+ B（可见性）+ C（搜索）**。D/E、Herdr、手机、账号、AI Chat、持久化索引不是 1.0。

---

## 一句话

Linux tmux 路径已经能 attach、切 tab、搜已连接工作区、红点/peek/回复、断线留最后一帧。
1.0 测试门禁还差四件，由 W17 锁住：自动重连、scroll lock、搜索滚到命中行、Done/前台静默/静音 live。

---

## 阶段 A — 日常终端

| 愿景项 | 代码 | 测试 | 判据 |
|---|---|---|---|
| 本地 attach / 切 tab / Surface | 有 | attach/live/render e2e | 够用 |
| 客户端 scrollback | `capture-pane -S -N` + VTE | `linux_attach_history_e2e` | W16a 绿 |
| 回底按钮 | `muxterm-jump-latest` | 同上 | W16a 绿 |
| Scroll lock | feed 仍可能拽回底部 | `linux_scroll_lock_e2e` | **W17b** |
| 断线水印 | overlay，不关窗 | `linux_disconnect_e2e`（kill-server） | W16b 绿 |
| 自动重连 + 不漏事 | 无 | `linux_reconnect_e2e` | **W17a** |
| 粘贴剥控制字符 | `sanitize_paste` | mirror 单测 | 够用 |
| 系统通知 fail-soft | GioSink 无 app 直接 return | `gio_sink_without_app_does_not_panic` | 单测锁住 |
| ET / 托盘 / 多窗口 | 无 | — | 不是 1.0 |

---

## 阶段 B — 可见性

| 愿景项 | 代码 | 测试 | 判据 |
|---|---|---|---|
| BEL / OSC 133 | 引擎 + live `%output` | feature e2e / semantics e2e | 够用 |
| blocked 看见不熄、输入才熄 | 转移表 + live | `linux_attention_semantics_e2e` | W16c 绿 |
| TOML 正则 | `blocked_regex` | 同上 | W16c 绿 |
| peek + 一行答复 | 小 VTE | feature e2e W15e | 够用 |
| 静音 | 面板 mute-1h | panel e2e 点按钮；**live 再 BEL 不亮是 W17d** | **W17d** |
| 前台 Done 不通知 / 后台 Done 看见即熄 | 引擎有；live 缺无 BEL 脚本 | `linux_attention_1_0_e2e` | **W17d** |

---

## 阶段 C — 搜索

| 愿景项 | 代码 | 测试 | 判据 |
|---|---|---|---|
| `search_all` 内存 | PaneBuf | feature e2e | 够用 |
| 跳到命中 tab/pane | SwitchTab+SwitchPane | `linux_search_jump_e2e` | 够用 |
| 滚到命中行并高亮 | 丢掉 seq | `linux_search_highlight_e2e` | **W17c** |
| 上次看到这里 / 命令轨 | 无 | — | 不是 1.0 完成定义 |

---

## SSH / 以后

真 SSH attach 仍 `#[ignore]`。W15c/d 只保证不冻窗、有灯。不要把 ignore 改成默认绿。
Herdr、ET、多窗口、上次看到这里：以后。
