# AGENTS.md — Muxterm 开发约定

> 这个文件给 Codex / Claude Code 等 coding agent 读。
> 产品文档见 `PRODUCT.md`。

## 角色

你是 Muxterm 项目的 Rust 实现 agent。Muxterm 是一个 Linux 桌面终端工具，通过 tmux 控制模式 (`-CC`) 把 tmux pane 渲染成 GTK4 原生 tab pane。

## 环境

- 工作目录：`/home/wlz/Project/muxterm/`（git main 分支）
- Rust 工具链：`rustc 1.97.1` / `cargo 1.97.1`（系统已装）
- tmux 3.7b（系统已装，可直接 `tmux -CC` 测试）
- GTK4 4.22.4（系统已装）
- bare 仓库：`/home/wlz/Project/muxterm.git`

## 工作原则

先读 `PRODUCT.md`（产品规划）、`ARCHITECTURE.md`（架构与交互规范），再动代码。
2. **增量提交**：每个可独立验证的逻辑单元一个 commit。commit 信息 `feat: / fix: / test: / refactor: / docs:`。
3. **TDD 优先**：协议解析这种纯逻辑模块，先写单元测试（`#[cfg(test)] mod tests`），再写实现。实现要能 `cargo test` 通过。
4. **不要跳步**：每次只做当前任务清单里的内容，做完汇报，等下一轮指令。
5. **验证**：改完必须 `cargo build`（或 `cargo check`）通过。新增依赖要在 `Cargo.toml` 里加，不要假设依赖存在。
6. **不碰无关代码**：不要重构没要求改的模块。
7. **中文注释优先**，代码标识符用英文。

## 技术约定

- Rust 2021 edition
- 错误处理：`anyhow::Result` (应用层) + `thiserror::Error` (库层错误类型)
- 日志：`tracing`（不要 println! 调试）
- 异步：`tokio` runtime
- 终端渲染：`vte` crate 解析 ANSI，GTK4 原生绘制
- 协议解析是**纯函数**，输入 `&str`/`&[u8]` 输出 `Message` enum，方便单元测试

## tmux 控制协议实现要点（关键）

- 消息行格式：`%<keyword> <args...>`，行尾可能有 `\r\n`
- `%output @1 "content\r\n"` 的 content 是 C 语言风格转义字符串（`\e` = ESC, `\n`, `\r`, `\\`, `\"`, `\t`, `\0xx` 八进制等）
- pane id `@N`、window id `@N`（靠消息上下文区分）、session id `$N`
- `%begin` / `%end` / `%error` 之间是命令响应行（不是通知）
- 命令响应里的多行输出每行都不带 `%` 前缀
- **实现 parser 时要能从 `tmux -CC` 的真实输出流逐行 parse**，可以本地跑 `tmux -CC new-session` 抓样例输出测试

## 提交前自检

- [ ] `cargo fmt` 格式化
- [ ] `cargo check` 通过
- [ ] `cargo test` 通过（如果有测试）
- [ ] commit 信息符合规范
