#!/usr/bin/env bash
# CI 环境 setup：安装并配置 tmux / herdr / loopback sshd / 系统依赖。
#
# 用法：
#   bash scripts/ci/setup.sh            # stdout 输出 shell-safe KEY=VALUE
#   MUXTERM_ENV_FILE=... bash scripts/ci/setup.sh   # 追加 KEY=VALUE 到文件
#
# 职责（平台自适应）：
#   - tmux：Linux apt / macOS brew
#   - herdr：GitHub release 预编译二进制（herdrdev/herdr v0.8.0；crates.io
#     无此包，`cargo install herdr` 会装到同名无关 crate）
#   - loopback sshd：Linux 复用 setup-sshd.sh；macOS 由测试按需 skip
#   - 其它系统依赖：build-essential / pkg-config / python3 / openssh
#
# 设计约束：
#   - 幂等：已装的工具跳过安装。
#   - stdout 只输出 `KEY=VALUE`（或经 MUXTERM_ENV_FILE 落盘），诊断走 stderr。
#   - 失败即退出非零，由 workflow 直接失败。
set -euo pipefail

OS="$(uname -s)"
ENV_LINES=""

# ── 系统依赖 ──
if [ "$OS" = "Linux" ]; then
  sudo apt-get update
  sudo apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    openssh-server \
    openssh-client \
    python3 \
    curl \
    libevent-dev \
    libncurses-dev \
    libutempter-dev \
    bison \
    flex
  # tmux：ubuntu 仓库只有 3.4，控制模式事件流与本地 3.7b 有差异
  # （detach/reattach 集成测试依赖 3.7b 行为），编译安装 3.7b。
  if ! tmux -V 2>/dev/null | grep -q "3.7b"; then
    echo "编译 tmux 3.7b（ubuntu 仓库版本过旧）..." >&2
    curl -fsSL -o /tmp/tmux-3.7b.tar.gz \
      https://github.com/tmux/tmux/releases/download/3.7b/tmux-3.7b.tar.gz
    tar xzf /tmp/tmux-3.7b.tar.gz -C /tmp
    (cd /tmp/tmux-3.7b \
      && ./configure --prefix=/usr/local >/dev/null \
      && make -j"$(nproc)" >/dev/null \
      && sudo make install >/dev/null)
    rm -rf /tmp/tmux-3.7b /tmp/tmux-3.7b.tar.gz
  fi
elif [ "$OS" = "Darwin" ]; then
  brew list tmux >/dev/null 2>&1 || brew install tmux
else
  echo "不支持的平台: $OS" >&2
  exit 1
fi

# ── herdr（GitHub release 预编译二进制，固定 0.8.0 与本地一致）──
if ! command -v herdr >/dev/null 2>&1; then
  ARCH="$(uname -m)"
  case "$OS-$ARCH" in
    Linux-x86_64)  ASSET="herdr-linux-x86_64" ;;
    Linux-aarch64) ASSET="herdr-linux-aarch64" ;;
    Darwin-arm64)  ASSET="herdr-macos-aarch64" ;;
    Darwin-x86_64) ASSET="herdr-macos-x86_64" ;;
    *) echo "不支持的平台组合: $OS-$ARCH" >&2; exit 1 ;;
  esac
  echo "下载 herdr 0.8.0 ($ASSET)..." >&2
  URL="https://github.com/herdrdev/herdr/releases/download/v0.8.0/$ASSET"
  if [ "$OS" = "Linux" ]; then
    curl -fsSL -o /tmp/herdr "$URL"
    sudo install -m 0755 /tmp/herdr /usr/local/bin/herdr
    rm -f /tmp/herdr
  else
    curl -fsSL -o /tmp/herdr "$URL"
    install -m 0755 /tmp/herdr /usr/local/bin/herdr
    rm -f /tmp/herdr
  fi
fi
herdr --version >&2
ENV_LINES="$ENV_LINES
MUXTERM_HERDR_VERSION=$(herdr --version | head -1)"

# ── loopback sshd（Linux；macOS 测试按 env 探测自行 skip）──
if [ "$OS" = "Linux" ]; then
  if [ -n "${MUXTERM_ENV_FILE:-}" ]; then
    MUXTERM_ENV_FILE="$MUXTERM_ENV_FILE" bash scripts/ci/setup-sshd.sh
  else
    SSH_ENV="$(bash scripts/ci/setup-sshd.sh)"
    ENV_LINES="$ENV_LINES
$SSH_ENV"
  fi
fi

# ── 输出 ──
if [ -n "${MUXTERM_ENV_FILE:-}" ]; then
  printf '%s\n' "$ENV_LINES" >> "$MUXTERM_ENV_FILE"
else
  printf '%s\n' "$ENV_LINES"
fi

echo "CI setup 完成: tmux $(tmux -V), herdr $(herdr --version | head -1)" >&2
