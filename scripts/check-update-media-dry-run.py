#!/usr/bin/env python3
"""Check partition discovery and updater plans without accessing a real disk."""
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import media_partitions as media

PROJECT_DIR = Path(__file__).resolve().parents[1]


def partition(path, esp=False, label=None):
    return media.Partition(path, media.ESP_GUID if esp else media.BASIC_GUID,
                           label if label is not None else ('VTOYEFI' if esp else 'NEXTDATA'),
                           'vfat' if esp else 'exfat')


class PartitionTests(unittest.TestCase):
    def test_both_layouts(self):
        for number in (1, 2):
            esp, data = f'/dev/sdz{number}', f'/dev/sdz{3-number}'
            self.assertEqual(media.select_partitions([partition(data), partition(esp, True)]), (esp, data))

    def test_unsafe_layouts_rejected_even_with_force(self):
        cases = [
            [], [partition('/dev/sdz1')],
            [partition('/dev/sdz1', True), partition('/dev/sdz2', True)],
            [partition('/dev/sdz1', True, 'NEXTDATA')],
            [partition('/dev/sdz1', True, 'SYSTEM')],
            [partition('/dev/sdz1'), partition('/dev/sdz2'), partition('/dev/sdz3', True)],
            [media.Partition('/dev/sdz2', media.ESP_GUID, 'VTOYEFI', 'exfat'), partition('/dev/sdz1')],
        ]
        for parts in cases:
            with self.subTest(parts=parts), self.assertRaises(ValueError):
                media.select_partitions(parts, True)

    def test_missing_data_needs_force_and_known_esp(self):
        with self.assertRaisesRegex(ValueError, 'NEXTDATA was not detected'):
            media.select_partitions([partition('/dev/sdz2', True)])
        self.assertEqual(media.select_partitions([partition('/dev/sdz2', True)], True), ('/dev/sdz2', '-'))

    def test_linux_inventory(self):
        payload = {'blockdevices': [{'name': '/dev/nvme2n1', 'type': 'disk', 'children': [
            {'name': '/dev/nvme2n1p1', 'type': 'part', 'parttype': media.BASIC_GUID, 'label': 'NEXTDATA', 'fstype': 'exfat'},
            {'name': '/dev/nvme2n1p2', 'type': 'part', 'parttype': media.ESP_GUID.upper(), 'label': 'VTOYEFI', 'fstype': 'vfat'},
        ]}]}
        with patch.object(media, 'command', return_value=json.dumps(payload).encode()):
            self.assertEqual(media.select_partitions(media.linux_partitions('/dev/nvme2n1')),
                             ('/dev/nvme2n1p2', '/dev/nvme2n1p1'))

    def test_linux_rejects_foreign_child_and_partition_target(self):
        for item in [{'name': '/dev/sdz', 'type': 'disk', 'children': [{'name': '/dev/sda1', 'type': 'part'}]},
                     {'name': '/dev/sdz', 'type': 'part'},
                     {'name': '/dev/sda', 'type': 'disk'}]:
            with self.subTest(item=item), patch.object(media, 'command', return_value=json.dumps({'blockdevices':[item]}).encode()):
                with self.assertRaises(ValueError):
                    media.linux_partitions('/dev/sdz')

    def test_macos_inventory_and_identity_change(self):
        responses = [
            {'Whole': True, 'DeviceIdentifier': 'disk9'},
            {'AllDisksAndPartitions': [{'DeviceIdentifier': 'disk9', 'Partitions': [
                {'DeviceIdentifier': 'disk9s1'}, {'DeviceIdentifier': 'disk9s2'}]}]},
            {'DeviceIdentifier': 'disk9s1', 'ParentWholeDisk': 'disk9', 'Content': 'Microsoft Basic Data',
             'VolumeName': 'NEXTDATA', 'FilesystemType': 'exfat'},
            {'DeviceIdentifier': 'disk9s2', 'ParentWholeDisk': 'disk9', 'Content': 'EFI',
             'VolumeName': 'VTOYEFI', 'FilesystemType': 'msdos'},
        ]
        with patch.object(media, 'command', side_effect=[plistlib.dumps(r) for r in responses]):
            self.assertEqual(media.select_partitions(media.macos_partitions('/dev/disk9')),
                             ('/dev/disk9s2', '/dev/disk9s1'))
        responses[2]['ParentWholeDisk'] = 'disk8'
        with patch.object(media, 'command', side_effect=[plistlib.dumps(r) for r in responses]):
            with self.assertRaisesRegex(ValueError, 'identity changed'):
                media.macos_partitions('/dev/disk9')


class ShellPlanTests(unittest.TestCase):
    """Test the shell using inventories already checked by the parser tests."""
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='nextboot-update-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'scripts/lib').mkdir(parents=True)
        for name in ('update-media.sh', 'lib/flash_helpers.sh', 'lib/flash_targets.sh'):
            shutil.copyfile(PROJECT_DIR / 'scripts' / name, self.root / 'scripts' / name)
        for target in ('x86_64-unknown-uefi', 'i686-unknown-uefi', 'aarch64-unknown-uefi'):
            artifact = self.root / 'target' / target / 'debug/nextboot-boot.efi'
            artifact.parent.mkdir(parents=True)
            artifact.write_bytes(b'isolated dry-run fixture')
        self.inspector = self.root / 'inspect-fixture'
        git_bash = Path('C:/Program Files/Git/bin/bash.exe')
        self.bash = str(git_bash) if os.name == 'nt' and git_bash.exists() else shutil.which('bash')
        self.assertIsNotNone(self.bash, 'bash is required')

    def plan(self, host, device, parts, inspect_fail=False):
        if inspect_fail:
            body = 'exit 1\n'
        else:
            esp, data = media.select_partitions(parts)
            body = f"printf '%s\\t%s\\n' '{esp}' '{data}'\n"
        self.inspector.write_text('#!/usr/bin/env bash\n' + body, encoding='utf-8', newline='\n')
        self.inspector.chmod(0o755)
        env = os.environ.copy()
        env.update(NEXTBOOT_OSTYPE=host, PYTHON=self.inspector.as_posix())
        return subprocess.run([self.bash, (self.root / 'scripts/update-media.sh').as_posix(),
                               '--dry-run', '--target', 'all', device], env=env, capture_output=True, text=True)

    def test_release_and_legacy_linux(self):
        for number in (1, 2):
            with self.subTest(esp=number):
                result = self.plan('linux', '/dev/sdz', [partition(f'/dev/sdz{number}', True), partition(f'/dev/sdz{3-number}')])
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn(f'+ sudo mount /dev/sdz{number} ', result.stdout)
                self.assertNotIn(f'+ sudo mount /dev/sdz{3-number} ', result.stdout)
                self.assertNotIn('mkfs', result.stdout)

    def test_macos_release(self):
        result = self.plan('darwin', '/dev/rdisk9', [partition('/dev/disk9s1'), partition('/dev/disk9s2', True)])
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('+ diskutil mount /dev/disk9s2', result.stdout)
        self.assertNotIn('+ diskutil mount /dev/disk9s1', result.stdout)

    def test_failed_inspection_stops_before_mount(self):
        result = self.plan('linux', '/dev/sdz', [], True)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('+ sudo mount', result.stdout)
        self.assertIn('nothing was written', result.stderr)

    def test_windows_explicitly_rejected(self):
        result = self.plan('msys', '/dev/sdz', [], True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Windows support is not available', result.stderr)


if __name__ == '__main__':
    unittest.main(verbosity=2)
