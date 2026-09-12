#!/usr/bin/env python3
"""Validate the host updater on a disposable loopback FAT/exFAT release image (Linux)."""
import argparse
import os
from pathlib import Path
import re
import subprocess
import tempfile

from media_partitions import linux_partitions, select_partitions
from runtime_assets import PROJECT_DIR, DEFAULT_DIRECTORY, release_files


def run(command, **kwargs):
    result = subprocess.run(command, capture_output=True, text=True, **kwargs)
    if result.returncode:
        raise AssertionError(f'{command[0]} failed:\n{result.stdout}\n{result.stderr}')
    return result.stdout.strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', type=Path, required=True)
    args = parser.parse_args()
    image = args.image.resolve()
    if (os.name != 'posix' or os.geteuid() != 0 or not image.is_file()
            or not image.is_relative_to((PROJECT_DIR / 'target').resolve())
            or image.name != 'update-volume-qa.img' or image.stat().st_size > 1024**3):
        raise ValueError('requires Linux root and disposable target/**/update-volume-qa.img, at most 1 GiB')
    directory = Path(tempfile.mkdtemp(prefix='nextboot-volume-qa-'))
    esp, data = directory / 'esp', directory / 'data'
    esp.mkdir()
    data.mkdir()
    device, mounted = None, []
    def mount(partition, path, filesystem):
        run(['mount', '-t', filesystem, partition, str(path)])
        mounted.append(path)
    def unmount_all():
        while mounted:
            run(['umount', str(mounted[-1])])
            mounted.pop()
    try:
        device = run(['losetup', '--find', '--show', '--partscan', str(image)])
        if not re.fullmatch(r'/dev/loop[0-9]+', device):
            raise ValueError('unexpected loop device')
        esp_part, data_part = select_partitions(linux_partitions(device))
        mount(esp_part, esp, 'vfat')
        mount(data_part, data, 'exfat')
        loader = esp / 'EFI/BOOT/BOOTX64.EFI'
        loader.write_bytes(b'previous EFI fixture')
        iso = data / 'ISO/keep.iso'
        iso.write_bytes(b'user ISO retained')
        (data / 'ventoy').mkdir()
        config = data / 'ventoy/ventoy.json'
        config.write_bytes(b'user config retained')
        unmount_all()
        updater = ['bash', str(PROJECT_DIR / 'scripts/update-media.sh'), '--yes']
        result = run([*updater, '--target', 'x86_64-unknown-uefi', device])
        identifier = re.search(r'Transaction: ([0-9a-f]{32})', result).group(1)
        mount(esp_part, esp, 'vfat')
        mount(data_part, data, 'exfat')
        assert loader.read_bytes() == (PROJECT_DIR / 'target/x86_64-unknown-uefi/release/nextboot-boot.efi').read_bytes()
        expected = release_files(DEFAULT_DIRECTORY)
        for name, content in expected:
            assert (data / name.removeprefix('/')).read_bytes() == content, name
        assert iso.read_bytes() == b'user ISO retained'
        assert config.read_bytes() == b'user config retained'
        unmount_all()
        run([*updater, '--rollback', identifier, device])
        mount(esp_part, esp, 'vfat')
        mount(data_part, data, 'exfat')
        assert loader.read_bytes() == b'previous EFI fixture'
        assert iso.read_bytes() == b'user ISO retained'
        assert config.read_bytes() == b'user config retained'
        for name, _ in expected:
            assert not (data / name.removeprefix('/')).exists(), name
        unmount_all()
        # Force the backend to fail after the frontend has mounted both volumes.
        # This fixture shim never reaches file replacement and is not product code.
        shim = directory / 'python-failure-fixture'
        shim.write_text('#!/usr/bin/env bash\ncase "$1" in *update-mounted-media.py) exit 9;; esac\nexec python3 "$@"\n')
        shim.chmod(0o755)
        result = subprocess.run([*updater, '--target', 'x86_64-unknown-uefi', device],
                                env={**os.environ, 'PYTHON': str(shim)}, capture_output=True, text=True)
        assert result.returncode != 0, result.stdout
        for partition in (esp_part, data_part):
            assert subprocess.run(['findmnt', '--source', partition], capture_output=True).returncode != 0
        shim.unlink()
        print('passed: Linux host updater on FAT/exFAT, 50 runtime files, ISO/config preservation, rollback and failure unmount cleanup')
    finally:
        # Never recursively remove a directory that could still contain a mount.
        unmount_all()
        if device is not None:
            run(['losetup', '--detach', device])
        for path in (esp, data, directory):
            path.rmdir()


if __name__ == '__main__':
    main()
