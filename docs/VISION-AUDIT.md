# 产品愿景 vs 实现 vs 测试（2026-08-17）

> 对照：`PRODUCT.md`、`docs/PRODUCT-VISION-STRATEGIC-REVIEW.md` §2.14 / §2.15 / §6 / §9、
> `docs/WORKSPACE.md`、`docs/SURFACE.md`。
> 时间：`2026-08-17T02:43:19+08:00`（本机 `date -Iseconds`）。
> 结论只看代码和会跑的测试，不看过期矩阵。`docs/TESTING.md` §7 里 E1–E6 那几行已经过时。

愿景自己把 1.0 写成 **A（日用）+ B（可见性）+ C（搜索）**。D（Herdr）/ E（投递）不是完成条件。
永不开工：手机端、自研 daemon、账号/云、AI Chat、持久化搜索索引、agent 专属 UI。

本仓库当前是 Linux + tmux `-CC`。macOS 是上一代 ConnectionPool，不在本轮对齐范围。

---

## 一句话

没有做完。本地 attach、切 tab、Surface、搜索索引、注意力引擎这些已经能用，用户 2026-08-17 也说切换没问题。
愿景里当作 1.0 地基的几件还没落地：attach 只抓可见屏、断线没有水印、回底按钮没有、正则 blocked 没有 live e2e。
W15（流量 / 跨 tab 搜索 / peek 回复 / 连接超时 / SSH 灯）还在 Codex 手上。

---

## 阶段 A — 日常终端

| 愿景项 | 代码 | 测试 | 判据 |
|---|---|---|---|
| 本地 `tmux -CC` attach，tab/pane 原生 UI | 有。W13 播种 + pause + 切 tab 像素缓存 | `tmux_attach_contract`、`linux_workspace_attach_e2e`、`linux_live_e2e` | 够用 |
| 单窗口装多个工作区 | `WorkspacePool` 在 core | quickconnect / attach e2e | 够用 |
| 客户端 scrollback 为事实源 | PaneBuf + VTE `scrollback_lines` 配置已接。**attach 仍是 `capture-pane -e -p` 只抓可见屏** | emulate 有界；**没有**「滚出可见区的 token 在 attach 后还能搜到 / 滚到」 | **缺。W16a** |
| 选择 / 复制 / URL 点击 | VTE 选择；OSC 8 + regex URL | `linux_render_e2e` URL | 基本够 |
| 粘贴剥控制字符（CVE-2026-26982 同类） | `sanitize_paste`：保留 `\n\r\t`，去掉其它 `0x00..=0x1F`；window 粘贴先 sanitize 再 encode | mirror 单测有 ESC/BEL/`\0` | 单测够；无 GTK 粘贴 e2e，本轮不补 |
| 向上滚 = scroll lock，回底按钮 | VTE 默认不跟滚。**没有** `muxterm-jump-latest` | `test_scroll_pane_to_top` 只给 mock-codex 读头 | **缺。W16a 同 crate** |
| 断线水印 + 自动重连，不弹窗 | `BackendStatus::Disconnected` 只改状态栏。VTE 不拆，也没有 overlay。Exited 还可能关窗 | 无 | **缺 overlay。W16b**。自动重连 / catch-up 一次渲染 → 之后 W17 |
| Eternal Terminal 传输 | 无。SSH 是 Transport | 无 | 以后。mosh 明确不做（与 `-CC` 不兼容） |
| 配置 TOML 唯一事实源 | `~/.config/muxterm/config.toml` + 偏好窗写回 | `linux_prefs_e2e` | 够用 |
| 快捷键 Alt+N/T/D/1-9/[]/P | keymap 有 | 部分 chrome e2e | 未全覆盖，不是 1.0 卡点 |
| tmux 与非 tmux 体验一致 | Shell Runtime 有；QuickConnect 可 attach | 分套件 | 大体有 |

`PRODUCT.md` 里「每个 pane 一个 Notebook tab」「底部输入框」是旧图，不要当现状。

---

## 阶段 B — 可见性

完成定义：状态变化到人知道 < 5 秒，红点只表示 blocked 工作区，产物是跨工作区聚合。

| 愿景项 | 代码 | 测试 | 判据 |
|---|---|---|---|
| BEL / OSC 133 → 状态机 | `AttentionEngine` + PaneBuf 信号 | emulate fixture；W15e 要真 `%output` BEL | 引擎有。live BEL 是 W15e |
| blocked 只在输入后熄灭，看见不算 | `Blocked + BecameVisible` 保持；`UserInput` → Idle | `attention/state.rs` 穷举表 | **单测有。缺 live e2e。W16c** |
| done 看见即熄 | `Done + BecameVisible` → Idle；前台 CommandDone 当已看见 | 单测 + W14 后台 Done 通知 | 后台 Done 有。前台 `ls` 不进列表靠 apply 后 `on_became_visible` |
| 红点 = blocked **工作区**数 | `blocked_workspace_count` | 单测两 pane 一区 = 1 | 有 |
| 跨工作区聚合列表 + peek + 一行答复 | Attention tab；peek VTE；W15e 接真字节和回复 | panel e2e 钩子；live 回复是 W15e | W15 做完才算闭 |
| 静音 1 小时（无「全部已读」） | `mute_for`；面板 5m/10m/30m/1h/4h/24h | panel e2e 点 mute-10m | 有 |
| 用户 TOML 正则 → blocked | `AttentionConfig.blocked_regex`；偏好窗能编辑 | 引擎 debounce 单测；config 解析单测；**无 attach e2e** | **缺 live。W16c** |
| 系统通知 fail-soft | `GioSink` | 无「Gio 失败不崩」用例 | 有代码，本轮不新开 crate |
| Linux 托盘 / 关窗感知常驻 | 无（macOS 菜单栏项是另一套） | 无 | 以后。Linux 先靠状态栏红点 |
| 多 OS 窗口 + `⧉N` | Linux 一个 `AppWindow` | 无 | v1 以后 |

---

## 阶段 C — 搜索

愿景：只搜已连接工作区内存，不落盘。发布后第一个大更新；Linux 已经提前做了。

| 愿景项 | 代码 | 测试 | 判据 |
|---|---|---|---|
| `search_all` 走 PaneBuf | W14 | `linux_feature_e2e` 真 attach | 有 |
| 跳到命中 tab/pane | W15b | `linux_search_jump_e2e` | W15 中 |
| 滚到命中行并高亮 | 尽量滚；overlay 高亮未做 | jump e2e 不要求高亮 | 高亮以后 |
| 「上次看到这里」分割线 | 无 | 无 | 愿景写明不进 B 的完成定义 |
| 命令时间轴细轨 | 无 | 无 | 与 C 同期甜点，本轮不做 |

没有历史播种的话，搜索只能命中 attach 之后的输出。C 的地基仍是 W16a。

---

## 明确以后 / 永不

| 项 | 处理 |
|---|---|
| Herdr Runtime（阶段 D） | 不做。等 tmux 路径和 Runtime trait 稳定 |
| OSC 52 远程剪贴板 | 愿景有，没做 |
| 后台 pane 压缩 scrollback / 全局 256MB 预算 | 有 `buffer_cap` 字节上限，没有按活跃/后台缩额 |
| libghostty-vt 评估 | time-box spike，B 之后 |
| 多窗口规则 / 工作区互换 | 设计已批，Linux 未做 |
| Superlogical / 自研 daemon | 永不自研；对方协议开放再当 Runtime |
| 手机、账号、AI Chat、持久化索引 | 永不 |

---

## SSH

`TmuxRuntime::new_ssh_attach` 有。`tmux_ssh_feature_contract` 是 `#[ignore]`（要 `scripts/ci/setup-sshd.sh`）。
`linux_ssh_e2e` 曾经把 CoreBridge 事件喂进另一个 Mock Workspace，**不能**当成 SSH attach 已证明。
W15c/d：主线程 `block_on` 冻窗、可达性灯。本轮不把 ignore 强行改成默认绿。

---

## 本轮补什么（W16）

只补愿景 1.0 里、Linux tmux 路径上、现在就能写红灯的缺口：

1. **W16a** attach 历史：`capture-pane -e -p -S -N` 进 PaneBuf 和 VTE；滚到顶能看见离屏 token；回底按钮
2. **W16b** 断线水印：隔离 server 被杀后窗口还在、VTE 还在、`muxterm-disconnect-overlay` 可见、没有模态框
3. **W16c** 注意力语义 live：看见 blocked pane 红点不灭；输入才灭；TOML 正则能把后台 pane 点亮

W15 必须先绿。不要并行开第二个 Codex 写手。

不在 W16：自动重连 catch-up、ET、多窗口、上次看到这里、命令轨、Herdr、像素重写、push。
