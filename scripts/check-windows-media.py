#!/usr/bin/env python3
"""Selection invariants for native Windows updates, without touching a disk."""
import copy
import unittest
from windows_media import VOLUME_PATH, select_disk, ESP_GUID, BASIC_GUID

ESP = '\\\\?\\Volume{11111111-1111-1111-1111-111111111111}\\'
DATA = '\\\\?\\Volume{22222222-2222-2222-2222-222222222222}\\'


class SelectionTests(unittest.TestCase):
    def setUp(self):
        self.disk = dict(Number=7, UniqueId='disk-identity', Size=1024**3, BusType='USB', Style='GPT',
                         IsBoot=False, IsSystem=False, IsReadOnly=False, IsOffline=False,
                         Partitions=[dict(Number=1, Guid='esp-guid', Type='{'+ESP_GUID+'}', Offset=1024,
                                          Size=64*1024**2, IsBoot=False, IsSystem=False, AccessPaths=[ESP]),
                                     dict(Number=2, Guid='data-guid', Type='{'+BASIC_GUID+'}', Offset=65*1024**2,
                                          Size=900*1024**2, IsBoot=False, IsSystem=False, AccessPaths=[DATA, 'F:\\'])])
        self.volumes = {ESP: dict(label='NEXTBOOT', filesystem='FAT', serial=11),
                        DATA: dict(label='NEXTDATA', filesystem='exFAT', serial=22)}

    def select(self, **options):
        return select_disk([self.disk], 7, inspect_volume=self.volumes.__getitem__, **options)

    def test_partition_order_and_drive_letters_do_not_identify_media(self):
        first = self.select()
        self.disk['Partitions'].reverse()
        self.disk['Partitions'][0]['AccessPaths'] = [DATA, 'G:\\']
        self.assertEqual(self.select()['identity'], first['identity'])
        self.assertEqual(first['esp'], ESP)
        self.assertEqual(first['data'], DATA)

    def test_system_disk_and_partition_refused_even_with_overrides(self):
        for subject, key in [(self.disk, 'IsSystem'), (self.disk, 'IsBoot'),
                             (self.disk['Partitions'][0], 'IsSystem'), (self.disk['Partitions'][1], 'IsBoot')]:
            subject[key] = True
            with self.assertRaisesRegex(ValueError, 'system or boot'):
                self.select(allow_fixed=True, allow_virtual=True)
            subject[key] = False

    def test_fixed_and_virtual_are_separate_explicit_opt_ins(self):
        self.disk['BusType'] = 'NVMe'
        with self.assertRaises(ValueError):
            self.select()
        self.select(allow_fixed=True)
        self.disk['BusType'] = 'File Backed Virtual'
        with self.assertRaises(ValueError):
            self.select(allow_fixed=True)
        self.select(allow_virtual=True)

    def test_offline_readonly_non_gpt_and_missing_identity_rejected(self):
        for key, value in [('IsOffline', True), ('IsReadOnly', True), ('Style', 'MBR'), ('UniqueId', '')]:
            original = self.disk[key]
            self.disk[key] = value
            with self.assertRaises(ValueError):
                self.select()
            self.disk[key] = original

    def test_missing_or_duplicate_partitions_rejected(self):
        original = copy.deepcopy(self.disk['Partitions'])
        for parts in [original[:1], original[1:], original + original[:1], original + original[1:]]:
            self.disk['Partitions'] = parts
            with self.assertRaises(ValueError):
                self.select()

    def test_unknown_filesystems_and_wrong_data_label_rejected(self):
        for path, key, value in [(ESP, 'filesystem', 'NTFS'), (DATA, 'filesystem', 'NTFS'),
                                 (DATA, 'label', 'Other')]:
            original = self.volumes[path][key]
            self.volumes[path][key] = value
            with self.assertRaises(ValueError):
                self.select()
            self.volumes[path][key] = original

    def test_drive_letter_alone_is_not_a_volume_identity(self):
        self.disk['Partitions'][0]['AccessPaths'] = ['E:\\']
        with self.assertRaises(ValueError):
            self.select()

    def test_hotplug_or_changed_geometry_changes_identity(self):
        initial = self.select()['identity']
        self.disk['UniqueId'] = 'replacement'
        self.assertNotEqual(initial, self.select()['identity'])
        self.disk['UniqueId'] = 'disk-identity'
        self.disk['Partitions'][1]['Size'] -= 512
        self.assertNotEqual(initial, self.select()['identity'])

    def test_volume_guid_roots_are_required(self):
        self.assertTrue(VOLUME_PATH.fullmatch(ESP))
        self.assertFalse(VOLUME_PATH.fullmatch('E:\\'))


if __name__ == '__main__':
    unittest.main(verbosity=2)
