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


SEND_KEY_BYTES = {
    "enter": b"\r",
    "escape": b"\x1b",
}


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

    send_bytes = bytes(args.send_text, "utf-8") if args.send_text is not None else b""
    if args.send_key:
        send_bytes += SEND_KEY_BYTES[args.send_key]
    send_after = args.send_after
    send_done = not send_after

    process = subprocess.Popen(
        args.command,
        stdin=subprocess.PIPE if send_after else subprocess.DEVNULL,
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

    def maybe_send_input() -> None:
        nonlocal send_done
        if send_done or not send_after:
            return
        text = captured.decode("utf-8", errors="replace")
        if send_after not in text:
            return
        if args.send_delay > 0:
            time.sleep(args.send_delay)
        if process.stdin is not None and send_bytes:
            try:
                os.write(process.stdin.fileno(), send_bytes)
            except BrokenPipeError:
                pass
        send_done = True

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
                maybe_send_input()
                if update_found():
                    if send_done:
                        return report_success()
                break

            timeout = max(0.05, min(0.5, deadline - time.monotonic()))
            for key, _events in selector.select(timeout):
                chunk = os.read(key.fileobj.fileno(), 4096)
                if not chunk:
                    continue
                captured.extend(chunk)
                maybe_send_input()
                if update_found():
                    if send_done:
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
    parser.add_argument("--send-after", help="output marker after which input is sent")
    parser.add_argument("--send-delay", type=float, default=0.25, help="seconds to wait before sending input")
    parser.add_argument("--send-text", help="literal text to send to QEMU stdin")
    parser.add_argument("--send-key", choices=sorted(SEND_KEY_BYTES), help="named key to send to QEMU stdin")
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
