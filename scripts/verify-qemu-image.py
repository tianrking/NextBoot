#!/usr/bin/env python3
"""Verify NextBoot raw QEMU disk images."""

from __future__ import annotations

import sys

from qemu_verify.cli import main


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
