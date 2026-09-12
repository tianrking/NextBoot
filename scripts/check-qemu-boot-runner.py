#!/usr/bin/env python3
"""Exercise streamed markers, input, EOF, and timeout on every host platform."""
from pathlib import Path
import subprocess
import sys
import unittest

RUNNER = Path(__file__).with_name('qemu-boot-smoke.py')


class RunnerTests(unittest.TestCase):
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
