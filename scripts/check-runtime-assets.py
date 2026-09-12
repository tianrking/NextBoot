#!/usr/bin/env python3
"""Check runtime integrity and offline failure behavior without network access."""
import hashlib
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import runtime_assets as runtime


class RuntimeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / 'source'
        self.source.mkdir()
        self.cache = self.root / 'cache'
        self.data = b'pinned runtime fixture\n'
        self.entry = {'path': 'helper.bin', 'source_path': 'INSTALL/ventoy/helper.bin',
                      'size': len(self.data), 'sha256': hashlib.sha256(self.data).hexdigest()}
        self.config = {'commit': '0' * 40, 'files': [self.entry]}
        self.mock = patch.object(runtime, 'manifest', return_value=self.config)
        self.mock.start()
        self.addCleanup(self.mock.stop)

    def test_import_and_offline_reuse(self):
        (self.source / 'helper.bin').write_bytes(self.data)
        runtime.prepare(self.cache, self.source)
        with patch.object(runtime.urllib.request, 'urlopen', side_effect=AssertionError('unexpected network')):
            runtime.prepare(self.cache, offline=True)
        self.assertEqual((self.cache / 'helper.bin').read_bytes(), self.data)

    def test_corrupt_cache_is_not_silently_accepted(self):
        self.cache.mkdir()
        (self.cache / 'helper.bin').write_bytes(b'corrupt')
        with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
            runtime.prepare(self.cache, offline=True)

    def test_missing_offline_asset_fails(self):
        with self.assertRaisesRegex(ValueError, 'missing pinned'):
            runtime.prepare(self.cache, offline=True)

    def test_wrong_source_does_not_publish_file(self):
        (self.source / 'helper.bin').write_bytes(b'wrong version')
        with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
            runtime.prepare(self.cache, self.source)
        self.assertFalse((self.cache / 'helper.bin').exists())


if __name__ == '__main__':
    unittest.main(verbosity=2)
