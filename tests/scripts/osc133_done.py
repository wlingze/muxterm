#!/usr/bin/env python3
"""Write OSC 133 C/D to stdout for attention-signal e2e.

tmux `send-keys -H`/`-l` 会把控制字节转成 `^[`/`^G` 字面量，OSC 133 必须
由 pane 进程直接写 stdout 才能原样进 `%output`（与 mock_codex.py 同理）。
"""

from __future__ import annotations

import sys
import time


def main() -> None:
    out = sys.stdout
    out.write("\x1b]133;C\x07")
    out.write("TASK_DONE_TOKEN")
    out.flush()
    # Cross the ordinary-command visibility threshold so this fixture tests
    # an observable completed task instead of the intentional flash filter.
    time.sleep(0.5)
    out.write("\x1b]133;D;0\x07")
    out.flush()
    while True:
        time.sleep(3600)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(0)
