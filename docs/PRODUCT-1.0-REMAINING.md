# Muxterm 1.0 还差什么

> 梳理时间：2026-09-09（Asia/Shanghai）
> 愿景底本：`docs/PRODUCT-VISION-STRATEGIC-REVIEW.md`（A+B+C = 1.0）
> 日用债：`docs/dogfood-0907-replay.md`
> 结论：**离目标不远。** 缺的是把已经开过的环收口，不是再开一条产品线。

定案身份没变：一扇窗、一个面板、一枚红点。工作跑在不会睡的机器上；Muxterm 是望向它的那扇窗。

本轮负责人拍板要进 1.0 的，在愿景 A/B/C 之外加了两件远程连续性：

- 剪贴板图片/文件投到当前远程工作区
- SSH 里起了端口 → 提醒 → 映射到本机

菜单栏常驻、全局热键、重启恢复、`muxterm status --json`、手机、账号，**不进这轮 1.0**。

---

## 0. 已经站住、不必再当「新功能」开题

- 一个窗口多工作区；本地 / SSH tmux；Herdr 已能当 Runtime 用
- 切 Tab / Workspace 的数字键；侧栏
- Attention 列表、状态机、通知
- 搜索入口、回底、命令刻度（macOS 第一版）、上次看到这里（按钮级）
- 设置页、QuickConnect、mute、一行答复 overlay 雏形

这些证明核心循环的前半段（打开、切换、看见有事）已经存在。缺的是后半段：**判断清楚、答一句、远程干活不跳出窗口**。

---

## 1. 四波收口（按这个顺序，不要并行铺开）

### 第一波 — 阶段 B 做成闭环（必须先做）

完成定义（愿景原文）：一个 pane 从「跑完 / 卡住 / 在问我」到你知道，**< 5 秒、零误报**。产物是跨机聚合体。

还没锁住的：

| 项 | 要成什么样 |
| --- | --- |
| Peek | Attention 行选中即显示该 pane **最后 N 行静态文本**，不是活终端 |
| 一行答复 | **先 peek 再输入**；只发一行+回车；面板不关，原地刷输出；输入框旁常驻 `工作区 · 机器 · agent` |
| 列表形态 | 按 **工作区** 汇总（一行 + 计数），点开才是 pane；两个 `muxterm` 必须带 `local` / `ryzen` |
| 红点口径 | 只算 blocked + 未看过的 done；未读输出永不进红点 / Dock badge |
| 键盘 | `Cmd+Option+1..9` 切 Agents；面板 key 时 `Ctrl+1..9` 切当前列表 |
| Title | Herdr 用协议 `title`；tmux Codex 用 `#{pane_title}` / OSC 0，不要刮画面 |

第一波同时消化 dogfood §16 之外、和「谁在等我」绑在一起的项：§19 机器标记、§20 agent 键与 title。

**明确不要在 B 里做：** Codex 专属按钮、agent 名单集成、识别「这是哪家模型」。title 是导航，不是 agent UI。

### 第二波 — 地标 + 搜索当导航

时间地标（§2.15.4）三件套共用副本坐标：

| 项 | 要成什么样 |
| --- | --- |
| 上次看到这里 | 历史上画线：`── 上次看到这里 · 2h 前 ──`，不要只靠一颗按钮 |
| 命令刻度 | 滚动条窄轨；点击跳命令；跟随时可藏；`[` / `]` 跳转补完 |
| 回底 | 离开几行才显示胶囊；贴尾才藏；**迟滞、不闪**（dogfood §17 / §22）；agent TUI 默认不吸附、可不显示胶囊 |

搜索（阶段 C，但 1.0 要「能用」）：

- 一个面板三个 tab：工作区 / 待处理 / 搜索，共用输入框，Tab 换范围、查询保留
- 命中 **跳到那一行**，不是只切 pane
- 只搜已连接工作区的内存副本，不落盘

Cmd-P → 工作区 tab；红点 / Cmd-R → 待处理 tab；搜索 tab 激活才跑查询。

### 第三波 — 日用 A 收口

愿景点过名、dogfood 也在用的「不可缺」：

| 项 | 要成什么样 |
| --- | --- |
| Cmd-W | 关 Settings/面板，否则一个个关 pane；关窗口用 `Cmd+Shift+W`（§16） |
| 选词 | 双击渐进扩大；拖选按空白 word（§18） |
| URL / 路径点击 | 点链接用系统打开 |
| `Cmd-\`` | 在上一个工作区来回（面板默认落点也是上一工作区） |
| `twork` 进产品 | `muxterm ~/proj` / `muxterm .` = attach-or-create；面板「新建」走同一条 |

### 第四波 — 远程连续性（本轮新加，进 1.0）

图片粘贴和端口转发都挂在 **当前 SSH 工作区** 上，不另做网关、不做账号。

---

## 2. 图片粘贴（本地 → 当前远程工作区）

愿景 §4.2 已经写过，场景只有一句：

> 截了个图，想让远程那个 agent 看看。

**最小版本（1.0 只做这个）**

1. 系统剪贴板里是图片或文件（Cmd-V，或「投递剪贴板」动作）。
2. 写到当前工作区远端约定目录：`~/.muxterm/inbox/<ts>-<name>`（走现有 SSH alias，和 tmux 同一台机器）。
3. **把远端路径贴进当前 pane 输入行**，人看得见文件去了哪。
4. 本地 pane：同样落到本地 inbox，再贴路径（agent 吃路径；不在 1.0 做内嵌 bitmap 协议）。

**纪律**

- 单向投递，不是剪贴板双向同步，不是网盘。
- 不渲染远程 Sixel/Kitty 图片。
- Muxterm 只清理自己的 inbox（TTL），绝不扫家。
- 粘贴文本仍走现在的系统剪贴板覆盖语义（dogfood §3），和图片投递是两条路：剪贴板是图 → 投递；是字 → 当字贴。

后做（不算 1.0）：拖拽文件、投递前预览、从远端取回。

---

## 3. SSH 端口转发（Cursor / VS Code 那类）

对照：[VS Code Remote-SSH 转发端口](https://code.visualstudio.com/docs/remote/ssh)（核对 2026-09-09）。他们是：终端/调试输出里看到端口 → 自动或手动 `ssh -L` 到本机；本机被占就换端口并告诉你 `http://127.0.0.1:xxxx`。Muxterm **不要默默全转**（VS Code 的 autoforward 很容易吵），要 **发现 → 提醒 → 人点一下再转**。

**发现**

只在 **SSH 工作区** 的 pane 输出里认「开始监听」：

- `http://127.0.0.1:5173` / `localhost:3000` / `0.0.0.0:8080`
- Vite / Next / webpack / uvicorn 那种 `Local: http://localhost:…`

同一工作区同一端口只提醒一次，直到进程没了或人忽略。不要扫文档里随口写的 `:8080`。1.0 不做远端 `ss`/`lsof` 轮询（要另开 SSH 命令，脆）。

本地工作区不需要转。Herdr 若端口不在这台 SSH 后面，1.0 不做。

**提醒**

系统通知或应用内条：

```text
ryzen · muxterm  远端 :5173 在听
[转发到本机]  [忽略]
```

点转发才动手。忽略后这个端口本会话不再烦。

**转发**

- 用该工作区已有的 SSH alias：`LocalForward 127.0.0.1:<local> 127.0.0.1:<remote>`
- 只绑本机 loopback，不对外、不公开隧道
- 本机端口被占 → 换一个，通知里写清 `http://127.0.0.1:<local>`
- 工作区断开 / 关掉 → 拆掉这条转发
- 仓库里已有 `herdr/forward.rs`（socket `-L`），应用端口是同类：多一条用户可见的 TCP `-L`

**不做**

- 默认自动全转
- 转发到 `0.0.0.0` / 公网 tunnel
- 持久化成 `~/.ssh/config` 的 `LocalForward`（那是用户自己的事）
- 做成 VS Code 那种完整 Ports 面板（1.0 有通知 + 当前工作区已转列表就够）

---

## 4. 和 dogfood 记录怎么叠

`docs/dogfood-0907-replay.md` 是「重构后按行为重做」的问题账。叠到上面四波里：

| 波 | 吃掉的 dogfood 节 |
| --- | --- |
| 1 B | §19 机器标记、§20 agent 键/title/OSC |
| 2 地标/搜索 | §9 刻度与回底、§17 误跳、§18 拖选、§22 胶囊闪 |
| 3 日用 | §2–§8、§10、§16 Cmd-W |
| 4 远程 | 新开；图片走愿景 §4.2；端口转发是本轮新加 |

重构落地后：**先按 replay 把已有行为迁到新树，再按本文件的四波往 1.0 收。** 不要在旧抽象上把 B 和端口转发做完再 rebase。

---

## 5. 不进这轮 1.0

- 窗口可关 + 菜单栏常驻感知、全局热键、重启恢复布局
- `muxterm status --json` / 只读 MCP
- 手机、账号、云、AI Chat、agent 编排看板
- 持久化搜索索引、跨天历史
- 插件系统、自研多路复用 daemon
- mosh 预测回显、Sixel 显示远程图

这些要么是形态级（菜单栏）适合 1.0 之后打磨，要么愿景里写了永不做。

---

## 6. 2026-09 对手位（Superlogical / Herdr）

核对时间：2026-09-09T16:33:46+08:00（Asia/Shanghai）。

三层不要打成一团：

| 层 | 他们在做 | Muxterm |
| --- | --- | --- |
| **Runtime**（会话活在哪） | Herdr：agent 进程的家；Superlogical：下一代 mux，想替 tmux/SSH | 已经接 Herdr，tmux 继续用 |
| **Client**（人从哪看） | Herdr 自带 TUI；Superlogical 预告 web + macOS/iOS 原生 | **这是 Muxterm 的座位** |
| **编排** | Herdr 插件/agent 互调；Superlogical 远期人+AI 工作平台 | 明确不做 |

**Herdr（2026-09-07/08）**

- 0.9：本机 + SSH 机器进同一个 TUI（「Connecting the machines」）。
- 次日：$6M seed（Bessemer 领投，YC / e2vc，Tobi Lütke、Dane Knecht 等）。
- 公开叙事：runtime 仍 Apache-2.0；钱用来招人；下一步 Herdr Cloud。
- 产品句：agent 活在后台 server 里，working/blocked/idle，关客户端工作不停。
- 来源：herdr.dev 博客摘要、YC 公司页、GitHub `herdrdev/herdr`。X 上用户在说 0.9 多机、手机看卡住、agent 可编程控 tab。

对 Muxterm：这是 **Runtime 变强**，不是来抢原生窗口。继续当一等 Runtime 用；不要去做第二个 Herdr TUI。他们做 Cloud/账号的时候，Muxterm 的「零账号客户端」才更值钱。

**Superlogical（2026-09-02/08）**

- Hashimoto 等，libghostty 上的 **server-side mux**：server 持 PTY，原始字节 tee 给客户端，不再 tmux 那种双重仿真。
- 9-02：pre-alpha demo（关了再开计数还在、原生滚动、session）。
- 9-08：远程持久 session、远端列目录；Hashimoto 称 **可以当 SSH 替代**（自己的服务器已不跑 sshd），也仍可用 Superlogical-over-SSH。
- 官网仍是 waitlist；web + macOS/iOS、现场分享是规划，不是已交付客户端。
- 来源：[superlogical.com](https://superlogical.com/)、[X @mitchellh 2097424868203758046](https://x.com/mitchellh/status/2097424868203758046)、YouTube pre-alpha（2026-09-02）。

对 Muxterm：他们在换 **地下管道**（mux + 传输）。1.0 不要跟他们造 mux。观察点只有一个：协议开不开。开了就当 Transport/Runtime 接；不开就继续吃 SSH+tmux/Herdr。他们的 native 客户端还没上市，窗口座位现在是空的。

**因此 1.0 的差异化只能写在 Client 层：**

原生一扇窗 + 跨 tmux 与 Herdr 的注意力闭环 + 不搬家、零账号。管道谁强接谁。

---

## 7. 距离判断

不是差一个新产品。差的是：

1. Attention 从「列表」变成「发现 → peek → 答一句」
2. scrollback 上的线、轨、回底要稳
3. 搜索要跳到坐标
4. Cmd-W / 选词 / 链接 / 建区这些日用刺
5. 远程截图和远程端口不必再开一个终端

对手加速 **不改变** 下一刀：还是把 B 做成闭环。Herdr 融资和 Superlogical demo 证明「谁在等我 + 持久会话」是真市场；他们占 Runtime，Muxterm 占窗。跟他们造 mux / 再做一个 agent TUI，才是走远。

四波都是现有 Workspace + SSH + 面板上的增量。B 做成闭环之后，对外那句才站得住：

> 几十个在飞的工作区在这一个窗口里，我一眼知道哪个在等我；远程的图和端口也还在这扇窗里处理。
