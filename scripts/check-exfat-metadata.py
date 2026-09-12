#!/usr/bin/env python3
"""Regressions for host-compatible exFAT metadata and Unicode file names."""
import sys
from pathlib import Path
import struct
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent / 'qemu'))
from disk_image.exfat import exfat_entry_set, exfat_name_hash, exfat_upcase_table
from qemu_verify.common import VerifyError
from qemu_verify.exfat_metadata import checksum, decode_upcase, name_hash


class MetadataTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.encoded = exfat_upcase_table()
        cls.table = decode_upcase(cls.encoded, checksum(cls.encoded, 32))

    def test_compressed_table_interoperability(self):
        self.assertLess(len(self.encoded), 65536)
        self.assertEqual(len(self.table), 65536)
        for unit, expected in [(0, 0), (97, 65), (0xDF, 0xDF), (0xE9, 0xC9),
                               (0xD801, 0xD801), (0xDC28, 0xDC28), (0xFFFF, 0xFFFF)]:
            self.assertEqual(self.table[unit], expected)

    def test_name_hash_uses_volume_mappings(self):
        for name in ['boot.iso', 'straße.iso', 'é中文.iso', '\U00010428.iso', '\U0001f600.iso']:
            with self.subTest(name=name):
                self.assertEqual(exfat_name_hash(name), name_hash(name.encode('utf-16le'), self.table))
        self.assertNotEqual(exfat_name_hash('ß'), exfat_name_hash('SS'))
        self.assertEqual(exfat_name_hash('é'), exfat_name_hash('É'))

    def test_checksum_damage_rejected(self):
        corrupted = bytearray(self.encoded)
        corrupted[4] ^= 1
        with self.assertRaisesRegex(VerifyError, 'checksum'):
            decode_upcase(corrupted, checksum(self.encoded, 32))

    def test_invalid_identity_runs_rejected(self):
        for raw in [b'\xff\xff\x00\x00', struct.pack('<HHHH', 0xFFFF, 65535, 0xFFFF, 2),
                    self.encoded[:-2], self.encoded + b'\x00\x00']:
            with self.subTest(raw_size=len(raw)), self.assertRaises(VerifyError):
                decode_upcase(raw, checksum(raw, 32))

    def test_stream_flags_and_dates(self):
        for size, contiguous, expected in [(0, True, 1), (512, True, 3), (512, False, 1)]:
            group = exfat_entry_set('keep.iso', 0x20, 7 if size else 0, size, contiguous)
            self.assertEqual(group[33], expected)
            self.assertEqual(struct.unpack_from('<H', group, 2)[0], checksum(group, 16, (2, 3)))
            for offset in (10, 14, 18):
                self.assertEqual(struct.unpack_from('<H', group, offset)[0], 0x21)


if __name__ == '__main__':
    unittest.main(verbosity=2)
