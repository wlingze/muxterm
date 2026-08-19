#!/usr/bin/env python3
"""Two OSC 133 command rounds: success then failure, with command text.

Used by linux_command_marks_e2e. Suffix from MUXTERM_CMD_SUFFIX.
"""

from __future__ import annotations

import os
import sys
import time


def osc(payload: str) -> None:
    sys.stdout.write("\x1b]" + payload + "\x07")
    sys.stdout.flush()


def main() -> None:
    suffix = os.environ.get("MUXTERM_CMD_SUFFIX", "x")
    pad_lines = int(os.environ.get("MUXTERM_CMD_PAD_LINES", "0"))
    ok = f"CMD_OK_{suffix}"
    fail = f"CMD_FAIL_{suffix}"
    osc("133;A")
    osc("133;B")
    sys.stdout.write(ok + "\r\n")
    osc("133;C")
    sys.stdout.write("out_ok\r\n")
    osc("133;D;0")
    for i in range(pad_lines):
        sys.stdout.write(f"PAD_{i}_{suffix}\r\n")
    sys.stdout.flush()
    osc("133;A")
    osc("133;B")
    sys.stdout.write(fail + "\r\n")
    osc("133;C")
    sys.stdout.write("out_fail\r\n")
    osc("133;D;1")
    sys.stdout.flush()
    while True:
        time.sleep(3600)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(0)
