# Dogfood 0907 问题与做法（重做手册）

> 用途：重构落地后，**不要 rebase 这条分支**。以本文为需求，在新树上按条重做。
> 来源：Grok 会话 `01a07ac0-4e2c-79f1-9848-946a024a2fa6`（续 Codex `01a07aaf-e33e-7ae3-85ef-0e4d31101758`）。
> 参考实现：`feature/dogfood-0907` @ `96c5076`（相对当时 `main` `fbd66f2`，11 个提交）。
> 整理：2026-09-09（Asia/Shanghai）。

本文记的是**使用中碰到的问题和你想要的行为**，不是 git 考古。旧路径只作对照；新树上文件搬家后，按行为验收。

---

## 0. 怎么用这份文档

1. 新分支从重构后的 `main` 拉出。
2. 按第 12 节顺序一条条做；每条可独立验证再提交。
3. 对照 `feature/dogfood-0907` 只看「意图和测试」，不要整文件拷贝。
4. 本机配置清理（`~/.config/muxterm` 里不用的文件）不要写进仓库。
5. GUI 问能力继续用 `support()`，禁止 `if runtime == "herdr"`。
6. 测试：macOS 用 `cargo test --no-default-features --features ffi --lib`；tmux 只用隔离 `-L`。

快捷键平台差：Linux `Ctrl+Alt+N`，macOS `Cmd+Ctrl+N`。`Cmd+0` 仍是重置字体，workspace 最后一个是 `Cmd+Ctrl+0`。

---

## 1. 你当时要的全貌

来自 Codex 原话（会话未做完，Grok 接着做）：

1. Workspace 太多才提醒：上限 **20**（soft reminder，不自动踢）。
2. 切换 Workspace：`1–9` 按侧栏**打开顺序**，`0` **永远是最后一个**（不是最近使用）。
3. 侧栏可以拖动 Workspace **改编号顺序**。
4. 复制必须走系统剪贴板：**覆盖**，不能追加。复制 `1` 再复制 `2`，粘贴应是 `2`，不是 `12`。
5. 设置里改 Project 总报错；配置本身也有错误。要能 Apply。
6. 没有配置文件就创建默认；Herdr 默认创建会报错，不要踩 SSH 禁止创建的契约。
7. 新 Tab 用 **当前 Workspace 的 project path**；同一 Tab 里 split pane **继承当前 pane cwd**。

```text
workspace path = /tmp/a
  新 tab        → /tmp/a
  在 /tmp/a/b 里 split pane → /tmp/a/b
  再新 tab      → 仍是 /tmp/a
现在（当时）永远用当前 pane cwd，不对。
```

Grok 开场额外要求：点侧栏 WORKSPACES / AGENTS / COMMANDS **闪烁，而且点了跳不过去**。

后面 dogfood 又补了一批：重复激活、新 Tab 仍走 pane cwd、Agents 被压扁且不能调高度、状态永远 Working、Cmd-R Attention 写成 `local ~`、渐进选词、命令刻度轨、回底胶囊、构建 `zsh: killed`、侧栏消失、NSSplitView 刷屏、诊断要进 `--log-file`、Attention 整行染色。

产品手感对照：`docs/PRODUCT-VISION-STRATEGIC-REVIEW.md` §2.15.4。

---

## 2. Workspace 上限 20、快捷键 1–9 / 0、侧栏改序

**问题**

- 开几个 Workspace 就提示太多（本机 `max_slots = 5`）。
- 只能切前 5 个；没有「永远最后一个」。
- 侧栏顺序不能拖，编号跟手感对不上。

**期望**

- 提醒上限 20，不自动淘汰。
- `1–9` = 侧栏固定打开顺序；`0` = 最后一个，哪怕总共不到 10 个。
- 拖 Workspace 行之后，编号跟着变。

**做法（当时）**

- 动作：`switch_workspace_1..9`、`switch_workspace_last`（index `0` 表示 last）。
- Core `WorkspacePool::reorder(&[WorkspaceId])`：按传入顺序重写 `opened_order`，没出现的格子保持相对顺序接在后面。
- Linux：keymap + `switch_workspace_n`；`n==0` 取 `list().last()`；侧栏编号前 9 个。
- macOS：`WorkspaceShortcutIndex.maximum = 9`；`Cmd+Ctrl+0..9`；拖拽 pasteboard `muxterm.sidebar.workspace`。
- 本机 `~/.config/muxterm/config.toml` 的 `max_slots` 改成 20。代码默认 / `config.example.toml` 已是 20。

**重做注意**

- macOS 当时改的是 GUI `WarmConnectionSlot.openedOrder`，**没有走 Core FFI**。Linux 走 `pool.reorder`。新树如果只有一个池，两边都应打到 Core。
- Linux 当时只接了 reorder 回调，GTK 拖拽 UI 不一定有。
- `Cmd+0` 不得变成切 Workspace。

**验收**

- `Action::switch_workspace_index`：9 → `Some(9)`，last → `Some(0)`。
- `WorkspacePool::reorder` 单测：`[a,b,c]` → `[c,a,b]`，编号 1 变成 c。
- macOS：`KeyBindingsTests.testCmdCtrlDigitsSwitchWorkspacesIncludingLast`。
- 手测：拖第 3 个到最前，`Ctrl+Alt+1` / `Cmd+Ctrl+1` 切到它；`0` 永远切最后一个。

---

## 3. 复制追加、写不进系统剪贴板

**问题**

复制 `1`，粘贴 `1`；再复制 `2`，粘贴变成 `12`。感觉没走系统剪贴板。

**期望**

和系统 Edit 菜单一样：每次复制**替换**当前剪贴板。

**原因**

`NSPasteboard.writeObjects` 在不清空时再挂一个 item，粘贴方拼在一起。

**做法**

`clearContents()` + `setString`；失败再 `setData`。不要先清再 `writeObjects`。

**验收**

复制 `1` 再复制 `2`，系统粘贴板和 pane 内粘贴都是 `2`。

---

## 4. 设置 Apply 报错、缺省配置、Herdr 创建

**问题**

- 改 Project 点 Apply 总失败。
- 没有配置文件时应该自动创建。
- 默认创建 Herdr（尤其 SSH）会报错。

**期望**

同一 runtime/transport/session 的 Project 只留一条；缺文件就写出默认 TOML；不要在 SSH Herdr 上偷偷 `CreateIfMissing`。

**原因**

旧 compact uniqueID（`4:tmux|3:ssh|…`）和 `name@transport` 并存，Apply 撞 `uniqueID`。

**做法**

- `ConfigDocument::normalize_projects()`：按 `logical_key`（runtime + transport + target + session + socket）去重，保留第一条。
- macOS `QuickConnectStore.projectJSON` 按 `uniqueID` 去重。
- `SettingsService::open`：文件不存在则 `create_dir_all` + 原子写入默认文档。
- Herdr SSH 无匹配禁止创建，这是已有契约，不要为「缺省配置」去开。

**本机（不要进仓库）**

当时删过 `~/.config/muxterm` 里不用的 `quickconnect.toml` / `quickconnect.json` / `ui-render.log`。

**验收**

- `from_toml_dedupes_equivalent_projects`
- `open_creates_missing_config_file`
- `testProjectJSONDropsDuplicateUniqueIDs`
- 设置页 Apply 同一 SSH project 不再报 duplicate。

---

## 5. 新 Tab 用 Workspace 路径，split 继承 pane cwd

**问题**

新 Tab 一直跟当前 pane 的 `#{pane_current_path}`。你怀疑是 tmux 默认、改不了。

**期望**

改得了。显式 workdir > Workspace project path > 才退回 pane cwd。SSH 的 `~/…` **不要在 Mac 上展开**，交给远端 tmux。

**原因（第一轮没做完）**

Workspace 层填了 `Task::NewTab.workdir`，但 tmux attach 后 Runtime 自己没有记住 Catalog path，execute 仍走 pane cwd。SSH 若本地 `expand_config_value`，会把 `/Users/…` 送到 ryzen。

**做法**

- `Workspace::apply_workspace_defaults`：`NewTab` 且 workdir 为空 → `resolved_target.canonical.path`，再退 `WorkspaceId.path`。`SplitPane` 不动。
- `TmuxRuntime.workspace_workdir`：Catalog `TmuxDriver::open` 和 attach 都 `set_workspace_workdir`。
- `new_tab_directory`：显式 > workspace path；本地展开 `~`，SSH 原样保留。
- Herdr：`tab.create` / `pane.split` 带 `cwd`（有 workdir 时）。

**验收**

- `new_tab_without_workdir_uses_workspace_project_path`
- `new_tab_without_explicit_dir_uses_workspace_project_path`（命令含 `-c`，不含 `#{pane_current_path}`）
- `ssh_new_tab_keeps_remote_tilde_path`（`-c "~/Developer/self/muxterm"`）
- 手测：project `/tmp/a`，cwd 进 `/tmp/a/b` 后 New Tab 仍在 `/tmp/a`，split 在 `/tmp/a/b`。

---

## 6. 侧栏闪烁、点了跳不过去、点一下整页重激活

**问题**

- 点 WORKSPACES / AGENTS / COMMANDS 行闪一下，跳不过去。
- 已经在当前 Workspace 再点 agent/command，日志里 `workspace activation begin/ready` 大约每秒重复一遍（`elapsed_ms≈24`），布局被重置。
- 60Hz 刷新把整张 agent/command 表 `reloadData`，选中行丢掉。

**期望**

点哪跳哪；当前 Workspace 内跳 pane **不要**走完整 activate。刷新时行身份不变就地改文字/颜色。

**做法**

- Agent/command 身份不变则 `configure*Cell`，变了才 `reloadData`。
- `pendingSidebarTarget`：点击立刻钉住目标，刷新时不要被 live 焦点抢回去；workspace+pane 对上再清。
- 跳转走 `routePanelJump`（和 Attention 同一条），不要 `activate` + `performWhenForegroundReady`。
- `activate(slot:)`：`activeKey == slot.key && bridge === slot.bridge` 直接 return。

**验收**

- `WorkspaceSidebarE2ETests`：agent/command 刷新不增加 reload count。
- 手测：当前 Workspace 点 agent，日志不再刷 activation；目标 pane 成为前台。

---

## 7. Agents 被压扁、分区不能上下拖

**问题**

点一下之后 Agents 突然变矮；WORKSPACES / AGENTS / COMMANDS 高度不能拖。

**期望**

四个分区可独立拖高矮。折叠只留 26pt 标题；展开最小约 80pt。

**原因**

`NSStackView` `fillEqually` 均分高度，一点击/折叠就把 Agents 压没。

**做法**

改成竖直 `NSSplitView`（`isVertical = false`），`autosaveName` 用新名字（当时最终是 `.v2`，旧名会把 0 高度写回去）。折叠区 26pt，展开可拖。

后面侧栏消失、启动警告，都是这条改动的续发，见第 10 节。

**验收**

折叠后标题还在；拖 Agents/Commands 分隔条能改高度；重启后比例还在（autosave 非 0）。

---

## 8. 侧栏永远 Working；Cmd-R Attention 显示 `local ~`

**问题**

- Agents 状态卡在 Working（structured 生命周期不更新）。
- 侧栏已经是「名字 · agent · Tab N」，Cmd-R Attention 却是 `local ~` 这种。

**期望**

状态跟注意力生命周期走。Attention 的内容和侧栏同一套：工作区名、状态、agent、Tab N。不要用本机 `~` 当标题。

**做法**

- `WorkspaceSidebarProjection`：同一 pane 有 attention 快照时，**覆盖** structured `Working`。
- FFI `muxterm_attention_snapshot` 带上 `name`、`transport`、`path`（`resolved_target` 再退 `WorkspaceId.path`）。
- Attention 行从侧栏 chrome 取 workspace / agent / tab。

**验收**

- `testAgentStatusFollowsAttentionWhenStructuredStaysWorking`：structured Working + attention Done → `Done · Codex`。
- Attention 行出现 `muxterm` / `Working` / `Codex` / `Tab 2`，不含 `local`、`~`。

**后续外观**见第 11 节（色块 + 两行）。当时第一轮是单行 `workspace · status · agent · Tab N`。

---

## 9. 终端手感：渐进选词、命令刻度、回底

**问题 / 期望（你的原话）**

> 双击选中一个单词，再次双击选更大的词。`cd a/b/c` 双击 `b` 再双击得到 `a/b/c`。滚动栏按命令状态红/绿。下拉跳到最后。位置和交互要舒服。文档里有想要的效果。

文档：`docs/PRODUCT-VISION-STRATEGIC-REVIEW.md` §2.15.4。

| 能力 | 产品要求 |
| --- | --- |
| 回底 | 鼠标滚上去后，右下角胶囊 `↓ 最新 · +N`；上滚即 scroll-lock，新输出不把人拽回去 |
| 命令刻度 | 滚动条一侧极窄轨（约 10px，悬停展开）；成功绿 / 失败红；点击跳命令；悬停出命令文本；宽度超过一个字符位算违规；跟随时可隐藏 |
| 选词 | 你额外要求**渐进扩大**，不要 iTerm 默认一次吞掉整段路径 |

旧 UI：右上角 ✓ / ✗ 按钮，不是轨。

**选词（第一轮 → 你当场改口径）**

第一轮两层：标识符 → 路径字符 `/\-_.~:@+%#`。

你后来说要以 `feature/dogfood-0907*` 为准，并且拖选用空格切（iTerm / Ghostty）：

```text
点 0907
  第 1 次  0907
  第 2 次  dogfood-0907*
  第 3 次  feature/dogfood-0907*
拖选：按空白分成整词，不要在 / 或 - 处切开
```

中间层扩不开就跳过（`a/b/c` 的 `b` 第二次直接到整段路径）。括号/引号仍交给 SwiftTerm 平衡匹配。TUI 鼠标模式不要动。

三层字符：

- identifier：字母数字下划线（含 CJK）
- word：`-.+%@#?*`
- path：`/\~:`

双击后若继续拖：`dragByTokens`，两端吸附到空白分隔的整词。

SwiftTerm 需要 `setSelectionRange` patch（`scripts/patch-swiftterm.sh`，标记 `MUXTERM_SET_SELECTION`）。

**命令轨 / 回底（当时实现要点）**

- `CommandMarkTick` 来自 OSC 133 / `pane_command_marks`；`exitCode == 0` 绿，非 0 红，没有码 pending。
- 折叠宽 8pt，悬停 14pt；offset 0（最新）靠下。
- 命中：距离近优先，同距离优先失败刻度。
- 点轨底部附近（约 8%）= 回底。
- 回底胶囊文案 `↓ 最新` / `↓ 最新 · +N`。
- macOS 滚动位置 ≥ 0.92 且未到 1.0 时向下吸附。
- Linux：`snap_history_to_latest`（距底 ≤ `page/6` 且正在向下滚才吸）；按钮同样改文案。Linux **没有**重做命令轨。

**验收**

- `ProgressiveWordSelectionTests`：`a/b/c`、`dogfood-0907*` 三层、拖过空格。
- `CommandMarkRailTests`、`JumpLatestCaption`。
- Linux `snap_history_to_latest_only_near_bottom_when_scrolling_down`。
- 手测：双击/三击分支名；拖选不被 `/` 切开；失败命令红点可点；滚上去出现胶囊，点了回尾且恢复跟随。

---

## 10. 构建 SIGKILL、侧栏消失、日志刷终端

### 10.1 `./build/macos/muxterm --help` → `zsh: killed`

**原因**

Apple Silicon 按 inode 缓存代码签名。`cp -f` 覆盖已有 Mach-O，内核用旧签名校验新内容 → SIGKILL。另外 `build-macos.sh` 先编 tui CLI 再编 ffi，ffi 把 `target/*/muxterm` 盖成没有 tui/gui 的二进制。

**做法**

- `install_executable`：拷到 `dst.new.$$` → `chmod +x` → ad-hoc `codesign --sign - --identifier dev.muxterm.cli` → `mv`。
- `smoke_cli_help`：装完必须能跑 `--help`（137 当失败）。
- `build-macos.sh`：**先 ffi 给 Swift，最后再编 tui CLI 并 install**。
- `build-cli.sh` / `build-tui.sh` 走同一套。

**验收**

`./scripts/build-macos.sh` 然后 `./build/macos/muxterm --help` 退出 0。脚本路径是 `scripts/` 不是 `script/`。

### 10.2 侧栏整栏没了

**现象**

日志：`arranged view frame` 全是高度 0，还有 `{0,-25}`，顺序和 arranged views 不一致。WORKSPACES / AGENTS / COMMANDS 看起来消失。

**原因**

分区 SplitView 在 0 尺寸时仍 `setPosition`，autosave 把四个 0 高 pane 写回去。`translatesAutoresizingMaskIntoConstraints = false` 和手排 frame 打架。

**做法**

抽出 `SidebarSectionSplitLayout`：

- 总高度不够（展开最小 80 × n + 折叠 26）→ **不要排**（`heights() == nil`）。
- 折叠固定 26pt；剩余高度分给展开区。
- autosave 换新名，丢掉 0 高度。
- TAMIC 打开，让 SplitView 自己管 subview。

### 10.3 启动仍警告 `{240, 0}` / 子视图 `{0, 0}`

**现象**

侧栏内容已经能看见，但启动还打：delegate `resizeSubviewsWithOldSize:` 后 outer edges 对不齐。Split bounds `{240, 0}`，四个 arranged frame `{0,0,0,0}`。

**原因**

高度为 0 时直接 return，宽度还停在 0，对不上 240。

**做法**

退化布局：`degenerateFrames(count, width)` → `{x:0,y:0,w:width,h:0}`。有真实高度后再走正常 heights。

**验收**

`SidebarSectionSplitLayoutTests`；`testZeroHeightSidebarKeepsArrangedSubviewWidth`。启动不再把侧栏排成全 0。

### 10.4 `--log-file` 时终端仍被刷

**期望**

```text
./build/macos/muxterm gui --debug --log-file test-….log
```

NSSplitView、IMK、`workspace activation begin/ready` 进那个文件，不要印在 TTY。

**做法**

- 有 `--log-file` 时 `freopen` stderr 到该文件（XCTest 进程不要做）。
- FFI `muxterm_log_message` → tracing `muxterm::ui`；header / CoreBridge / Swift `NSLog` 换这条。
- 无 `--log-file` 的 `--debug` 仍可走 stderr。

**验收**

`GuiLogFilePolicyTests`；`log_message_does_not_panic_without_subscriber`。手测：TTY 干净，日志文件里有 UI 行。

`error messaging the mach port for IMKCFRunLoopWakeUpReliable` 当时当作输入法噪音，没当 bug 修。

---

## 11. Attention 整行一种颜色

**问题**

Attention 整行绿或橙，和侧栏不像。

**期望**

和 Agents / Commands / Workspace 行一样：前面一块颜色，**文字用正常前景色**。每项两行，内容对齐侧栏。

```text
[色块]  muxterm
        Working · Codex · Tab 2
```

色：运行绿、未读完成/阻塞橙、已读灰。8pt 圆角色块，不要把 title/detail 染成 systemGreen/Orange。

**做法**

`AttentionRow.title` = 工作区名；`detail` = `status · agent · Tab N`；`indicator` 映射侧栏同一套。`AttentionRowCellView`：色块 + 两行 label。

**验收**

`NativePanelLayoutE2ETests`；`testAttentionRowUsesNormalTextColor`。手测：Cmd-R 打开，行结构与侧栏 Agents 同类，只有色块上色。

---

## 12. 建议重做顺序

按依赖，不要按旧 commit hash：

| 序 | 主题 | 主要落点 |
| --- | --- | --- |
| 1 | 构建安装 / SIGKILL | `scripts/build-*.sh` |
| 2 | 缺省配置、Project 去重、max_slots=20 | config / settings |
| 3 | 快捷键 1–9 / 0 + Core reorder | config + pool + 两端 GUI |
| 4 | NewTab cwd（含 SSH `~`） | Workspace + tmux + herdr |
| 5 | 系统剪贴板覆盖 | macOS TerminalView |
| 6 | 侧栏点选跳转、禁重复 activate、就地刷新 | macOS sidebar / MainWindow |
| 7 | 分区 SplitView + 0 高度退化布局 | sidebar layout |
| 8 | Attention 数据对齐侧栏 | FFI snapshot + projection |
| 9 | Attention 色块两行 | UnifiedPanel |
| 10 | `--log-file` 吃掉 UI stderr | logging FFI + AppDelegate |
| 11 | 渐进选词 + 命令轨 + 回底 | Chrome + TerminalView；Linux 至少回底吸附 |

---

## 13. 观察记录：远程校准「抢锁」（当时没改代码）

你问：远程校准在抢锁，是不是因为多个 client 连着同一个 tmux？并要求记到文件。当时只在对话里解释过，仓库里没有单独文档，记在这里。

**结论：不是 tmux 全局锁，也不是「好几个 Muxterm/iTerm 抢同一个 session」的主因。**

证据日志：`test-2026-0908-1051.log`（约 10:51 CST）。muxterm 激活 `elapsed_ms=3430`、`5263`。同时有 `pane output gap fenced`、`output-dropped` pause/capture、`initial pane seed … deadline`、交错 `select-window -t @61` / `@0`、`忽略其它 session 的 %session-window-changed`。

实际机制：

1. 每个 Workspace 自己的 `WarmConnectionSlot.bridgeLock`（NSLock）串行化这一路 FFI。校准 / `prepareForForeground` / `captureForegroundAuthority` 都要这把锁。
2. 输出缺口会 fence，然后对**当前这条** `-CC` 发 `refresh-client -A %N:pause` + `capture-pane`。SSH 上这一串是串行的，容易到数秒。
3. 侧栏连点（muxterm → legion → muxterm → …）会 generation++，作废上一次校准，再发一轮。`pendingForeground` 叠起来，看起来像抢锁。
4. ConnectionPool 通常是 **一个 Workspace 一条 `tmux -CC`**，连的是不同 session（muxterm / legion / yaklang）。日志里「忽略其它 session 的 window-changed」是故意丢的，不是死锁。
5. 真有另一个 client（iTerm 等）attach **同一个** session，会抢 current-window 和 client size，一般不会让 `bridgeLock` 卡 5 秒。

若重构后还要修：减少「已在前台」时的全量校准（和第 6 节重复 activate 是一类）；output-gap 的 pause/capture 不要和连点 SwitchTab 叠成队。不要把它理解成「先禁多 client」。

---

## 14. 对照：当时的提交（只作索引）

| Commit | 覆盖 |
| --- | --- |
| `5525cf1` feat(workspace): expand shortcuts, reorder, and tab cwd | §2 §4 §5 第一轮（Linux/Core；macOS 快捷键在下一笔） |
| `76e2934` fix(macos): replace clipboard contents and stabilize sidebar jumps | §2 macOS、§3、§6 |
| `64dd3e0` fix(tmux): create new tabs in the workspace project directory | §5 真正记住 Catalog path |
| `55e2c1c` fix(macos): stop repeat activation and align attention with sidebar | §6 后半、§7、§8 |
| `fc66609` feat(terminal): progressive select, command rail, and jump-latest | §9 第一轮 |
| `087e96d` fix(macos): stop SIGKILL after installing the CLI binary | §10.1 |
| `7056d9d` fix(macos): restore sidebar section layout after NSSplitView collapse | §10.2 |
| `54a1794` fix(macos): silence NSSplitView warning on zero-height sidebar | §10.3 |
| `7e6bb31` fix(macos): send GUI diagnostics to --log-file instead of the terminal | §10.4 |
| `6a83061` fix(terminal): grow double-click by hyphen then path, drag by spaces | §9 按 `dogfood-0907*` 改口径 |
| `96c5076` fix(macos): give Attention rows a color block and two-line layout | §11 |

新文件（旧树，搬家后自行对）：

- `ProgressiveWordSelection.swift` + Tests
- `CommandMarkRail.swift` + `CommandMarkRailView.swift` + Tests
- `SidebarSectionSplitLayout.swift` + Tests
- `GuiLogFilePolicy.swift` + Tests

---

## 15. 明确不做 / 未做完

- 没改 TUI 鼠标选词。
- Linux 命令刻度轨没按 macOS 重做（Linux 原先就有自己的 mark 按钮）。
- 「上次看到这里」未读分割线（§2.15.4 主点子）这轮没做。
- `[` / `]` 命令跳转产品里有，这轮没作为完成项。
- IMK mach port 警告没修。
- §13 抢锁只做了诊断，没有代码改动。
- 本机删除的 `~/.config/muxterm` 废文件不要在重做时再碰别人的机器。
