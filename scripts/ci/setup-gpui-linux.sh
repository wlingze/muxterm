#!/usr/bin/env bash
# GPUI 前端在 Linux 上的系统依赖（Debian / Ubuntu）。
#
# 与 `scripts/ci/setup.sh`（CI 的 tmux/herdr/sshd 环境）分开：ci-core 只跑
# 无 GUI 的 TUI / CLI，不需要这些；gpui 的 Linux job 与 release 的 Linux job
# 才装。缺库时 `scripts/build-gpui-linux.sh` 也会先给可读的安装提示。
set -euo pipefail

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "setup-gpui-linux.sh 只在 Linux 上运行（当前: $(uname -s)）" >&2
  exit 1
fi

sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  build-essential \
  pkg-config \
  libxkbcommon-dev \
  libxkbcommon-x11-dev \
  libwayland-dev \
  libx11-dev \
  libxcb1-dev \
  libxkbcommon0 \
  libfontconfig1-dev \
  libfreetype-dev \
  libasound2-dev \
  mesa-vulkan-drivers \
  libwayland-client0 \
  libx11-6 \
  fonts-dejavu-core \
  fontconfig

# gpui 的 Wayland 后端需要 xdg-desktop-portal 的 D-Bus 接口（ashpd）；
# 无头 CI 没有 session bus，编译期不需要，运行期才需要 —— 这里只装库。
echo "GPUI Linux 系统依赖已安装"
pkg-config --modversion xkbcommon wayland-client x11 fontconfig