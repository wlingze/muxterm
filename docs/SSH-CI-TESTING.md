# SSH CI 测试说明

> 本文档说明 Linux CI 中 SSH long-chain 集成测试的运行方式与 workflow 命令。

## 测试矩阵

| location | runtime | CLI |
|----------|---------|-----|
| local | shell | `local_shell_cli` |
| local | tmux | `local_tmux_cli` |
| ssh | shell | `ssh_shell_cli`（`#[ignore]`，需 sshd） |
| ssh | tmux | `ssh_tmux_cli`（`#[ignore]`，需 sshd） |

TUI 集成测试在 `tests/tui_integration.rs`（local shell/tmux）。

## Workflow 文件

- `.github/workflows/four-mode-integration.yml`
  - `standard` job：local CLI + split/sendkeys/cli_integration/tmux_backend（不需要 sshd）
  - `ssh-integration` job：matrix strategy 运行 2 个 SSH CLI case

## SSH 测试环境要求

- Linux runner（`ubuntu-latest`）
- 安装 `openssh-server`（提供 `sshd`）、`openssh-client`（提供 `ssh`、`ssh-keygen`）、`tmux`
- `scripts/ci/setup-sshd.sh` 启动共享 loopback sshd（127.0.0.1:随机端口）
- 测试动态生成临时 host key / client key / authorized_keys / ssh_config / HOME
- 不访问公网，不读取用户真实 `~/.ssh/config`，不使用真实密钥
- 每个测试有硬超时；失败时上传 sshd 日志到 CI artifact
- 测试结束自动清理 sshd 进程和临时目录（Drop impl）

## 产品路径

SSH 测试走 muxterm 自己的 SSH transport（`SshProcessTransport`）：
- `muxterm tmux session list --target <alias>` → `list_ssh_tmux_sessions` → SSH transport
- `muxterm tmux pane list --target <alias> --session <name>` → `list_ssh_tmux_panes` → SSH transport

显式 `--target <alias>` 不回退 local（见 `tests/ssh_no_fallback.rs`）。
`MUXTERM_SSH_CONFIG_PATH` 环境变量传递显式 ssh config 路径。

## 本地复现

```bash
# 启动共享 sshd
MUXTERM_ENV_FILE=/tmp/muxterm-sshd.env bash scripts/ci/setup-sshd.sh
export $(cat /tmp/muxterm-sshd.env | xargs)

# local CLI（always-on）
cargo test --no-default-features --features ffi --test four_mode_integration -- local --nocapture

# SSH CLI（需要 sshd + --ignored）
cargo test --no-default-features --features ffi --test four_mode_integration -- --ignored --nocapture

# SSH transport 底层测试
cargo test --no-default-features --features ffi --test ssh_transport_unit -- --ignored --nocapture

# No-fallback 测试（不需要 sshd）
cargo test --no-default-features --features ffi --test ssh_no_fallback -- --nocapture
```

## 硬超时

所有测试都有硬超时（`run_with_timeout`）：
- local CLI: 60-120s
- SSH CLI: 45-60s
- CI workflow 级 timeout: 20-30 分钟
