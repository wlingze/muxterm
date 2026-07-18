#!/bin/bash
# 用 unbuffer 分配 pty 抓取 tmux -CC 真实输出样本
set -e
OUT=tests/samples/new-session.txt
# 启动一个独立 tmux server 避免污染
timeout 2 unbuffer tmux -CC -L muxterm-sample new-session -s muxterm-sample -x 80 -y 24 > "$OUT" 2>&1 || true
# 关掉那个 server
tmux -L muxterm-sample kill-server 2>/dev/null || true
echo "saved $OUT, lines: $(wc -l < $OUT), bytes: $(wc -c < $OUT)"
