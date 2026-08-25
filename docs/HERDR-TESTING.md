# HERDR-TESTING.md — Herdr、Runtime×Transport 与 CI 验收契约

> 状态：实施前测试定案（2026-08-22）
> 最终复核：`2026-08-22T19:31:35+08:00`（CST；本机版本、官方 release metadata、
> PR #20 required checks 均已重新只读核对）
> Runtime 设计：[`HERDR-RUNTIME-STABILITY.md`](HERDR-RUNTIME-STABILITY.md)
> 总测试规范：[`TESTING.md`](TESTING.md)
> 实施计划：[`../.plan-herdr-runtime-stabilization-20260822.md`](../.plan-herdr-runtime-stabilization-20260822.md)

本文回答两个问题：

1. 为什么此前 runtime×transport 测试曾经绿色，真实 Herdr 手动使用仍严重失败？
2. 修复后什么证据才足以称为“Herdr runtime 可用、本地成功可以合理预测 CI”？

本文是 Herdr 稳定化的专项门禁。它不删除 [`TESTING.md`](TESTING.md) 中已有用例，
而是提高 Herdr、tab/pane 和 claimed matrix 的完成标准。

---

## 1. 旧测试实际证明了什么

历史绿色基线 `c61ea92` 的六格 GTK matrix 使用新建、干净、短输出 fixture，验证了：

- runtime/transport 注册表能产生 tmux/herdr/shell × local/SSH 的路径；
- 2 tab/3 pane 的基本拓扑可以建立；
- 短单行 token 能从服务端路由到目标 Workspace/VTE；
- 支持 PersistDetach 的 runtime 在理想条件下可以 detach/attach；
- named Herdr session 与隔离 tmux socket 没有破坏用户默认 session。

这些测试有意义，但只证明上述边界。它们没有证明：

- 已运行很久、已有多 tab 和 100–500KB 历史的 Herdr workspace 能正确 attach；
- full frame、incremental frame、pane.read 和 GTK grid 在竞争时仍有正确顺序；
- 真实 `+` 按钮与真实 Alt+T key controller 接线正确；
- 创建响应先于 event snapshot 时，新 pane 一定会获得 stream；
- stale Close/Error、重复重建和 takeover conflict 有界；
- Ctrl-L 后旧屏不会因 snapshot/reseed 复活；
- Project 持久化后和 Existing Connection 仍指向同一 Herdr identity；
- `--test-threads=1` 能隔离 GTK 全局状态——它不能，同一 test binary 仍是同一进程；
- 旧 commit 的绿色能代表当前 HEAD。

此外，测试直接调用 `test_handle_action()` 或 `test_poll_once()` 会绕过生产 widget、
key controller 或 GLib timer。这样的测试只能证明 helper，自身不能证明用户路径。

结论：旧结果应表述为“短 fixture 下的拓扑/路由基线曾通过”，不得表述为
“Herdr 手动使用已通过”。

---

## 2. 验收层次

### 2.1 L0：纯逻辑和 wire 单测

不启动 GTK，不使用真实默认 server。必须先 RED 再 GREEN。

| 领域 | 必须覆盖 |
|---|---|
| Stream mode | ObserveTerminal 与 ControlTerminal 的 wire message 精确不同；open/reattach/Pool activate 首次 Control 为 `takeover=false`，只有真实 focus/input intent 可为 true |
| Generation | old generation 的 Frame/Closed/Error 全部忽略；event ordinal 递增；wire seq 重复/倒序丢弃，diff gap 失败，full gap 可重建 baseline |
| Registry | 一个 pane 不能有两个 Starting/Live；重复错误只能安排一次 retry |
| Backoff | 重试间隔固定 `100/200/400/800/1600ms`，第五次后 Degraded；只有 full 后连续 Live 10 秒才恢复普通故障预算，短 flap 不恢复 |
| Rearm | 重复 snapshot/layout/resize 不重置 retry budget 或 control intent；只有新本地 focus edge/input、target reopen 或稳定窗口能按各自规则重置 |
| Takeover | current control 被 taken over 后进入 suppression 并降 Observe；重复 reconciliation 仍不得反抢；新用户 intent 最多 promote 一次 |
| Detach | Detached/Stopped registry 收到旧事件不能重启 |
| Frame bootstrap | full 前最多 256 event/2 MiB、首个 full deadline 5 秒；溢出/超时使 generation 有界失败；full 后只追赶更大 wire seq；切 generation 不混帧 |
| Input handoff | Control Starting 时 256 write/64 KiB 有界排队，绑定 control intent 并可跨同一 intent 的自动 retry；latest resize 先发，input 按序恰好一次；stale/detach/suppression 不 flush |
| NewTab JSON | `name=None` 完全没有 label；Some 才序列化；空响应 label 按 protocol-19 bijective base-32 public id 解码成十进制数字；非法 id 不得变成 0/空名 |
| Mutation outcome | 入队返回 `Accepted(operation_id)` 而不是 `Done`；最终 `MutationSettled` 恰好一次；队列满/超时/收敛失败带明确阶段，Accepted 不能触发主动 refresh |
| Mutation FFI | 旧 execute ABI 不变；additive JSON execute 返回 Accepted operation id；`STATE_MUTATION_SETTLED=16` 用既有 data buffer 传完整 JSON；Linux/macOS/TUI 不吞异步失败 |
| Workspace event FFI | 新 wrapper 为 active/background 每个 event 保留完整 WorkspaceId；两个 Workspace 都有 PaneId(1) 时不串流；旧 poll 只返回 active；`CStateChange` size/offset 与 `window_id=0` 不变；`PaneIndexSnapshot` 不出 FFI |
| Mutation convergence | create response/snapshot 两种先后顺序结果相同；NewTab 的 active tab/focus/layout 和 Split focus 未权威一致时保持 Pending；新 pane 只初始化一次；并发双操作经有界 FIFO 串行且各用 dispatch-time baseline；probe 为 dispatch 后 100/250/500/1000/2000/4000ms，enqueue 起 5 秒总 deadline；新 pane full baseline 共用剩余 deadline，不能再追加 5 秒 |
| Split convergence | 含新 pane 但 focused_pane 仍旧时保持 Pending |
| Project store | session/socket/workspace_id round-trip；旧 TOML 能迁移；path 不再兼任 workspace id |
| Project FFI | 旧 workspace_open ABI 保留为低层 None descriptor；open_target_json 走 resolver；workspace list 的 optional resolved_target round-trip 不丢 path/socket/workspace_id |
| Resolver | 精确 identity、唯一 label、歧义；`AttachOnly` 零创建；`CreateIfMissing` 只在显式/唯一 selected local session 创建，缺选择不偷用 default；SSH no-start；open 后保存 canonical descriptor |
| Identity key | session/target-side socket/workspace_id 参与 identity；name/path 不参与；同名不同 server 不合并；五段 WorkspaceId collision 明确失败；candidate 转 TargetConfig 不经过 Linux socket 推导 |

测试使用 fake clock 或显式 `Instant` 注入验证退避，禁止实际 sleep 1.6 秒。

### 2.2 L1：Core + 真实 Runtime contract

启动真实隔离 tmux/Herdr，不启动 AppWindow。

- Herdr 只使用 `muxterm-test-<pid>-<case>` named session。
- tmux 只使用 `tmux -L muxterm-test-<pid>-<case>`。
- 创建 4 tab；至少一个 tab 有 3 pane；逐 pane 写唯一短 token。
- 用服务端 snapshot/read 与 Workspace State 同时断言 tab、pane、layout、focus。
- 注入重复 stream close/takeover 后断言 transition/retry 计数，而不是只等最终 token。
- 在 Observe→Control 握手窗口输入 token，断言服务端和目标 pane 恰好一次，非目标 pane
  为零；队列溢出必须得到显式错误。
- detach/reattach 后所有 pane token 仍能被服务端和 Workspace 找到。
- 测试结束只清理自己创建的 named session/socket。

### 2.3 L2：GTK/VTE Surface

每个场景只创建一个 AppWindow，并在独立进程运行。

- 查找真实 widget_name 后触发 `gtk4::Button` clicked signal。
- 按键通过窗口的真实 `EventControllerKey` 投递 Alt+T、Alt+S、Alt+V、Ctrl+L。
- 只 pump 生产 GLib main context；禁止 `test_handle_action()`、`test_poll_once()`
  代替生产接线。
- 断言唯一目标 VTE 的可见文本、cursor row、grid size、RenderTrace seed/reset，不能只查
  Core 或 Herdr API。
- “token 恰好一次”必须可判定：token 控制在单个 visual row 内，发送到 shell 的命令文本
  不得原样包含完整 token（可拆成两个片段后 `printf`，或在 fixture 中关闭 echo），避免把
  命令回显和真实输出各算一份后再随意放宽断言。
- dispatcher 必须按 `(WorkspaceId, PaneId)` 查常驻 Surface；hidden tab/background workspace
  的 frame/output 在隐藏期间继续 feed，不能只更新 PaneBuf/attention。
- 所有等待使用 `wait_until` + 硬 deadline；禁止加长裸 sleep 修时序。

默认键位的事实来源是 `src/core/config.rs::default_keybindings()`：Alt+T 是 NewTab；Alt+N
是独立的 NewWindow action。`keymap.rs` 中 Alt+N→NewTab 只出现在“用户自定义覆盖”的测试，
不能用来改写 `1531` 的 Alt+T 回归路径。若保留 Linux 当前把 NewWindow 映射为同一
`Task::NewTab` 的行为，可另加 Alt+N alias contract，但它不能替代 required Alt+T child。

### 2.4 L3：Runtime×Transport 产品矩阵

tab/pane 稳定性必须覆盖以下四格：

| Runtime | Local | Loopback SSH |
|---|---|---|
| tmux | 必跑 | 必跑 |
| Herdr | 必跑 | 必跑 |

现有 shell × local/SSH 两格兼容 coverage 保留，但不能替代四格。harness 必须通过
runtime/transport 注册表和 `accepted_transports()` 枚举所有 accepted cell：tmux/Herdr
四格运行本专项 required 场景，shell 两格运行已有兼容场景；新注册且 accepted 的 cell 若
没有明确 fixture/scenario manifest，parent 必须失败而不是静默漏测。禁止为当前列表写一个
不随注册表变化的“看起来像矩阵”的固定循环。

SSH 自动化只证明 LoopbackSshd、显式 ssh_config 和 Unix socket forward。任意真实 Host
仍需单独手动/外部环境验证。

---

## 3. 进程隔离

`--test-threads=1` 只串行调度 test function，不会重置 GTK、GLib、VTE、全局 SourceId、
环境变量或 AppWindow 单例。Herdr matrix 和 agent e2e 必须使用子进程隔离。

推荐的测试 harness 形状锁死为：

```text
parent test
  -> runtime_list × transport_list
  -> 对每个 accepted cell、每个 scenario 启动 current_exe 子进程
     env: MUXTERM_TEST_CHILD=1
          MUXTERM_TEST_RUNTIME=<id>
          MUXTERM_TEST_TRANSPORT=<id>
          MUXTERM_TEST_SCENARIO=<name>
     args: --exact isolated_matrix_child --nocapture --test-threads=1
  -> 收集 exit status；任一 child 非零则 parent 失败

isolated_matrix_child
  -> 未设置 MUXTERM_TEST_CHILD 时立即返回
  -> 初始化一次 GTK
  -> 创建一个 AppWindow
  -> 创建一套独立 fixture
  -> 执行一个 scenario
  -> drain/close/cleanup
```

父进程负责动态枚举和汇总，子进程负责真正 GTK 场景。不得把六个 cell 放在一个
AppWindow 里循环，也不得在同一 test binary 顺序创建多个 AppWindow 后称为隔离。

超时预算固定为：普通 child 15 秒，`large_history_*` 与 `takeover_watchdog` 30 秒，完整
parent matrix 15 分钟。parent 超时后必须终止 child、保存其 stdout/stderr/artifact，并按
事先分配的**精确** named Herdr session/tmux socket 做幂等清理；不能依赖被强制终止进程的
Drop。不得通过放大 timeout 掩盖收敛失败。

---

## 4. 必须新增的 RED 场景

### 4.1 `herdr_large_history_attach_is_surface_correct`

Fixture：

- 4 tab，目标 tab 至少 3 pane；
- 各 pane 生成不同的短单行 token；
- 动态生成约 100KB、393KB、500KB 三种历史量，不提交巨型 fixture；
- 在 attach 前完成数据生成，确保不是“连上后 echo 一行”的空 session 冒充。

断言：

- attach 后每个 tab/pane 的服务端、Workspace 和唯一 VTE token 一致；
- 连续切 tab 20 轮，token 恰好一份；
- 第一次切入此前从未显示的 hidden tab 时，VTE 已含隐藏期间的最新 token，且该次切换的
  seeds/resets 增量为 0；
- prompt/cursor 在最后两行内，不固定停在 3/4 高度；
- 首次 seed 后切换的 `seeds`、`resets` 增量均为 0；
- 没有把 `pane.read visible_ansi` feed 给 PaneView 的诊断记录。

Local Herdr 与 loopback SSH Herdr 都必须跑。

### 4.2 `new_tab_button_uses_production_path`

- 找到真实 `+` widget 并 emit clicked；
- 5 秒内 Herdr snapshot 增加一个 tab/root pane；
- Workspace 和 GTK 同时出现；
- current generation 已为 Live 且 full baseline Ready；不能把 Starting 当完成；
- 输入 token 后只出现在新 VTE；
- 不调用 action helper，不手动 refresh_ui。

### 4.3 `new_tab_shortcut_uses_production_key_controller`

- 向真实窗口投递 Alt+T；
- 断言条件与按钮场景相同；
- 新 tab 名与 Herdr 权威 label 相同、非空，且数字与 public tab id 后缀一致；删除造成的 id
  缺口不要求重编号；
- 请求 payload 没有空 label。

该场景在四格共用同一个真实 Alt+T 入口，但断言分两层：所有 Runtime 都要求新 tab 可见、
顺序标签非空且新 VTE 可输入；只有 Herdr local/SSH 要求 raw authority label 为数字并检查
wire payload 省略空 `label`。tmux 的 raw window name 允许保持 tmux 语义。

### 4.4 `herdr_attach_split_incident`

这是 `test_2026-0824-1856.log` 的独立 Herdr-local 回归，不由空 workspace 或普通
`large_history_*` 场景替代：

- attach 前先在已 populated 的 2-tab/3-pane workspace 写入固定的约 `223,320` bytes
  baseline，并等待 `MX_INCIDENT_BASELINE_DONE`；历史生成不计入 GTK attach 计时；
- 通过生产 Existing Connections row attach，确认同一 named session + workspace identity；
- attach 后立即走真实 Alt+S、Alt+V、`+`，再通过 VTE commit 执行 echo；
- 断言服务端/Core/目标 VTE 的新 token 与 baseline 都存在，pane geometry 有效，child 正常退出；
- 在 `G_DEBUG=fatal-criticals` + Xvfb 下运行，任何 SIGABRT/SIGSEGV 或 timeout 都失败。

`pane.read` 单次 JSON 响应可能小于完整历史，测试不把返回长度误当成历史大小；固定大小由
fixture 的 `head -c 223320` 生成命令保证，marker 证明命令完成。

### 4.5 `direct_reattach_*`

wire 回归保留两条独立断言：plain token 的 detach/reattach 后旧内容连续且可执行新命令；
colored multiline `PS1` 是单独测试，验证 prompt 解析不会阻塞新 Control 输入。临时
`zz_probe_*` 只作诊断，不再作为 required gate。

按钮与快捷键必须是两个独立子进程场景，防止一个入口代替另一个。

### 4.6 `split_shortcut_waits_for_authoritative_focus`

- 分别投递 Alt+S 与 Alt+V；
- fixture 允许第一份 layout 暂时仍指向旧 focused pane；
- Muxterm 必须保持 Pending，直到权威 layout 收敛；
- 5 秒内 Herdr、Workspace、GTK layout leaves 和 active pane 完全一致；
- 新 pane token 只在对应 VTE 出现。

### 4.7 `ctrl_l_clears_and_stays_clear`

- 在当前 pane 写入 `BEFORE_CLEAR_<unique>`；
- 通过真实 VTE/key controller 输入 Ctrl+L；
- 再写 `AFTER_CLEAR_<unique>`；
- 当前屏不可见 BEFORE，可见 AFTER，cursor 在底部；
- 切到其他 tab 再切回，BEFORE 不得因 snapshot/reseed 复活；
- Workspace Index 可以保留历史用于搜索，但不得反灌 Surface。

### 4.8 `stale_stream_events_do_not_take_over_new_generation`

- 为同 pane 创建 generation N，再 promote/replace 为 N+1；
- 按顺序注入 N 的 Error、Closed、Frame 和 N+1 的 Frame；
- registry 只保留 N+1；VTE 只出现 N+1 token；
- stream start 计数不因三个 stale event 增加。

### 4.9 `takeover_storm_is_bounded_and_ui_remains_responsive`

- 对 current control generation 连续制造 taken-over/EOF；
- 10 秒观察窗内自动 start 尝试不超过 5；
- taken-over 后无用户动作时 control start 为 0；
- 重复 focus snapshot/reconciliation 不能清除 suppression；
- 普通连接短暂 Live 少于 10 秒不能恢复 retry budget，连续 Live 满 10 秒才恢复；
- 每 100ms 安排的 GLib watchdog 能持续执行，最大延迟不超过 500ms；
- 再次显式 focus/input 后只产生一次 control promote，并可输入 token。

CPU 百分比只作为诊断，不作为唯一断言；transition 上限和 watchdog 才是确定性门禁。

### 4.10 `saved_project_matches_existing_connection_identity`

Local 与 loopback SSH 分别：

1. 从 Existing Entry 获得 session/socket/workspace_id。
2. 保存为 Project，关闭并重新加载 store。
3. 分别点击真实 Existing row 与重新加载后的真实 Project row 连接；禁止直接调用
   `connect_target`/resolver helper 冒充入口。
4. 断言两条入口生成相同 identity key、attach spec 身份字段、WorkspaceId 和最终 Herdr
   workspace；若 store 已有同 identity Project，则合并其 name/path 后完整 ResolvedTarget
   也相同。
5. 关闭面板并从 Core Recent 再打开一次；Recent 必须保留原 path、target-side socket、session
   和 workspace_id，不能由 WorkspaceId 反推。
6. 删除 identity 字段模拟旧 TOML，再验证唯一候选迁移；制造两个同名候选验证 ambiguity。
7. 对无候选分别执行 `AttachOnly` 与 `CreateIfMissing`：前者零创建；后者只允许 local 所选
   named/default session 创建；SSH 两者都零启动/零创建。

---

## 5. 精确完成标准

| 指标 | 门槛 |
|---|---|
| 创建 tab/pane | enqueue 起 5 秒内服务端/Core/current-generation full baseline/GTK 收敛；超时必须有唯一 Failed settlement，不能卡住或再追加一段 stream timeout |
| Tab 名 | 所有 Runtime 的 UI 顺序标签非空；Herdr 默认 raw name 数字与 public id 后缀一致，Herdr/Core/GTK 一致，不要求删除后无缺口；tmux raw name 保留自身语义 |
| 切换压力 | 4 tab、目标 tab 3 pane、20 轮切换全部通过 |
| 大历史 | 100KB、393KB、500KB 三档动态数据均通过 |
| VTE 内容 | 目标 token 恰好一次；不只 `contains` |
| 几何 | grid 与 pane cols/rows 一致；cursor/prompt 在最后两行 |
| Surface 稳定 | 初次 seed 后切换 seeds/resets 增量为 0 |
| 后台连续性 | hidden tab/background workspace 隐藏期间持续 raw feed；首次切入内容连续且零 reseed |
| Ctrl-L | 旧屏不再可见，切换后不复活 |
| Stream stale | stale event 导致的 start/remove 数为 0 |
| 重试 | 每次故障链自动 retry 最多 5 次（不含原始/用户 start）；taken-over 无用户动作不反抢 control |
| GTK 响应 | watchdog 最大延迟 500ms |
| Project parity | Existing 与 Project 的 identity key/attach spec identity/WorkspaceId 相等；同 identity Project 元数据可合并为相同完整 ResolvedTarget；Core Recent round-trip 不丢 path/socket/workspace_id |
| Matrix | tmux/herdr × local/loopback SSH 四格全部 required、无 ignore/skip |

---

## 6. Fixture 安全

### 6.1 tmux

- socket：`muxterm-test-<pid>-<case>`；每条命令都带同一个 `-L`。
- 只允许对该 socket 执行 cleanup kill-server。
- 默认 server 不做任何写操作；测试不靠用户已有 session。

### 6.2 Herdr

- session：`muxterm-test-<pid>-<case>`。
- local/SSH 都指向该 named session 的 socket。
- cleanup 只能 stop/delete 该 named session；禁止无名字 `herdr server stop`。
- 测试前后记录 session list，确认用户 default session 未变化。

### 6.3 SSH

- 使用 LoopbackSshd、随机端口、动态 host/client key、显式 ssh_config。
- 不读取用户真实 key，不访问公网，不使用用户 22 端口。
- 远端 tmux 仍使用隔离 `-L`；远端 Herdr 仍使用 named session。
- 所有临时目录使用系统 temp/`tempfile` 并在 Drop 清理；不建立新 cache/target。
- 每个内容 token 必须短于测试 pane 一行，且输入命令不含连续完整 token；这既避免 soft-wrap
  假失败，也保证“目标 VTE 恰好一次、非目标为零”真的在测路由。

---

## 7. 本地与 CI 环境契约

### 7.1 固定部分

实施后以下版本必须由仓库声明并在测试开头校验：

| 工具 | 版本 |
|---|---|
| Rust | 1.97.1 |
| tmux | 3.7c |
| Herdr | 0.8.0 / protocol 19 |

`stable`、Homebrew 当前最新版或 runner 偶然预装版本不能作为 required gate 的隐式输入。

版本与 release metadata 最终核验于 `2026-08-22T19:31:35+08:00`（CST）：

- Rust 1.97.1：[`channel-rust-1.97.1.toml.sha256`](https://static.rust-lang.org/dist/channel-rust-1.97.1.toml.sha256)；
- tmux 3.7c：[`tmux/tmux` release 3.7c](https://github.com/tmux/tmux/releases/tag/3.7c)；
- Herdr 0.8.0：[`herdrdev/herdr` release v0.8.0](https://github.com/herdrdev/herdr/releases/tag/v0.8.0)；
- Herdr socket 语义：官方 [Socket API](https://herdr.dev/docs/socket-api/)；本分支和本机
  `herdr 0.8.0` 的 wire contract 固定为 protocol 19。

`~/Developer/terminal/herdr` 当前 HEAD 已是 protocol 20，不能作为 protocol-19 wire
字节布局的直接 fixture；它只用于概念对照。测试必须同时断言 Hello/Welcome 的 exact
protocol-19 bytes 和运行时 handshake mismatch 明确失败。版本边界见
[`HERDR-RUNTIME-STABILITY.md`](HERDR-RUNTIME-STABILITY.md) §2.3。

CI 下载必须校验以下 SHA-256，不能只校验文件名或 `--version`：

| artifact | SHA-256 |
|---|---|
| `channel-rust-1.97.1.toml` | `03569b1886ceb5c05276b50c8431ab111de944cd6140fe1fa7d821dd8e0f29cf` |
| `tmux-3.7c.tar.gz` | `7c60cae9a0e25288e2e24750aafc9e8800fc7fd4555e447e1b29ee4201cfb3bf` |
| `herdr-linux-x86_64` | `b872ea7e40fa2cb17e857ac9b62b1bf26db7b403c622f5d2f3f5b35f6e9acd28` |
| `herdr-linux-aarch64` | `f647ac66468d9efbc642fe534fb284468f0aea60641606fc008dfc0d82a3ca87` |
| `herdr-macos-x86_64` | `77cb5afd6c8fcaaaf3bc28e474ec01c209331ad08094e20d7f8aa9b0bb78d649` |
| `herdr-macos-aarch64` | `d53a9f93fccfdfcc55632927bf51002f5add0aa7990bcdf508ffbd84ac658178` |

### 7.2 有意保留的兼容差异

Arch 本机与 Ubuntu/macOS runner 的 GTK/VTE/系统库不要求字节级相同。它们是支持范围，
不应被一个自建全同容器抹掉。要求是：

- 打印 `uname`、rustc/cargo、tmux、herdr、GTK、VTE、ssh、locale、TERM、DISPLAY；
- 测试使用语义事件/权威 snapshot 收敛，不依赖机器速度 sleep；
- 本地与 runner 都由同一环境 helper 固定并预检
  `LC_ALL=en_US.UTF-8`、`LANG=en_US.UTF-8`、`TERM=xterm-256color`、
  `COLORTERM=truecolor`；locale
  不存在时 setup 直接失败，不能带 warning 继续；
- 差异导致失败时 artifact 足以重放，而不是远程只有一个 panic 行。

### 7.3 单一入口

- 本地先调用 `scripts/test.sh doctor`，只读校验并打印版本/locale/display/sshd 能力；它
  不安装、不覆盖系统 tmux/Herdr。
- Core workflow 调用 `scripts/test.sh run core`。
- Linux workflow 调用 `scripts/test.sh run linux`，脚本内部唯一启动 Xvfb；workflow
  外层不得再套 `xvfb-run`。
- macOS workflow 调用 `scripts/test.sh run macos`，不能维护第二份 cargo/Swift 命令表。
- 四格 matrix 的 child isolation 由 Rust test harness 完成，local 与 CI 使用同一逻辑。
- macOS 的 claimed SSH 用例必须建立 loopback sshd；环境缺失是 gate failure，不是 skip。

Agent 本地执行命令统一加 `rtk`，并复用 worktree 本地 `./target`：

```bash
rtk bash scripts/test.sh doctor
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --all-features -- -D warnings
rtk bash scripts/test.sh run core
rtk bash scripts/test.sh run linux
```

Linux 本地绿色可预测 Core/Linux required；它不能替代 macOS runner，所以最终完成仍要求
PR 的 macOS required 绿色。

---

## 8. CI 失败基线与归因

最终状态复核时间：`2026-08-22T19:31:35+08:00`（CST）。PR
[#20](https://github.com/wlingze/muxterm/pull/20) 当前 HEAD `4e81f77`：

当前环境并不一致：本机是 Rust/Cargo 1.97.1、tmux 3.7c、Herdr 0.8.0；三个 required
workflow 都通过 `dtolnay/rust-toolchain@stable` 实际升级到 Rust 1.98.0，并由
`scripts/ci/setup.sh` 安装 tmux 3.7b、Herdr 0.8.0。Linux workflow 外层又执行
`xvfb-run -a bash scripts/test.sh run linux`，而脚本内部已有 `xvfb-run`。这些差异不是下面
行为失败的替代解释，但会让“本地绿≈远端绿”失去可比性，必须按 §7.1–§7.3 一并收敛。

- [Core run 32559662382](https://github.com/wlingze/muxterm/actions/runs/32559662382)：
  Herdr SSH right split 的权威 `layout.active` 未收敛到 created pane。
- [Linux run 32559662385](https://github.com/wlingze/muxterm/actions/runs/32559662385)：
  detach/reattach 后丢 pane token；同一 GTK test process 后续 SIGSEGV。
- [macOS run 32559662392](https://github.com/wlingze/muxterm/actions/runs/32559662392)：
  Rust tests 通过；Swift 的
  `PaletteSessionListE2ETests.testLocalSessionListRefreshesBeyondNewSession` 只得到
  `titles=["New"]`，另有 Palette SSH 与 SshAttach 两个 loopback SSH case 被 skip。
- four-mode local/SSH shell/tmux 四个 CLI job 为绿色；它们不能替代 Herdr/GTK required。

对应修复关系：

| CI 失败 | 设计修复 |
|---|---|
| split active 不一致 | PendingMutation + 权威 layout/focus 收敛 |
| reattach 丢 token | generation registry + baseline/catch-up |
| GTK SIGSEGV | 先用每场景独立子进程消除跨场景 AppWindow/GLib 全局态污染；若单一 child 仍 SIGSEGV，仍是必须修的 lifecycle bug，不能用进程退出掩盖 |
| macOS readiness/skip | 统一脚本、隔离 socket readiness helper、required loopback SSH |

---

## 9. 失败 artifact

每个 required child 失败时上传：

- Muxterm tracing log；
- pane stream transition log（generation/mode/event ordinal/wire seq/retry）；
- 测试创建的 Herdr session snapshot 和 pane layout/read 摘要；
- 测试创建的 tmux list-windows/list-panes/capture 摘要；
- Xvfb/GTK backtrace 或 macOS Swift test output；
- loopback sshd log 与显式 ssh_config（私钥除外）；
- 环境版本清单。

artifact 只能包含测试 fixture，不得采集用户默认 tmux/Herdr session 内容。

---

## 10. 完成定义

- [ ] L0 的 stream、mutation、project RED 测试先失败，再由对应实现变绿。
- [ ] L1 真实 named Herdr/isolated tmux contract 通过。
- [ ] L2 每场景独立 AppWindow 子进程通过。
- [ ] L3 tmux/herdr × local/loopback SSH 四格 required 全通过。
- [ ] 大历史、20 轮切换、Ctrl-L、数字名、split focus、takeover bounded 全达标。
- [ ] Project 与 Existing Connection identity parity 通过。
- [ ] 无新增 `#[ignore]`、无环境 skip、无加长 sleep、无弱化 token/几何断言。
- [ ] fmt/check/clippy/Core/Linux 本地门禁通过。
- [ ] 获准 push 后 PR #20 Core/Linux/macOS required 全绿。
- [ ] 测试前后用户默认 tmux/Herdr session 未改变。

---

## 11. Attach 后继续使用的生命周期覆盖（2026-08-24）

### 11.1 Canonical runtime contract

registry 枚举的 tmux/Herdr × local/loopback SSH 四格都必须：

1. 在隔离 server 上预建 2 tabs/3 panes；
2. Attach 后立即水平 split、垂直 split、NewTab；
3. 等待每个 operation 唯一 `MutationSettled::Completed`；
4. 切换全部 tab/pane，并通过 `WriteRaw` 执行不同 echo token；
5. detach/drop Runtime 后重新 attach，再执行一次 mutation 和 echo。

断言同时覆盖 server、Core topology/focus/layout、Surface geometry、VTE 文本和 token 唯一
归属。任何一层只通过不算完成。

### 11.2 GTK production scenarios

每个四格独立 child process 运行：

- `attach_then_mutate_existing`：真实 QuickConnect → Existing Connections → 生产 GLib
  probe → 精确 row click → attach populated workspace → Alt+S/Alt+V/真实 `+` → VTE
  commit echo；断言 3 tabs、最终 geometry 和单一目标 VTE token。
- `detach_reattach`：生产 attach、detach/drop Runtime、再次打开同一 fixture，重新切换
  tab/pane 并执行 echo；旧 Surface generation 不得影响新 Surface。

测试只使用 `tmux -L muxterm-test-*` 与 named Herdr session；不得用默认 server。

### 11.3 Incident regression

仅在 Herdr-local 增加一条大 payload 回归：active 单 pane + 另一 tab 的 split，attach 前固定
约 223KB baseline；attach 后 split active pane、创建 tab、执行 echo。在
`G_DEBUG=fatal-criticals` 下必须正常退出，不得 timeout、SIGABRT、SIGSEGV，并上传 stderr、
stream transition、topology 和 Surface 诊断。

### 11.4 停止规则

每个状态转换一个 L0 contract；每个 registry cell 一个 canonical workflow；真实 GTK 只保留
attach/reattach 两条生产入口；只有新增 ordering、generation 或 payload invariant 才增加
regression，不做完整操作笛卡尔积。
