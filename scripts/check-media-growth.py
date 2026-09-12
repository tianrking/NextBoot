#!/usr/bin/env python3
"""Check no-op growth and rejected shrink attempts against a real generated image."""
import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

PROJECT_DIR = Path(__file__).resolve().parents[1]
GROW = PROJECT_DIR / 'scripts/grow-release-media.py'


def digest(path):
    with path.open('rb') as data:
        return hashlib.file_digest(data, 'sha256').hexdigest()


class GrowthTests(unittest.TestCase):
    def test_idempotence_and_no_shrink(self):
        with tempfile.TemporaryDirectory(prefix='nextboot-grow-') as folder:
            root = Path(folder)
            efi = root / 'fixture.efi'
            efi.write_bytes(b'growth fixture, not executable')
            image = root / 'disk.img'
            env = os.environ.copy()
            env.update(NEXTBOOT_GROWABLE_EXFAT='1', NEXTBOOT_GROWABLE_EXFAT_MAX_MIB='256',
                       NEXTBOOT_VENTOY_ASSETS_DIR='')
            subprocess.run([sys.executable, str(PROJECT_DIR / 'scripts/qemu/create-disk-image.py'),
                            str(image), '64', '512', 'split', 'exfat', str(efi),
                            '0', '0', '0', '', '', '0', '0', 'BOOTX64.EFI', ''],
                           env=env, check=True, capture_output=True)
            original = digest(image)
            command = [sys.executable, str(GROW), '--disk-image', str(image)]
            noop = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(noop.returncode, 0, noop.stdout + noop.stderr)
            self.assertIn('no changes made', noop.stdout)
            self.assertEqual(digest(image), original)
            for options in [('--target-size-mib', '32'), ('--media-size-bytes', str(32*1024*1024))]:
                result = subprocess.run([*command, *options], capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(image.stat().st_size, 64*1024*1024)
                self.assertEqual(digest(image), original)
            expanded = subprocess.run([*command, '--target-size-mib', '128'], capture_output=True, text=True)
            self.assertEqual(expanded.returncode, 0, expanded.stdout + expanded.stderr)
            after = digest(image)
            again = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(again.returncode, 0, again.stdout + again.stderr)
            self.assertEqual(digest(image), after)


if __name__ == '__main__':
    unittest.main(verbosity=2)
