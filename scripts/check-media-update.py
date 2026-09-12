#!/usr/bin/env python3
"""Exercise preservation, interrupted updates, rollback and untrusted journal paths."""
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import media_update as update
from media_update_io import ADMIN, roots_for, update_lock, validate_efi


class UpdateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='nextboot-update-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.esp, self.data = self.root / 'esp', self.root / 'data'
        self.esp.mkdir()
        self.data.mkdir()
        self.loader = self.esp / 'EFI/BOOT/BOOTX64.EFI'
        self.loader.parent.mkdir(parents=True)
        self.loader.write_bytes(b'old loader')
        (self.data / 'ISO').mkdir()
        (self.data / 'ISO/user.iso').write_bytes(b'large user image represented here')
        (self.data / 'ventoy').mkdir()
        (self.data / 'ventoy/ventoy.json').write_bytes(b'user configuration')
        self.payload = [('data', 'ventoy/ventoy.cpio', b'new runtime'),
                        ('esp', 'EFI/BOOT/BOOTX64.EFI', b'new loader')]
        self.pending = self.esp / ADMIN / 'pending.json'

    def apply(self, **kwargs):
        return update.apply(self.esp, self.data, self.payload, **kwargs)

    def assert_users_untouched(self):
        self.assertEqual((self.data / 'ISO/user.iso').read_bytes(), b'large user image represented here')
        self.assertEqual((self.data / 'ventoy/ventoy.json').read_bytes(), b'user configuration')

    def test_success_noop_and_explicit_rollback(self):
        identifier = self.apply()
        self.assertEqual(self.loader.read_bytes(), b'new loader')
        self.assertFalse(self.pending.exists())
        self.assertIsNone(self.apply())
        update.rollback(self.esp, self.data, identifier)
        self.assertEqual(self.loader.read_bytes(), b'old loader')
        self.assertFalse((self.data / 'ventoy/ventoy.cpio').exists())
        update.rollback(self.esp, self.data, identifier)  # idempotent recovery
        self.assert_users_untouched()

    def test_dry_run_has_no_writes(self):
        before = sorted(str(p.relative_to(self.root)) for p in self.root.rglob('*'))
        entries = self.apply(dry_run=True)
        self.assertEqual(len(entries), 2)
        self.assertEqual(before, sorted(str(p.relative_to(self.root)) for p in self.root.rglob('*')))
        self.assertEqual(self.loader.read_bytes(), b'old loader')

    def test_existing_runtime_is_backed_up_and_restored(self):
        runtime = self.data / 'ventoy/ventoy.cpio'
        runtime.write_bytes(b'previous runtime')
        identifier = self.apply()
        self.assertEqual(runtime.read_bytes(), b'new runtime')
        update.rollback(self.esp, self.data, identifier)
        self.assertEqual(runtime.read_bytes(), b'previous runtime')
        self.assertEqual(self.loader.read_bytes(), b'old loader')
        self.assert_users_untouched()

    def interrupt_after_first_replacement(self):
        replace = os.replace
        def crash(source, destination):
            if Path(destination) == self.loader:
                raise KeyboardInterrupt('simulated process termination')
            replace(source, destination)
        with patch.object(update.os, 'replace', side_effect=crash), self.assertRaises(KeyboardInterrupt):
            self.apply()
        self.assertTrue(self.pending.exists())
        self.assertEqual((self.data / 'ventoy/ventoy.cpio').read_bytes(), b'new runtime')

    def test_process_interruption_requires_recovery_then_resumes(self):
        self.interrupt_after_first_replacement()
        with self.assertRaisesRegex(ValueError, 'unfinished update'):
            self.apply()
        update.rollback(self.esp, self.data)
        self.assertFalse(self.pending.exists())
        self.assertFalse((self.data / 'ventoy/ventoy.cpio').exists())
        self.assertEqual(self.loader.read_bytes(), b'old loader')
        self.assertIsNotNone(self.apply())
        self.assert_users_untouched()

    def test_runtime_is_installed_before_any_loader_replacement(self):
        self.payload.reverse()  # frontend input order must not expose a new loader first
        self.interrupt_after_first_replacement()
        self.assertEqual(self.loader.read_bytes(), b'old loader')
        update.rollback(self.esp, self.data)
        self.assert_users_untouched()

    def test_write_failure_rolls_back_every_changed_volume(self):
        replace = os.replace
        def fail(source, destination):
            if Path(destination) == self.loader:
                raise OSError('simulated write failure')
            replace(source, destination)
        with patch.object(update.os, 'replace', side_effect=fail), self.assertRaisesRegex(RuntimeError, 'original files were restored'):
            self.apply()
        self.assertFalse(self.pending.exists())
        self.assertFalse((self.data / 'ventoy/ventoy.cpio').exists())
        self.assertEqual(self.loader.read_bytes(), b'old loader')
        self.assert_users_untouched()

    def test_failed_rollback_retains_recoverable_journal(self):
        finish, write = update.finish, update.durable_write
        def fail_commit(roots, journal, status):
            if status == 'committed':
                raise OSError('commit record write failed')
            return finish(roots, journal, status)
        def fail_restore(path, content):
            if path == self.loader:
                raise OSError('volume temporarily unavailable during rollback')
            return write(path, content)
        with patch.object(update, 'finish', side_effect=fail_commit), patch.object(update, 'durable_write', side_effect=fail_restore):
            with self.assertRaisesRegex(RuntimeError, 'recovery required'):
                self.apply()
        self.assertTrue(self.pending.exists())
        update.rollback(self.esp, self.data)
        self.assertEqual(self.loader.read_bytes(), b'old loader')
        self.assertFalse((self.data / 'ventoy/ventoy.cpio').exists())
        self.assertFalse(self.pending.exists())
        self.assert_users_untouched()

    def test_rollback_refuses_external_modification_before_any_writes(self):
        identifier = self.apply()
        self.loader.write_bytes(b'external edit')
        with self.assertRaisesRegex(ValueError, 'changed outside'):
            update.rollback(self.esp, self.data, identifier)
        self.assertEqual((self.data / 'ventoy/ventoy.cpio').read_bytes(), b'new runtime')
        self.assertEqual(self.loader.read_bytes(), b'external edit')

    def test_bad_backup_refused_before_any_writes(self):
        identifier = self.apply()
        (self.esp / ADMIN / identifier / 'old/1').write_bytes(b'corrupt')
        with self.assertRaisesRegex(ValueError, 'backup checksum'):
            update.rollback(self.esp, self.data, identifier)
        self.assertEqual((self.data / 'ventoy/ventoy.cpio').read_bytes(), b'new runtime')
        self.assertEqual(self.loader.read_bytes(), b'new loader')

    def test_wrong_data_volume_and_missing_backup_fail_closed(self):
        identifier = self.apply()
        other = self.root / 'other-data'
        other.mkdir()
        with self.assertRaises(OSError):
            update.rollback(self.esp, other, identifier)
        self.assertEqual(self.loader.read_bytes(), b'new loader')

    def test_journal_cannot_overwrite_iso_or_config(self):
        identifier = self.apply()
        manifest = self.esp / ADMIN / identifier / 'manifest.json'
        original = json.loads(manifest.read_text())
        for path in ('ISO/user.iso', 'ventoy/ventoy.json', '../outside', 'ventoy/../ISO/user.iso'):
            journal = dict(original)
            journal['entries'] = [dict(original['entries'][0], path=path)]
            manifest.write_text(json.dumps(journal))
            with self.assertRaisesRegex(ValueError, 'unowned'):
                update.rollback(self.esp, self.data, identifier)
        self.assert_users_untouched()

    def test_duplicate_and_unowned_inputs_are_rejected_before_writes(self):
        for payload in (self.payload * 2, [('data', 'ISO/user.iso', b'bad')], [('data', 'ventoy/ventoy.json', b'bad')]):
            with self.assertRaises(ValueError):
                update.apply(self.esp, self.data, payload)
            self.assertFalse((self.esp / ADMIN).exists())
        self.assert_users_untouched()

    def test_overlapping_roots_rejected(self):
        for root in (self.esp, self.esp / 'EFI'):
            with self.assertRaisesRegex(ValueError, 'non-overlapping'):
                roots_for(self.esp, root)

    def test_second_updater_cannot_acquire_lock(self):
        with update_lock(self.esp):
            with self.assertRaisesRegex(ValueError, 'already running'):
                self.apply()
        self.assertIsNotNone(self.apply())

    def test_out_of_space_is_detected_before_replacement(self):
        usage = type('Usage', (), {'free': 0})()
        with patch.object(update.shutil, 'disk_usage', return_value=usage), self.assertRaisesRegex(ValueError, 'free space'):
            self.apply()
        self.assertEqual(self.loader.read_bytes(), b'old loader')
        self.assertFalse(self.pending.exists())

    def test_loader_only_transaction(self):
        identifier = update.apply(self.esp, None, [self.payload[1]])
        update.rollback(self.esp, None, identifier)
        self.assertEqual(self.loader.read_bytes(), b'old loader')
        self.assert_users_untouched()

    @unittest.skipIf(os.name == 'nt', 'symlink creation requires privileges on some Windows hosts')
    def test_linked_target_rejected(self):
        self.loader.unlink()
        outside = self.root / 'outside'
        outside.write_bytes(b'private')
        self.loader.symlink_to(outside)
        with self.assertRaisesRegex(ValueError, 'linked'):
            self.apply()
        self.assertEqual(outside.read_bytes(), b'private')


class LoaderTests(unittest.TestCase):
    def test_reject_non_efi_and_wrong_architecture(self):
        with self.assertRaises(ValueError):
            validate_efi('BOOTX64.EFI', b'not an EFI application')
        payload = bytearray(512)
        payload[:2], payload[60:64] = b'MZ', (64).to_bytes(4, 'little')
        payload[64:68] = b'PE\0\0'
        payload[68:70] = (0x8664).to_bytes(2, 'little')
        payload[84:86] = (240).to_bytes(2, 'little')
        payload[88:90] = (0x20b).to_bytes(2, 'little')
        payload[156:158] = (10).to_bytes(2, 'little')
        validate_efi('BOOTX64.EFI', payload)
        with self.assertRaisesRegex(ValueError, 'architecture'):
            validate_efi('BOOTAA64.EFI', payload)


if __name__ == '__main__':
    unittest.main(verbosity=2)
