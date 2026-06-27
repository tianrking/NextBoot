#!/usr/bin/env python3
"""Run QEMU until expected NextBoot boot log markers appear."""

from __future__ import annotations

import argparse
import os
import selectors
import signal
import subprocess
import sys
import time


def terminate(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def run_smoke(args: argparse.Namespace) -> int:
    if not args.command:
        print("qemu-boot-smoke: missing command after --", file=sys.stderr)
        return 2

    process = subprocess.Popen(
        args.command,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    assert process.stdout is not None

    selector = selectors.DefaultSelector()
    selector.register(process.stdout, selectors.EVENT_READ)

    deadline = time.monotonic() + args.timeout
    captured = bytearray()
    expected = list(args.expect)
    found = {item: False for item in expected}

    def update_found() -> bool:
        text = captured.decode("utf-8", errors="replace")
        for item in expected:
            if not found[item] and item in text:
                found[item] = True
        return all(found.values())

    def report_success() -> int:
        if args.log:
            with open(args.log, "wb") as out:
                out.write(captured)
        print("QEMU boot smoke passed")
        for item in expected:
            print(f"  found: {item}")
        return 0

    try:
        while time.monotonic() < deadline:
            if process.poll() is not None:
                remaining = process.stdout.read()
                if remaining:
                    captured.extend(remaining)
                if update_found():
                    return report_success()
                break

            timeout = max(0.05, min(0.5, deadline - time.monotonic()))
            for key, _events in selector.select(timeout):
                chunk = os.read(key.fileobj.fileno(), 4096)
                if not chunk:
                    continue
                captured.extend(chunk)
                if update_found():
                    terminate(process)
                    return report_success()
        terminate(process)
    except KeyboardInterrupt:
        terminate(process)
        raise
    finally:
        selector.close()

    if args.log:
        with open(args.log, "wb") as out:
            out.write(captured)

    text = captured.decode("utf-8", errors="replace")
    missing = [item for item, ok in found.items() if not ok]
    print("QEMU boot smoke failed", file=sys.stderr)
    if process.returncode is not None and process.returncode not in (
        -signal.SIGTERM,
        -signal.SIGKILL,
    ):
        print(f"  QEMU exited with status {process.returncode}", file=sys.stderr)
    for item in missing:
        print(f"  missing: {item}", file=sys.stderr)
    print("--- QEMU output tail ---", file=sys.stderr)
    print(text[-4000:], file=sys.stderr)
    return 1


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout", type=float, default=20.0, help="seconds to wait")
    parser.add_argument("--log", help="optional path to write captured QEMU output")
    parser.add_argument(
        "--expect",
        action="append",
        default=[],
        help="text that must appear in QEMU output; repeatable",
    )
    parser.add_argument("command", nargs=argparse.REMAINDER, help="QEMU command after --")
    args = parser.parse_args(argv)
    if args.command and args.command[0] == "--":
        args.command = args.command[1:]
    if not args.expect:
        args.expect = ["NextBoot v"]
    return args


def main(argv: list[str]) -> int:
    return run_smoke(parse_args(argv))


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
