#!/usr/bin/env python3
"""OSC 133 C then D, no extra BEL.

`osc133_done.py` 在 D 之后又写了一个 BEL，会把 Done 盖成 Blocked。
前台「跑完不算通知」和「看见 Done 才熄」必须用这条，不能复用带 BEL 的脚本。
"""

from __future__ import annotations

import sys
import time


def main() -> None:
    out = sys.stdout
    out.write("\x1b]133;C\x07")
    out.write("CMD_DONE_ONLY")
    out.write("\x1b]133;D;0\x07")
    out.flush()
    while True:
        time.sleep(3600)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(0)
