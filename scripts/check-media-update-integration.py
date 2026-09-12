#!/usr/bin/env python3
"""Migrate old-media fixtures using the actual loader and pinned runtime, then roll back."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile

from media_update_io import ADMIN, digest
from runtime_assets import PROJECT_DIR, prepare, release_files


def check(esp, data, loader, runtime):
    """Both roots must belong to a disposable test fixture, never user media."""
    target = esp / 'EFI/BOOT/BOOTX64.EFI'
    target.parent.mkdir(parents=True, exist_ok=True)
    old_loader = b'old-media loader bytes retained by backup'
    target.write_bytes(old_loader)
    (data / 'ISO').mkdir(exist_ok=True)
    iso = data / 'ISO/user.iso'
    iso.write_bytes(b'preserved user ISO' * 65536)
    (data / 'ventoy').mkdir(exist_ok=True)
    config = data / 'ventoy/ventoy.json'
    config.write_bytes(b'{"control":[{"VTOY_DEFAULT_SEARCH_ROOT":"/ISO"}]}')
    user_hashes = (digest(iso.read_bytes()), digest(config.read_bytes()))
    expected = release_files(runtime)
    # The old public media lacked these resources. This fixture starts likewise.
    for name, _ in expected:
        path = data / name.removeprefix('/')
        if path.exists():
            raise AssertionError('runtime fixture must start empty')
    command = [sys.executable, str(PROJECT_DIR / 'scripts/update-mounted-media.py'),
               '--esp', str(esp), '--data', str(data)]
    def invoke(*args):
        result = subprocess.run([*command, *args], capture_output=True, text=True)
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)
        return result.stdout
    options = ['--loader', f'BOOTX64.EFI={loader}', '--runtime-dir', str(runtime)]
    plan = invoke(*options, '--dry-run')
    assert '51 file(s) would change' in plan, plan
    assert not (esp / ADMIN).exists()
    result = invoke(*options)
    identifier = result.split('Transaction: ', 1)[1].strip()
    assert target.read_bytes() == loader.read_bytes()
    for name, content in expected:
        assert (data / name.removeprefix('/')).read_bytes() == content, name
    manifest = json.loads((esp / ADMIN / identifier / 'manifest.json').read_text())
    assert manifest['status'] == 'committed' and len(manifest['entries']) == 51
    assert 'Already up to date' in invoke(*options)
    invoke('--rollback', identifier)
    assert target.read_bytes() == old_loader
    for name, _ in expected:
        assert not (data / name.removeprefix('/')).exists(), name
    assert (digest(iso.read_bytes()), digest(config.read_bytes())) == user_hashes
    assert not (esp / ADMIN / 'pending.json').exists()
    print('passed: actual EFI + 50 pinned runtime files migrated, verified, no-op, rolled back; ISO/config unchanged')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--loader', type=Path, default=PROJECT_DIR / 'target/x86_64-unknown-uefi/release/nextboot-boot.efi')
    parser.add_argument('--offline', action='store_true')
    args = parser.parse_args()
    runtime = prepare(offline=args.offline)
    with tempfile.TemporaryDirectory(prefix='nextboot-upgrade-integration-') as directory:
        esp, data = Path(directory) / 'esp', Path(directory) / 'data'
        esp.mkdir()
        data.mkdir()
        check(esp, data, args.loader, runtime)


if __name__ == '__main__':
    main()
