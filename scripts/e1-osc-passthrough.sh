#!/usr/bin/env bash
# E1: does tmux control-mode %output carry OSC 133 / BEL / OSC 9 / 777?
#
# LINUX-PLAN §8.1 C2.0. Drives one `tmux -CC` control client on an isolated
# socket (muxterm-test-e1-*), runs a helper inside the pane three times
# (default, allow-passthrough on, allow-passthrough all), then records the
# raw control-mode output as a committed fixture.
#
# Usage: scripts/e1-osc-passthrough.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OUT="$REPO_ROOT/tests/samples/osc-attention-tmux3.7b.txt"
TMP_OUT="$(mktemp "$REPO_ROOT/tests/samples/osc-attention-tmux3.7b.XXXXXX")"
SOCKET="muxterm-test-e1-$$"
FIFO="$(mktemp -u /tmp/e1-osc-fifo-XXXXXX)"
HELPER="$(mktemp /tmp/e1-osc-seq-XXXXXX.sh)"

cleanup() {
  tmux -L "$SOCKET" kill-server 2>/dev/null || true
  rm -f "$FIFO" "$HELPER" "$TMP_OUT"
}
trap cleanup EXIT

mkfifo "$FIFO"

# OSC/BEL 序列写在 pane 侧 helper 里，避免控制模式命令解析器吃掉转义。
cat > "$HELPER" <<'SH'
#!/bin/sh
printf '\033]133;C\007'
printf '\033]133;D;0\007'
printf '\a'
printf '\033]9;hello\007'
printf '\033]777;notify;t;b\007'
printf 'e1-round-done\007'
SH

TMUX_VERSION="$(tmux -V 2>/dev/null || echo unknown)"

{
  echo "# tmux version: $TMUX_VERSION"
  echo "# geometry: 80x24 (new-session -x 80 -y 24)"
  echo "# reproduction: run scripts/e1-osc-passthrough.sh"
  echo "#   round 1: default options"
  echo "#   round 2: set-option -g allow-passthrough on"
  echo "#   round 3: set-option -g allow-passthrough all"
  echo "#   each round runs a helper inside the pane:"
  echo "#     printf '\\033]133;C\\007'  (OSC 133 command start)"
  echo "#     printf '\\033]133;D;0\\007' (OSC 133 done)"
  echo "#     printf '\\a'               (BEL)"
  echo "#     printf '\\033]9;hello\\007'  (OSC 9 notify)"
  echo "#     printf '\\033]777;notify;t;b\\007' (OSC 777 notify)"
  echo "# E1 conclusion: PLACEHOLDER"
  echo "# raw control-mode output follows (C-escaped %output lines):"
} > "$TMP_OUT"

emit() { printf '%s\r' "$1" >&3; }

timeout 15 script -q -a -e -c "tmux -L '$SOCKET' -f /dev/null -CC new-session -s e1 -x 80 -y 24" "$TMP_OUT" < "$FIFO" &
PID=$!
exec 3>"$FIFO"

sleep 1.2
emit 'display-message -p R1-DEFAULT-START'
sleep 0.4
emit "send-keys -t e1 \"bash $HELPER\" Enter"
sleep 1.0
emit 'display-message -p R1-DEFAULT-END'
sleep 0.4

emit 'set-option -g allow-passthrough on'
sleep 0.5
emit 'display-message -p R2-ALLOW-ON-START'
sleep 0.4
emit "send-keys -t e1 \"bash $HELPER\" Enter"
sleep 1.0
emit 'display-message -p R2-ALLOW-ON-END'
sleep 0.4

emit 'set-option -g allow-passthrough all'
sleep 0.5
emit 'display-message -p R3-ALLOW-ALL-START'
sleep 0.4
emit "send-keys -t e1 \"bash $HELPER\" Enter"
sleep 1.0
emit 'display-message -p R3-ALLOW-ALL-END'
sleep 0.5
# 控制模式 Ctrl-C 退出 tmux -CC；timeout 兜底防挂起。
emit "$(printf '\003')"
sleep 0.5

exec 3>&-
wait "$PID" || true

# 分类：PASS_THROUGH = round 1 默认即有 OSC 133；NEED_ALLOW = 仅 on/all 后有；
# ABSENT = 始终没有。
round1="$(sed -n '/R1-DEFAULT-START/,/R1-DEFAULT-END/p' "$TMP_OUT")"
round23="$(sed -n '/R2-ALLOW-ON-START/,$p' "$TMP_OUT")"
result="ABSENT"
detail="no OSC 133, BEL or notify sequences observed in %output"
if printf '%s' "$round1" | rg -q '133;([CD])'; then
  result="PASS_THROUGH"
  detail="OSC 133 C/D present in %output without allow-passthrough (three-state)"
elif printf '%s' "$round23" | rg -q '133;([CD])'; then
  result="NEED_ALLOW"
  detail="OSC 133 only after allow-passthrough on/all"
fi

has_bel="no"
if rg -q '\\a|\\007|]9;|]777;' "$TMP_OUT"; then
  has_bel="yes"
  if [ "$result" = "ABSENT" ]; then
    detail="BEL/OSC 9/777 present but no OSC 133 (two-state needs-you)"
  fi
fi

sed -i "s|^# E1 conclusion: PLACEHOLDER$|# E1 conclusion: $result ($detail; BEL/notify observed: $has_bel)|" "$TMP_OUT"
mv -f "$TMP_OUT" "$OUT"
echo "E1 fixture: $OUT"
echo "conclusion: $result; BEL/notify: $has_bel"
rg '%output' "$OUT" >/dev/null && echo "rg '%output' fixture: OK"
