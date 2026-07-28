# SSH CI 测试说明

> 本文档说明 Linux CI 中 SSH E2E 测试的运行方式与 workflow 命令。

## 测试矩阵

Linux CI 必须运行全部 8 个 case：

| location | runtime | CLI | TUI |
|----------|---------|-----|-----|
| local | shell | `local_shell_cli` | `local_shell_tui` |
| local | tmux | `local_tmux_cli` | `local_tmux_tui` |
| ssh | shell | `ssh_shell_cli` | `ssh_shell_tui` |
| ssh | tmux | `ssh_tmux_cli` | `ssh_tmux_tui` |

## Workflow 文件

- `.github/workflows/four-mode-e2e.yml`
  - `standard` job：local CLI + local TUI（不需要 sshd）
  - `ssh-integration` job：matrix strategy 运行 4 个 SSH case

## 所需 workflow 变更

现有 `.github/workflows/ci.yml` 不含四模式 E2E。需在 CI 中引用 `four-mode-e2e.yml`，
或将其 job 合并到 `ci.yml`。

最小变更：在 `ci.yml` 末尾添加：

```yaml
  four-mode-e2e:
    name: Four-Mode E2E
    needs: ci
    uses: ./.github/workflows/four-mode-e2e.yml
```

或直接在 push/PR 触发时运行 `four-mode-e2e.yml`。

## SSH 测试环境要求

- Linux runner（`ubuntu-latest`）
- 安装 `openssh-server`（提供 `sshd`）、`openssh-client`（提供 `ssh`、`ssh-keygen`）、`tmux`
- 测试动态生成临时 host key / client key / authorized_keys / ssh_config / HOME
- sshd 只监听 `127.0.0.1:随机端口`
- 不访问公网，不读取用户真实 `~/.ssh/config`，不使用真实密钥
- 每个测试有硬超时；失败时上传 sshd 日志到 CI artifact
- 测试结束自动清理 sshd 进程和临时目录（Drop impl）

## 本地复现

```bash
# local CLI（always-on）
cargo test --no-default-features --features ffi --test four_mode_e2e -- local_shell_cli local_tmux_cli --nocapture

# local TUI（需要 --features tui + tmux）
cargo test --no-default-features --features tui --test four_mode_e2e -- local_shell_tui local_tmux_tui --nocapture --ignored

# SSH CLI（需要 sshd + tmux）
cargo test --no-default-features --features tui --test four_mode_e2e -- ssh_shell_cli ssh_tmux_cli --nocapture --ignored

# SSH TUI
cargo test --no-default-features --features tui --test four_mode_e2e -- ssh_shell_tui ssh_tmux_tui --nocapture --ignored
```

## 硬超时

所有测试都有硬超时（`run_with_timeout`）：
- local CLI: 10-20s
- local TUI: 30s
- SSH CLI: 30-45s
- SSH TUI: 45s
- CI workflow 级 timeout: 20-30 分钟
