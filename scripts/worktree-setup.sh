#!/bin/bash
# 用法: ./scripts/worktree-setup.sh <feature-name>
set -e
NAME=$1
if [ -z "$NAME" ]; then
  echo "用法: $0 <feature-name>" >&2
  exit 1
fi
WORKTREE="/home/wlz/Project/muxterm-$NAME"
git worktree add -b "feat/$NAME" "$WORKTREE" main
cd "$WORKTREE"
echo "Worktree 已创建: $WORKTREE"
echo "分支: feat/$NAME"
echo "编译缓存/产物使用仓库本地 ./target 与 ./build（不跨 worktree 共享）"
