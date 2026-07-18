#!/bin/bash
set -e
S=tests/samples
# 抓 new-session（带更多输出）
timeout 3 unbuffer tmux -CC -L muxterm-sample2 new-session -s mxs2 -x 120 -y 40 > $S/new-session-full.txt 2>&1 || true
tmux -L muxterm-sample2 kill-server 2>/dev/null || true
# 抓 layout-change / window-add 等：在 CC 模式下创建窗口并 resize
cat > /tmp/cc_drive.exp <<'EX'
set timeout 5
log_file -noappend tests/samples/driven.txt
spawn tmux -CC -L muxterm-driven new-session -s drv -x 100 -y 30
expect "%session-changed"
# 发送一些命令
send "echo hello\r"
sleep 1
send "ls\r"
sleep 1
# 不发 detach，直接 kill
send "\003"
sleep 1
close
EX
expect -f /tmp/cc_drive.exp 2>/dev/null || true
tmux -L muxterm-driven kill-server 2>/dev/null || true
echo "=== new-session-full ===" && wc -l $S/new-session-full.txt
echo "=== driven ===" && wc -l $S/driven.txt 2>/dev/null || true
