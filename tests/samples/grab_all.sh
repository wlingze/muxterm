#!/bin/bash
# 抓取多场景真实 tmux -CC 输出样本
set -e
S=tests/samples
# 1. new-session 基础样本（已有，重新抓确保最新）
timeout 2 unbuffer tmux -CC -L mxt-sample new-session -s mxt-sample -x 80 -y 24 > $S/new-session.txt 2>&1 || true
tmux -L mxt-sample kill-server 2>/dev/null || true
# 2. 命令响应样本（list-sessions/display-message/new-window/list-windows）
cat > /tmp/cc_drv.exp <<'EX'
set timeout 8
log_file -noappend tests/samples/cmd-response.txt
spawn tmux -CC -L mxt-drv new-session -s drv -x 100 -y 30
expect "%window-renamed"
send "list-sessions\r"
expect "%end"
send "display-message -p '#{session_name}:#{window_index}.#{pane_index}'\r"
expect "%end"
send "new-window -n second\r"
expect "%window-add"
sleep 1
send "list-windows\r"
expect "%end"
sleep 1
close
EX
expect -f /tmp/cc_drv.exp 2>/dev/null || true
tmux -L mxt-drv kill-server 2>/dev/null || true
echo "samples refreshed"
