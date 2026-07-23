# Muxterm 重构计划 — 从 archmini 迁移到 Ryzen 继续

> 创建: 2026-07-21
> 原因: archmini (i5-9500T) 编译太慢,迁移到 Ryzen (5950X, 64GB) 继续开发

## 当前进度

### 已完成
- **Step 1** ✅ 核心 trait 定义 + 纯模型类型 + 单元测试
  - `src/core/model/layout.rs` — 布局树 (Session/Window/Pane 嵌套分割)
  - `src/core/model/state.rs` — State trait + 状态快照类型
  - `src/core/model/task.rs` — TerminalTask enum
  - `src/core/model/backend.rs` — Backend trait (State supertrait + execute + subscribe_output)
  - `src/core/model/mod.rs` — 模块声明
  - commit: `542d88c feat(model): 引入 Terminal 层纯模型核心 trait 与类型`

### 进行中
- **Step 2** 🔄 TerminalModel 纯逻辑 + mock backend 测试
  - `src/core/model/terminal_model.rs` 已创建（半成品）
  - `backend.rs` 和 `mod.rs` 有未完成修改
  - commit: `a0e4268 wip: Step 2 TerminalModel in progress`
  - Codex 在 archmini 上开始但未完成，需要在 Ryzen 上继续

### 待做
- **Step 3** LocalBackend 从现有代码提取，实现 Backend trait
- **Step 4** TmuxBackend 从现有代码提取，实现 Backend trait
- **Step 5** ASCII TUI 前端 + --tui flag
- **Step 6** GTK4 前端适配新架构 + 全量回归

## 架构设计确认（6 点已确认）

1. **Push** — subscribe_output() 流式通道
2. **Backend 合一** — Box<dyn Backend> 一个 trait
3. **TerminalTask enum** — 不实现 Task trait
4. **双轨** — 新旧并存，Step 6 一次性切换
5. **--tui flag** — CLI 加 --tui 启动 ASCII TUI（可交互 + 测试用）
6. **undo/redo 延后** — 只定义 StateSnapshot 结构

## Ryzen 环境

- 代码目录: `~/Developer/self/muxterm/`
- 共享编译缓存: `~/Developer/self/muxterm-target/`
- Rust: rustc 1.97.1 / cargo 1.97.1
- tmux: 3.7b
- GTK4: 4.22.4（但 Ryzen 不需要 GTK，只跑 TUI）
- GitHub: `https://github.com/wlingze/muxterm`（已有 CI/CD）

## 在 Ryzen 上继续的步骤

### 1. 启动 Codex
```bash
cd ~/Developer/self/muxterm
codex --yolo
```

### 2. 给 Codex 的指令
```
读 .codex-refactor-task.md 了解完整架构设计。
当前进度: Step 1 已完成（commit 542d88c），Step 2 半成品（commit a0e4268）。
继续 Step 2: 完成 TerminalModel 纯逻辑 + mock backend 测试。
然后依次 Step 3-6。
每步完成 cargo test 确认通过。
```

### 3. 验证
```bash
cargo test  # 当前应有 270+ 测试通过
```

## 文件说明

| 文件 | 用途 |
|------|------|
| `.codex-refactor-task.md` | 完整架构设计文档（给 Codex 读） |
| `migration-plan.md` | 迁移计划（本文件的前身） |
| `src/core/model/*.rs` | Step 1 产出的核心模型代码 |
| `src/core/model/terminal_model.rs` | Step 2 半成品 |
| `PRODUCT.md` | 产品规划 |
| `AGENTS.md` | 开发约定 |
| `ARCHITECTURE.md` | 架构文档 |