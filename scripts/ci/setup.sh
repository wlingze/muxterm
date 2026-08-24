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
    flex \
    fonts-dejavu-core \
    zsh
  # 统一 pane shell 为 zsh（与本地开发机一致）：herdr pane 用 passwd 的
  # 登录 shell，CI runner 默认 bash 的多行彩色 PS1 与本地 zsh 环境不同，
  # 会让 detach/reattach 内容连续性测试在 CI 上表现不一致。
  if ! command -v zsh >/dev/null 2>&1; then
    echo "zsh 安装失败" >&2
    exit 1
  fi
  if [ "$(getent passwd "$USER" | cut -d: -f7)" != "$(command -v zsh)" ]; then
    echo "设置默认 shell 为 zsh（$USER）..." >&2
    sudo chsh -s "$(command -v zsh)" "$USER"
  fi
  # tmux：ubuntu 仓库只有 3.4，控制模式事件流与本地 3.7c 有差异
  # （detach/reattach 集成测试依赖 3.7c 行为），编译安装官方 3.7c。
  if ! tmux -V 2>/dev/null | grep -q "3.7c"; then
    echo "编译 tmux 3.7c（ubuntu 仓库版本过旧）..." >&2
    curl -fsSL -o /tmp/tmux-3.7c.tar.gz \
      https://github.com/tmux/tmux/releases/download/3.7c/tmux-3.7c.tar.gz
    echo "7c60cae9a0e25288e2e24750aafc9e8800fc7fd4555e447e1b29ee4201cfb3bf  /tmp/tmux-3.7c.tar.gz" \
      | sha256sum -c - >/dev/null
    tar xzf /tmp/tmux-3.7c.tar.gz -C /tmp
    (cd /tmp/tmux-3.7c \
      && ./configure --prefix=/usr/local >/dev/null \
      && make -j"$(nproc)" >/dev/null \
      && sudo make install >/dev/null)
    rm -rf /tmp/tmux-3.7c /tmp/tmux-3.7c.tar.gz
  fi
  # 精确校验：tmux -V == tmux 3.7c（§13.1）。
  if ! tmux -V | grep -q "3.7c"; then
    echo "tmux 版本必须为 3.7c，实际: $(tmux -V)" >&2
    exit 1
  fi
elif [ "$OS" = "Darwin" ]; then
  brew list tmux >/dev/null 2>&1 || brew install tmux
else
  echo "不支持的平台: $OS" >&2
  exit 1
fi

# ── herdr（GitHub release 预编译二进制，固定 0.8.0 与本地一致）──
# 已存在但版本不匹配时必须安装固定资产或失败，不能只判断 command 存在（§13.1）。
if command -v herdr >/dev/null 2>&1 && herdr --version 2>/dev/null | grep -q "0.8.0"; then
  echo "herdr 0.8.0 已存在，跳过安装" >&2
elif ! command -v herdr >/dev/null 2>&1 || ! herdr --version 2>/dev/null | grep -q "0.8.0"; then
  ARCH="$(uname -m)"
  case "$OS-$ARCH" in
    Linux-x86_64)  ASSET="herdr-linux-x86_64";    SHA="b872ea7e40fa2cb17e857ac9b62b1bf26db7b403c622f5d2f3f5b35f6e9acd28" ;;
    Linux-aarch64) ASSET="herdr-linux-aarch64";   SHA="f647ac66468d9efbc642fe534fb284468f0aea60641606fc008dfc0d82a3ca87" ;;
    Darwin-arm64)  ASSET="herdr-macos-aarch64";   SHA="d53a9f93fccfdfcc55632927bf51002f5add0aa7990bcdf508ffbd84ac658178" ;;
    Darwin-x86_64) ASSET="herdr-macos-x86_64";    SHA="77cb5afd6c8fcaaaf3bc28e474ec01c209331ad08094e20d7f8aa9b0bb78d649" ;;
    *) echo "不支持的平台组合: $OS-$ARCH" >&2; exit 1 ;;
  esac
  echo "下载 herdr 0.8.0 ($ASSET)..." >&2
  URL="https://github.com/herdrdev/herdr/releases/download/v0.8.0/$ASSET"
  curl -fsSL -o /tmp/herdr "$URL"
  echo "$SHA  /tmp/herdr" | sha256sum -c - >/dev/null
  if [ "$OS" = "Linux" ]; then
    sudo install -m 0755 /tmp/herdr /usr/local/bin/herdr
  else
    install -m 0755 /tmp/herdr /usr/local/bin/herdr
  fi
  rm -f /tmp/herdr
fi
# 精确校验：herdr 0.8.0（protocol 19）。
if ! herdr --version 2>/dev/null | grep -q "0.8.0"; then
  echo "herdr 版本必须为 0.8.0，实际: $(herdr --version 2>/dev/null | head -1)" >&2
  exit 1
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
