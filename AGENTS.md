# AGENTS.md — Muxterm 开发约定

> 这个文件给 Codex / Claude Code / Cursor 等 coding agent 读。
> 产品：[`PRODUCT.md`](PRODUCT.md)。结构：[`docs/WORKSPACE.md`](docs/WORKSPACE.md)
> （Muxterm 组合根 → WorkspacePool → Workspace = Runtime(Transport) + path → Tab → Pane；
> Window 只是体现）。Runtime：[`docs/RUNTIME.md`](docs/RUNTIME.md)。
> 像素 / 常驻 Scene：[`docs/SURFACE.md`](docs/SURFACE.md)。
> Catalog：[`docs/CATALOG.md`](docs/CATALOG.md)。配置：[`docs/CONFIG.md`](docs/CONFIG.md)。
> 目录：[`docs/PROJECT-STRUCTURE.md`](docs/PROJECT-STRUCTURE.md)。
> 施工顺序（不入库）：仓库根目录 `refactor-muxterm-0908.md`。
>
> 契约冻结 2026-09-08。代码仍在旧树（`src/platform/`、Catalog 含 Pool、`RuntimeMode`）。
> 新代码按目标契约写，不要把旧目录当长期架构。

## 角色

你是 Muxterm 项目的 Rust 实现 agent。Muxterm 是跨平台终端：Linux GTK4、macOS Swift、
TUI、CLI 都是 frontend。Core 是库；frontend 只经 C FFI + 统一 `ffi_client`。
长期目标是跨平台统一体验，并让 Activity lane 感知 commands / agents。

## 环境

- 先 `pwd` / `git status` 确认当前 worktree 与分支，不要假设路径。
- 常见布局：bare 仓库 + worktree（例如 `~/Developer/self/muxterm/.worktree/...`）。
  旧文档里的 `~/Project/muxterm/` 可能仍是某台机器上的 main worktree。
- GitHub：`https://github.com/wlingze/muxterm`
- Rust：系统 `rustc` / `cargo`（开发机曾实测 1.97.1）
- tmux 3.x（开发机曾实测 3.7b）；GTK4 在 Linux 构建 GUI 时需要

## 工作原则

1. 先读 `docs/WORKSPACE.md`、`PRODUCT.md`，再动代码。
   动 Runtime / Herdr 还要读 `docs/RUNTIME.md`。
   像素 / 前端场景还要读 `docs/SURFACE.md`。
   **不要**实现产品 Session / 虚拟 Window。
   **不要**在 frontend 做连接池、warm/cold slot、`bridgeLock`、串行后台校准。
   **不要**让 frontend 直接 `use` Core 内部模块或 Runtime concrete type。
   GUI 问能力用 `support()`，禁止 `if runtime == "herdr"`。
   Catalog 只回答「能打开什么」；组合根是 `Muxterm`，不是 Catalog。
2. **增量提交**：每个可独立验证的逻辑单元一个 commit。commit 信息 `feat:` / `fix:` / `test:` / `refactor:` / `docs:` / `ci:` / `chore:`。
2b. **commit 一律用英文写**：subject 用 `类型(scope): 英文描述` 格式（如 `feat(tui): rewrite TUI with ratatui`），
    body 用英文逐条列出改动。类型前缀保持英文（feat/fix/test/refactor/docs/ci/perf/chore），描述与细节一律英文。
3. **TDD 优先**：协议解析这种纯逻辑模块，先写单元测试（`#[cfg(test)] mod tests`），再写实现。实现要能 `cargo test` 通过。
4. **不要跳步**：每次只做当前任务清单里的内容，做完汇报，等下一轮指令。
5. **验证**：改完必须 `cargo build`（或 `cargo check`）通过。新增依赖要在 `Cargo.toml` 里加，不要假设依赖存在。
6. **不碰无关代码**：不要重构没要求改的模块。
7. **中文注释优先**，代码标识符用英文。
8. **不要**在 commit message 里加 `Co-authored-by` 尾注。
9. **绝不杀用户 tmux 会话**：`tmux kill-server` / `kill-session` / `kill-pane` 等任何
   破坏性命令，**一律禁止**直接对默认 server 执行。任何需要 tmux 的测试/验证，
   **必须**用独立隔离 socket（`-L <唯一名>`），且清理时也**必须**带同一个 `-L`。
10. **绝不停用户默认 Herdr server**：禁止无名字的 `herdr server stop`。Herdr 测试只用
    named session（`muxterm-test-<唯一后缀>`）。

## 技术约定

- Rust 2021 edition
- 错误处理：`anyhow::Result`（应用层 / frontend）+ `thiserror::Error`（库层：`RuntimeError` / `TransportError` / `ResolveError`）
- 日志：`tracing`（不要 println! 调试）
- 异步：`tokio` runtime。需要 `Box<dyn Runtime>` 时用 `#[async_trait]`（trait 内 `async fn` 不可 dyn 分发）
- 终端渲染：frontend Surface 吃原始字节；Core Index 用 `vte` crate 做搜索 / OSC 133 / BEL
- 协议解析是**纯函数**，输入 `&str`/`&[u8]` 输出 `Message` enum，方便单元测试
- ID 用 newtype（`WorkspaceId` / `TabId` / `PaneId` / `ActivityId`），不要裸 `u64`/`String` 互换
- `src/main.rs` 禁止 `mod` 声明；Core module tree 只从 `src/lib.rs` 长出来

## tmux 控制协议实现要点（关键）

### ⚠️ tmux 会话安全（最高优先级，违反=严重事故）

- **永远不要**对默认 server 执行 `tmux kill-server`、`kill-session`、`kill-pane`。
  这会杀掉用户全部真实 tmux 会话/窗口/pane，造成不可逆数据丢失。
- 任何需要真实 tmux 的测试/复现/验证，**必须**使用**独立隔离 socket**：
  - 建：`tmux -L muxterm-test-<唯一后缀> new-session -d -s <name>`
  - 查/操作：每一步都带同一个 `-L muxterm-test-<唯一后缀>`
  - 清理：`tmux -L muxterm-test-<唯一后缀> kill-server`（**必须带 -L**，只杀自己的测试 server）
- 默认 server（不带 `-L`）只允许**只读**命令：`tmux ls` / `list-sessions` / `list-windows`
  / `list-panes` / `has-session`，且仅用于查看，绝不写、绝不杀。
- 拿不准某条命令会不会破坏会话时，先停下来，把命令写给用户确认，再执行。
- 本条规则对测试脚本、集成测试、复现 bug 的临时命令**同样适用**。

- 消息行格式：`%<keyword> <args...>`，行尾可能有 `\r\n`
- `%output @1 "content\r\n"` 的 content 是 C 语言风格转义字符串（`\e` = ESC, `\n`, `\r`, `\\`, `\"`, `\t`, `\0xx` 八进制等）
- pane id `@N`、window id `@N`（靠消息上下文区分）、session id `$N`
- `%begin` / `%end` / `%error` 之间是命令响应行（不是通知）
- 命令响应里的多行输出每行都不带 `%` 前缀
- **实现 parser 时要能从 `tmux -CC` 的真实输出流逐行 parse**，可以本地跑 `tmux -CC new-session` 抓样例输出测试
- `$N` / `@N` / `%output` **只允许出现在** `runtime/tmux`。Workspace / FFI / frontend 只看见产品事件。

## 提交前自检

- [ ] `cargo fmt` 格式化
- [ ] `cargo check` 通过
- [ ] `cargo test` 通过（如果有测试）
- [ ] `cargo clippy`（CI 会以 `-D warnings` 检查）
- [ ] commit 信息符合规范，且无 Co-authored-by
- [ ] 没有引入 `RuntimeMode` 复合变体、frontend 对 `crate::core` 的直接引用、或 live `visible_ansi`

## Time verification (MUST)

Before making any decision, first verify the current real time on the machine.

- Run: `date -Iseconds` and `date`
- Record the output in your reasoning/logs (include timezone offset)
- If time seems inconsistent with the user-provided context, call it out explicitly with an absolute timestamp

## Web verification (MUST)

For any decision that depends on facts outside the local repository or user-provided text (including versions, policies, specs, security guidance, compatibility claims, recommended tooling, or anything time-sensitive), you MUST verify via the internet using authoritative sources.

- Use web browsing/search before deciding
- Prefer primary/authoritative sources:
  - Official documentation, standards bodies, vendor release notes
  - Reputable institutions (e.g., government, academia, major foundations)
  - Primary papers or RFCs when relevant
- Always include:
  - What was verified
  - Which sources were used (links or clear citations)
  - The exact verification time (from the Time verification step)

## If web verification is blocked

If network access/tools are unavailable or the user explicitly forbids browsing:

- Do not guess
- State what cannot be verified and why
- Ask for user-provided source/material, or offer safe fallback options that do not rely on unverified facts

## Worktree 开发工作流

多特性并行开发时，用 git worktree 隔离工作区。每个仓库/ worktree 使用**本地**
编译缓存与产物目录（`./target` 与 `./build`），不跨 worktree 共享 target。

### 本地 target

- `.cargo/config.toml` 不再设置 `target-dir`，cargo 使用仓库本地默认 `./target`
- 产物统一输出到 `./build/<os>/`（见 `scripts/build-common.sh`）
- 可用环境变量 `CARGO_TARGET_DIR` 覆盖

### 新建 worktree

在**主仓库**根目录执行：

```bash
./scripts/worktree-setup.sh <feature-name>
```

效果以脚本为准（路径与分支名因机器上的仓库根而异）。编译产物进入该 worktree 本地 `./build` 与 `./target`。

### Agent 注意

- 改代码前确认当前 worktree 路径与分支（`pwd` / `git status`）
- 不要把 `./target`、`./build` 提交进仓库（已在 `.gitignore` 中）
- 清理 worktree：`git worktree remove <path>`（必要时再删本地分支）
- 产品概念里的 Worktree（Project 下的 checkout）与 git worktree 工作流不是同一层：前者走 `core/projects`，见 [`docs/WORKSPACE.md`](docs/WORKSPACE.md)
