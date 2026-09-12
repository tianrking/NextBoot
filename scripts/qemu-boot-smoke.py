#!/usr/bin/env python3
"""Run QEMU until expected NextBoot boot log markers appear."""

from __future__ import annotations

import argparse
import os
import queue
import threading
import json
import socket
import subprocess
import sys
import time
from terminal_probe import TerminalProbe


SEND_KEY_BYTES = {
    "enter": b"\r",
    "escape": b"\x1b",
}


def send_qmp_key(port: int, key: str) -> None:
    """Send a real emulated keyboard event; redirected serial stdin is not ConIn on every host."""
    with socket.create_connection(('127.0.0.1', port), timeout=5) as client:
        with client.makefile('rb') as stream:
            greeting = json.loads(stream.readline())
            if 'QMP' not in greeting:
                raise ValueError('invalid QMP greeting')
            for index, command in enumerate([
                {'execute': 'qmp_capabilities'},
                {'execute': 'send-key', 'arguments': {'keys': [{'type': 'qcode', 'data': {'enter': 'ret', 'escape': 'esc'}[key]}]}},
            ]):
                command['id'] = index
                client.sendall(json.dumps(command).encode() + b'\n')
                while True:
                    response = json.loads(stream.readline())
                    if response.get('id') == index:
                        if 'error' in response:
                            raise ValueError(f'QMP keyboard event failed: {response["error"]}')
                        break


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
    if args.log:
        with open(args.log, 'wb'):
            pass

    process = subprocess.Popen(
        args.command,
        stdin=subprocess.PIPE if send_after or args.terminal_probes else subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    assert process.stdout is not None

    chunks: queue.Queue[bytes | None] = queue.Queue()

    def read_output() -> None:
        try:
            while True:
                chunk = os.read(process.stdout.fileno(), 4096)
                if not chunk:
                    break
                chunks.put(chunk)
        finally:
            chunks.put(None)

    reader = threading.Thread(target=read_output, daemon=True)
    reader.start()

    deadline = time.monotonic() + args.timeout
    captured = bytearray()
    expected = list(args.expect)
    found = {item: False for item in expected}
    terminal = TerminalProbe() if args.terminal_probes else None

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
        if args.qmp_port:
            send_qmp_key(args.qmp_port, args.send_key)
        elif process.stdin is not None and send_bytes:
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
            timeout = max(0.05, min(0.5, deadline - time.monotonic()))
            try:
                chunk = chunks.get(timeout=timeout)
            except queue.Empty:
                continue
            if chunk is None:
                break
            captured.extend(chunk)
            if terminal is not None and process.stdin is not None:
                response = terminal.feed(chunk)
                if response:
                    try:
                        process.stdin.write(response)
                        process.stdin.flush()
                    except BrokenPipeError:
                        pass
            if args.log:
                with open(args.log, 'ab') as live_log:
                    live_log.write(chunk)
            maybe_send_input()
            if update_found() and send_done:
                terminate(process)
                return report_success()
        terminate(process)
    except KeyboardInterrupt:
        terminate(process)
        raise
    finally:
        terminate(process)
        reader.join(timeout=3)
        process.stdout.close()
        if process.stdin is not None:
            process.stdin.close()

    if args.log:
        with open(args.log, "wb") as out:
            out.write(captured)

    text = captured.decode("utf-8", errors="replace")
    missing = [item for item, ok in found.items() if not ok]
    print("QEMU boot smoke failed", file=sys.stderr)
    if process.returncode is not None:
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
    parser.add_argument('--qmp-port', type=int, help='localhost QMP port for an emulated keyboard event')
    parser.add_argument('--terminal-probes', action='store_true',
                        help='answer serial cursor/status probes for a 24-row, 80-column terminal')
    parser.add_argument(
        "--expect",
        action="append",
        default=[],
        help="text that must appear in QEMU output; repeatable",
    )
    parser.add_argument("command", nargs=argparse.REMAINDER, help="QEMU command after --")
    args = parser.parse_args(argv)
    if args.qmp_port and (not 1 <= args.qmp_port <= 65535 or not args.send_key or not args.send_after or args.send_text):
        parser.error('--qmp-port requires a valid port, --send-after and --send-key, without --send-text')
    if args.command and args.command[0] == "--":
        args.command = args.command[1:]
    if not args.expect:
        args.expect = ["NextBoot v"]
    return args


def main(argv: list[str]) -> int:
    return run_smoke(parse_args(argv))


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
