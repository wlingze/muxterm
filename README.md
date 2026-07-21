# Muxterm

Linux 桌面端的 **tmux control mode (`-CC`) 原生 UI 终端**。

用 Rust + GTK4 把本地或远程 tmux 的 session / window / pane 渲染成原生 tab 与分割布局，而不是「黑框终端 + `Ctrl+B`」。体验上接近 iTerm2 的 tmux 集成，但面向 Linux。

> 状态：Phase 1 开发中（Linux）。详见 [`PRODUCT.md`](PRODUCT.md)。

## 功能概览

- **tmux `-CC` 原生集成**：pane 输出进原生终端视图，布局变化同步到 UI
- **Tab / Pane**：每个 pane 可对应 tab；支持水平 / 垂直分割
- **本地与远程**：本地 `tmux -CC`，以及通过 SSH 连接远程 tmux session
- **命令面板**：VSCode 风格（默认 `Alt+P`），含 `ssh: connect` 等命令
- **可配置**：TOML 配置（字体、主题、快捷键、SSH、滚动缓冲等）

## 截图

截图请放在 [`assets/screenshots/`](assets/screenshots/)（当前仓库占位说明见该目录）。

建议文件名：

| 文件 | 说明 |
|------|------|
| `overview.png` | 主界面：tab 栏 + pane 分割 |
| `command-palette.png` | 命令面板 |
| `ssh-connect.png` | SSH 远程连接流程 |

添加截图后，可在本节引用，例如：

```markdown
![主界面](assets/screenshots/overview.png)
```

## 依赖环境

| 组件 | 说明 |
|------|------|
| Linux | 桌面环境（GTK4） |
| Rust | 建议 stable（开发机实测 `rustc 1.97.1`） |
| tmux | 建议 3.x（实测 `3.7b`） |
| 系统库 | `gtk4`、`vte`（GTK4 版）、OpenSSL 等开发包 |

Arch 示例：

```bash
sudo pacman -S rust gtk4 vte4 openssl pkgconf tmux
```

Debian / Ubuntu 示例：

```bash
sudo apt-get install -y build-essential pkg-config \
  libgtk-4-dev libvte-2.91-gtk4-dev libssl-dev tmux
```

## 安装

目前以源码构建为主：

```bash
git clone https://github.com/wlingze/muxterm.git
cd muxterm
cargo build --release
./target/release/muxterm
```

调试运行：

```bash
cargo run
# 或更详细日志
cargo run -- --verbose
```

可选：把二进制装到 PATH：

```bash
cargo install --path .
```

发布版二进制也可通过打 tag（`v*.*.*`）触发 GitHub Actions Release 工作流生成 tarball（见 [`.github/workflows/release.yml`](.github/workflows/release.yml)）。

## 配置

复制示例配置：

```bash
mkdir -p ~/.config/muxterm
cp configs/config.example.toml ~/.config/muxterm/config.toml
```

常用段落：

- `[font]` / `[theme]` — 字体与主题（主题文件在 `configs/themes/`）
- `[tmux]` — 鼠标、默认 session
- `[ssh]` — 远程 host / port / user / key
- `[[keybindings]]` — 快捷键自定义

## 使用方法

启动后，Muxterm 会以 GTK4 窗口呈现本地/远程 tmux 的 pane。

### 默认快捷键

| 快捷键 | 作用 |
|--------|------|
| `Alt+N` | 新窗口（new-window ≈ 新 tab + pane） |
| `Alt+T` | 新本地 shell tab |
| `Alt+D` | 水平分割 pane |
| `Alt+Shift+D` | 垂直分割 pane |
| `Alt+1` … `Alt+9` | 切换到第 N 个 tab |
| `Alt+0` | 最后一个 tab |
| `Alt+[` / `Alt+]` | 上一个 / 下一个 pane |
| `Alt+R` | Pane 切换器（模糊搜索） |
| `Alt+P` | 命令面板 |
| `Alt+Shift+Q` | 退出 |

### 命令面板与 SSH

1. 按 `Alt+P` 打开命令面板  
2. 选择 `ssh: connect`（或输入关键字过滤）  
3. 在 QuickPick 中输入 `user@host`（或依赖 `config.toml` 的 `[ssh]`）  
4. 连接成功后，远程 tmux `-CC` 的 pane 会出现在本地 UI 中  

断开可用命令面板中的 `ssh: disconnect`。

### CLI

```text
muxterm [OPTIONS]

Options:
  -v, --verbose  启用详细日志（也可用 RUST_LOG）
  -h, --help
  -V, --version
```

## 项目结构

```text
muxterm/
├── src/
│   ├── main.rs                 # 入口（clap + tracing）
│   ├── core/                   # 可跨平台核心
│   │   ├── tmux/               # -CC 协议解析、命令、本地 pty 客户端
│   │   ├── ssh/                # 远程 tmux -CC（SSH）
│   │   ├── terminal/           # 进程 / scrollback / 输入抽象
│   │   ├── config.rs           # TOML 配置
│   │   └── types.rs
│   └── platform/linux/         # GTK4 + vte4 UI
│       ├── app.rs / window.rs  # 应用与主窗口
│       ├── notebook.rs         # tab
│       ├── pane_view.rs        # pane 终端视图
│       ├── command_palette.rs  # 命令面板
│       └── ...
├── configs/                    # 示例配置与主题
├── assets/                     # 样式与截图
├── tests/                      # 集成 / 样例测试
├── PRODUCT.md                  # 产品规划
├── ARCHITECTURE.md             # 架构与交互规范
└── AGENTS.md                   # 给 coding agent 的开发约定
```

更细的模块职责见 [`ARCHITECTURE.md`](ARCHITECTURE.md)。

## 开发指南

开始前建议阅读：

1. [`PRODUCT.md`](PRODUCT.md) — 产品目标与路线图  
2. [`ARCHITECTURE.md`](ARCHITECTURE.md) — 交互与模块边界  
3. [`AGENTS.md`](AGENTS.md) — commit / 测试约定  

常用命令：

```bash
cargo fmt
cargo check
cargo clippy --all-targets
cargo test
```

约定摘要：

- Rust 2021；应用层 `anyhow`，库层错误用 `thiserror`
- 日志用 `tracing`，不要用 `println!` 调试
- tmux 协议解析保持纯函数，便于单元测试
- 增量提交：`feat:` / `fix:` / `test:` / `refactor:` / `docs:` / `ci:` / `chore:`
- 不要在 commit message 里加 `Co-authored-by`

CI（push / PR 到 `main`）会跑 check、fmt、clippy、test；详见 [`.github/workflows/ci.yml`](.github/workflows/ci.yml)。

## 许可证

MIT（见 `Cargo.toml` 中的 `license` 字段）。
