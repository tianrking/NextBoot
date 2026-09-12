#!/usr/bin/env python3
"""Exercise streamed markers, input, EOF, and timeout on every host platform."""
from pathlib import Path
import subprocess
import sys
import unittest
import io
import json
import runpy
from unittest.mock import MagicMock, patch
from terminal_probe import TerminalProbe

RUNNER = Path(__file__).with_name('qemu-boot-smoke.py')


class RunnerTests(unittest.TestCase):
    def test_fragmented_terminal_probe_and_size(self):
        probe = TerminalProbe()
        self.assertEqual(probe.feed(b'\x1b[1;1H\x1b['), b'')
        self.assertEqual(probe.feed(b'6n'), b'\x1b[1;1R')
        self.assertEqual(probe.feed(b'\x1b[32766;32766H\x1b[6n'), b'\x1b[24;80R')
        self.assertEqual(probe.feed(b'\x1b[?25h\x1b[5n'), b'\x1b[0n')

    def test_terminal_probe_handshake(self):
        result = self.run_case("import sys; sys.stdout.buffer.write(b'\\x1b[32766;32766H\\x1b[6n'); sys.stdout.flush(); x=sys.stdin.buffer.read(8); print('PAYLOAD_STARTED' if x==b'\\x1b[24;80R' else 'BAD')",
                               ('--terminal-probes',))
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_qmp_keyboard_command(self):
        scope = runpy.run_path(str(RUNNER))
        client = MagicMock()
        client.__enter__.return_value = client
        client.makefile.return_value = io.BytesIO(b'{"QMP":{}}\n{"return":{},"id":0}\n{"return":{},"id":1}\n')
        with patch.object(scope['socket'], 'create_connection', return_value=client):
            scope['send_qmp_key'](12345, 'enter')
        commands = [json.loads(call.args[0]) for call in client.sendall.call_args_list]
        self.assertEqual(commands[1]['execute'], 'send-key')
        self.assertEqual(commands[1]['arguments']['keys'], [{'type': 'qcode', 'data': 'ret'}])

    def run_case(self, script, extra=()):
        return subprocess.run([sys.executable, str(RUNNER), '--timeout', '2', '--expect', 'PAYLOAD_STARTED',
                               *extra, '--', sys.executable, '-u', '-c', script],
                              capture_output=True, text=True, timeout=8)

    def test_fragmented_marker(self):
        result = self.run_case("import sys,time; sys.stdout.write('PAYLOAD_'); sys.stdout.flush(); time.sleep(.05); print('STARTED')")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_input_handshake(self):
        result = self.run_case("import sys; print('READY'); x=sys.stdin.buffer.read(1); print('PAYLOAD_STARTED' if x==b'\\r' else 'BAD')",
                               ('--send-after', 'READY', '--send-key', 'enter', '--send-delay', '0'))
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_eof_without_marker_fails(self):
        result = self.run_case("print('not the payload')")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('missing: PAYLOAD_STARTED', result.stderr)

    def test_timeout_terminates_child(self):
        result = self.run_case("import time; time.sleep(60)", ('--timeout', '.2'))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('QEMU boot smoke failed', result.stderr)


if __name__ == '__main__':
    unittest.main(verbosity=2)
