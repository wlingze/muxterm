# Muxterm 迁移计划: archmini → Ryzen

> 创建日期: 2026-07-21
> 原因: Ryzen (5950X, 64GB) 性能更高,CI/CD 打版本后 archmini 不再需要本地开发

## 现状

| 项目 | archmini (当前) | Ryzen (目标) |
|------|----------------|-------------|
| 代码 | ~/Project/muxterm/ + ~/Project/muxterm.git | ~/Developer/self/muxterm/ |
| GitHub | wlingze/muxterm (private, CI/CD already) | 同一远程 |
| 构建 | rustc 1.97.1, gtk4 4.22.4, vte4 ✅ | rustc 1.97.1, gtk4 4.22.4, **vte4 缺失** |
| 存储 | 256G NVMe | 916G NVMe (706G free) |
| agent | Cursor Agent + Codex (GLM-5.2) | 可同样配置 |

## 步骤

### Step 1: Ryzen 安装缺失依赖
```bash
sudo pacman -S libvte-2.91-gtk4  # 或者 Arch 上对应的 vte4 包
```

### Step 2: Ryzen 创建项目目录
```bash
mkdir -p ~/Developer/self/muxterm
# bare 仓库
git init --bare ~/Developer/self/muxterm.git
git -C ~/Developer/self/muxterm.git symbolic-ref HEAD refs/heads/main
# main worktree
git clone ~/Developer/self/muxterm.git ~/Developer/self/muxterm/main
```

### Step 3: 推送代码到 Ryzen
当前 archmini 上的 main 分支已经推到 GitHub。Ryzen 从 GitHub clone:
```bash
cd ~/Developer/self/muxterm/main
git remote add origin https://github.com/wlingze/muxterm.git
git fetch origin main
git reset --hard origin/main
```

### Step 4: 配置共享编译缓存
```bash
mkdir -p ~/Developer/self/muxterm-target
# .cargo/config.toml 已在 repo 中: target-dir = "../muxterm-target"
# 但需要调整相对路径: 因为 worktree 在 ~/Developer/self/muxterm/main/
# 所以 target-dir = "../../muxterm-target" 才指向 ~/Developer/self/muxterm-target/
```

需要修改 `.cargo/config.toml` 中的相对路径,或者用绝对路径:
```toml
[build]
target-dir = "/home/wlz/Developer/self/muxterm-target"
```

### Step 5: 配置 Codex / Cursor Agent
在 Ryzen 上安装和配置 agent 工具:
```bash
# Codex (如果 Ryzen 没有)
npm install -g @openai/codex
# 配置 ~/.codex/auth.json 和 ~/.codex/config.toml (从 archmini 复制)
```

### Step 6: 验证构建
```bash
cd ~/Developer/self/muxterm/main
cargo check
cargo test
# 预期: 270 passed, 0 failed
```

### Step 7: (可选) archmini 清理
- 保留 `~/Project/muxterm/` 目录不动(备份)
- 删除 `~/Project/muxterm-target/`
- 删除 tmux Codex/Cursor agent 会话

## Ryzen 开发目录结构(最终)
```
~/Developer/self/
├── muxterm.git/               # bare 仓库
├── muxterm/
│   └── main/                  # main worktree
├── muxterm-feature-x/         # feature worktrees
├── muxterm-target/            # 共享编译缓存
└── muxterm-issue-y/           # bugfix worktrees
```

## CI/CD
不需要修改。archmini 上的 CI/CD 已经配好(GitHub Actions),Ryzen 只需要 `git push`,自动触发构建和测试。