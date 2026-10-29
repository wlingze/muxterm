#!/usr/bin/env bash
# CI 环境 setup：安装并配置 tmux / herdr / loopback sshd / 系统依赖。
#
# 用法：
#   bash scripts/ci/setup.sh            # stdout 输出 shell-safe KEY=VALUE
#   MUXTERM_ENV_FILE=... bash scripts/ci/setup.sh   # 追加 KEY=VALUE 到文件
#
# 职责（平台自适应）：
#   - tmux：Linux apt / macOS brew
#   - herdr：GitHub latest 稳定版预编译二进制（不钉版本号；crates.io 无此包，
#     `cargo install herdr` 会装到同名无关 crate）
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

verify_sha256() {
  local expected="$1"
  local file="$2"
  if command -v sha256sum >/dev/null 2>&1; then
    printf '%s  %s\n' "$expected" "$file" | sha256sum -c - >/dev/null
  else
    printf '%s  %s\n' "$expected" "$file" | shasum -a 256 -c - >/dev/null
  fi
}

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
    tmux \
    libevent-dev \
    libncurses-dev \
    libfontconfig1-dev \
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
  # Debian/Ubuntu 的交互 zsh 在没有任何 startup file 时会跑
  # zsh-newuser-install（全屏 TUI）。apt+chsh 之后 runner 没有 ~/.zshrc；
  # SSH pane 默认 80x24，向导会真正启动（COLUMNS<72 时它自己 return）。
  # 向导吞掉 WriteRaw，SSH_CONNECTION 探测永远不会执行。
  # 官方跳过方式是写一份只含注释的 ~/.zshrc。禁止从 test.sh 写 $HOME：
  # 那会改开发机；本脚本只在 CI runner 上跑。
  if [ ! -e "${HOME}/.zshrc" ]; then
    printf '%s\n' '# muxterm-ci: skip zsh-newuser-install' >"${HOME}/.zshrc"
  fi
  # tmux 使用 Ubuntu 的预编译包；setup 不从源码编译，避免 CI runner 长时间占用。
  if ! command -v tmux >/dev/null 2>&1; then
    echo "tmux 安装失败" >&2
    exit 1
  fi
elif [ "$OS" = "Darwin" ]; then
  # Homebrew 安装预编译 bottle；setup 不从源码编译，版本跟随 runner 的当前 formula。
  brew list tmux >/dev/null 2>&1 || brew install --force-bottle tmux
  if ! command -v tmux >/dev/null 2>&1; then
    echo "tmux 安装失败" >&2
    exit 1
  fi
else
  echo "不支持的平台: $OS" >&2
  exit 1
fi

# ── herdr（GitHub latest 稳定版，版本号与 SHA 都不写进仓库）──
# /releases/latest 只返回非 prerelease。摘要取该 asset 的 digest 字段，
# 下载后校验；协议不兼容由 Muxterm 握手明确失败，不在这里猜协议号。
if ! command -v python3 >/dev/null 2>&1; then
  echo "解析 herdr release 需要 python3" >&2
  exit 1
fi
ARCH="$(uname -m)"
case "$OS-$ARCH" in
  Linux-x86_64)  ASSET="herdr-linux-x86_64" ;;
  Linux-aarch64) ASSET="herdr-linux-aarch64" ;;
  Darwin-arm64)  ASSET="herdr-macos-aarch64" ;;
  Darwin-x86_64) ASSET="herdr-macos-x86_64" ;;
  *) echo "不支持的平台组合: $OS-$ARCH" >&2; exit 1 ;;
esac
# runner 出口 IP 共用时，匿名 /releases/latest 很容易撞上 60 次/小时。
# workflow 传入 GH_TOKEN 后走认证额度。
HERDR_CURL=(curl -fsSL
  -H "Accept: application/vnd.github+json"
  -H "X-GitHub-Api-Version: 2022-11-28"
  -H "User-Agent: muxterm-ci")
if [ -n "${GH_TOKEN:-${GITHUB_TOKEN:-}}" ]; then
  HERDR_CURL+=(-H "Authorization: Bearer ${GH_TOKEN:-${GITHUB_TOKEN}}")
fi
HERDR_META="$("${HERDR_CURL[@]}" https://api.github.com/repos/herdrdev/herdr/releases/latest)"
HERDR_RESOLVED="$(printf '%s' "$HERDR_META" | python3 -c '
import json, sys
asset = sys.argv[1]
rel = json.load(sys.stdin)
if rel.get("prerelease") or rel.get("draft"):
    raise SystemExit("refusing herdr prerelease or draft")
tag = rel.get("tag_name") or ""
if not tag.startswith("v") or len(tag) < 2:
    raise SystemExit(f"unexpected herdr tag: {tag!r}")
found = next((item for item in rel.get("assets") or [] if item.get("name") == asset), None)
if found is None:
    raise SystemExit(f"release {tag} has no {asset}")
digest = found.get("digest") or ""
prefix = "sha256:"
if not digest.startswith(prefix) or len(digest) != len(prefix) + 64:
    raise SystemExit(f"release {tag} asset {asset} has no sha256 digest")
url = found.get("browser_download_url") or ""
expected = f"https://github.com/herdrdev/herdr/releases/download/{tag}/{asset}"
if url != expected:
    raise SystemExit(f"unexpected herdr download url: {url}")
print(tag)
print(digest[len(prefix):])
print(url)
' "$ASSET")"
HERDR_TAG="$(printf '%s\n' "$HERDR_RESOLVED" | sed -n '1p')"
HERDR_SHA="$(printf '%s\n' "$HERDR_RESOLVED" | sed -n '2p')"
HERDR_URL="$(printf '%s\n' "$HERDR_RESOLVED" | sed -n '3p')"
HERDR_VERSION="${HERDR_TAG#v}"
if command -v herdr >/dev/null 2>&1 \
  && [ "$(herdr --version 2>/dev/null | head -1)" = "herdr ${HERDR_VERSION}" ]; then
  echo "herdr ${HERDR_VERSION} 已是最新稳定版，跳过安装" >&2
else
  echo "下载 herdr ${HERDR_TAG} (${ASSET})..." >&2
  curl -fsSL -o /tmp/herdr "$HERDR_URL"
  verify_sha256 "$HERDR_SHA" /tmp/herdr
  if [ "$OS" = "Linux" ]; then
    sudo install -m 0755 /tmp/herdr /usr/local/bin/herdr
  else
    install -m 0755 /tmp/herdr /usr/local/bin/herdr
  fi
  rm -f /tmp/herdr
fi
if [ "$(herdr --version 2>/dev/null | head -1)" != "herdr ${HERDR_VERSION}" ]; then
  echo "herdr 版本必须为最新稳定版 ${HERDR_VERSION}，实际: $(herdr --version 2>/dev/null | head -1)" >&2
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
