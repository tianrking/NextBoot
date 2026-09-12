#!/usr/bin/env python3
"""Windows administrator integration test, restricted to a newly created disposable VHD."""
import ctypes
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile

from runtime_assets import PROJECT_DIR, prepare, release_files
from windows_media import checked_roots, inventory, select_disk


def run(command, **kwargs):
    result = subprocess.run(command, capture_output=True, text=True, **kwargs)
    if result.returncode:
        raise AssertionError(f'{command[0]} failed:\n{result.stdout}\n{result.stderr}')
    return result.stdout.strip()


def powershell(script, image):
    prefix = "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); "
    return run(['powershell.exe', '-NoProfile', '-NonInteractive', '-Command', prefix + script],
               env={**os.environ, 'NEXTBOOT_TEST_VHD': str(image)}, encoding='utf-8')


def diskpart(root, commands):
    path = root / 'diskpart-fixture.txt'
    path.write_text('\n'.join(commands) + '\n', encoding='utf-8')
    print(run(['diskpart.exe', '/s', str(path)]), flush=True)


def main():
    if os.name != 'nt' or not ctypes.windll.shell32.IsUserAnAdmin():
        raise ValueError('this disposable VHD test requires Windows administrator privileges')
    target = (PROJECT_DIR / 'target').resolve()
    target.mkdir(exist_ok=True)
    # Leave the detached file as test evidence; never recursively delete a path
    # that might still be a mount or an attached device.
    root = Path(tempfile.mkdtemp(prefix='windows-volume-qa-', dir=target)).resolve()
    image = root / 'nextboot-test.vhd'
    if not image.is_relative_to(target) or image.exists():
        raise ValueError('only a new disposable target VHD may be created')
    attached = False
    try:
        diskpart(root, [f'create vdisk file="{image}" maximum=512 type=expandable', 'exit'])
        if not image.is_file():
            raise ValueError('VHD creation failed')
        powershell('Mount-DiskImage -ImagePath $env:NEXTBOOT_TEST_VHD -NoDriveLetter | Out-Null', image)
        attached = True
        disk_number = int(powershell('(Get-DiskImage -ImagePath $env:NEXTBOOT_TEST_VHD | Get-Disk).Number', image))
        disk = next(d for d in inventory() if d['Number'] == disk_number)
        if disk['BusType'] != 'File Backed Virtual' or disk['IsSystem'] or disk['IsBoot'] or disk['Style'] != 'RAW':
            raise ValueError('new VHD does not identify a blank non-system virtual disk')
        # Select the verified newly created VHD itself, never a physical disk.
        diskpart(root, [f'select vdisk file="{image}"', 'convert gpt',
                        'create partition efi size=64', 'format fs=fat32 quick label=NEXTBOOT',
                        'create partition primary', 'format fs=exfat quick label=NEXTDATA', 'exit'])
        selection = select_disk(inventory(), disk_number, allow_virtual=True)
        roots = checked_roots(selection)
        esp, data = roots['esp'], roots['data']
        loader = esp / 'EFI/BOOT/BOOTX64.EFI'
        loader.parent.mkdir(parents=True)
        loader.write_bytes(b'old EFI fixture')
        iso = data / 'ISO/keep.iso'
        iso.parent.mkdir()
        iso.write_bytes(b'keep user ISO')
        config = data / 'ventoy/ventoy.json'
        config.parent.mkdir()
        config.write_bytes(b'keep user config')
        artifacts = root / 'artifacts'
        new_loader = artifacts / 'x86_64-unknown-uefi/release/nextboot-boot.efi'
        new_loader.parent.mkdir(parents=True)
        # A non-booted PE fixture exercises file operations and machine checks.
        content = bytearray(512)
        content[:2] = b'MZ'
        struct.pack_into('<I', content, 0x3C, 0x80)
        content[0x80:0x84] = b'PE\x00\x00'
        struct.pack_into('<H', content, 0x84, 0x8664)
        struct.pack_into('<H', content, 0x94, 0xF0)
        struct.pack_into('<H', content, 0x98, 0x20B)
        struct.pack_into('<H', content, 0x98 + 68, 10)
        new_loader.write_bytes(content)
        cache = prepare(root / 'runtime')
        command = [sys.executable, str(PROJECT_DIR / 'scripts/update-media-windows.py'),
                   '--disk', str(disk_number), '--allow-virtual', '--yes', '--offline',
                   '--artifacts', str(artifacts), '--runtime-dir', str(cache)]
        preview = run([*command, '--dry-run'])
        assert '51 file(s) would change' in preview, preview
        assert not (esp / '.nextboot-update').exists()
        result = run(command)
        identifier = re.search(r'Transaction: ([0-9a-f]{32})', result).group(1)
        assert loader.read_bytes() == content
        for name, expected in release_files(cache):
            assert (data / name.removeprefix('/')).read_bytes() == expected, name
        assert 'Already up to date' in run(command)
        run([*command, '--rollback', identifier])
        assert loader.read_bytes() == b'old EFI fixture'
        assert iso.read_bytes() == b'keep user ISO'
        assert config.read_bytes() == b'keep user config'
        for name, _ in release_files(cache):
            assert not (data / name.removeprefix('/')).exists(), name
        # A clean detach/remount checks persistence beyond open filesystem caches.
        powershell('Dismount-DiskImage -ImagePath $env:NEXTBOOT_TEST_VHD', image)
        attached = False
        powershell('Mount-DiskImage -ImagePath $env:NEXTBOOT_TEST_VHD -NoDriveLetter | Out-Null', image)
        attached = True
        disk_number = int(powershell('(Get-DiskImage -ImagePath $env:NEXTBOOT_TEST_VHD | Get-Disk).Number', image))
        roots = checked_roots(select_disk(inventory(), disk_number, allow_virtual=True))
        assert (roots['esp'] / 'EFI/BOOT/BOOTX64.EFI').read_bytes() == b'old EFI fixture'
        assert (roots['data'] / 'ISO/keep.iso').read_bytes() == b'keep user ISO'
        assert (roots['data'] / 'ventoy/ventoy.json').read_bytes() == b'keep user config'
        print('passed: native Windows VHD selection, dry-run, runtime update, no-op, rollback and remount preservation')
    finally:
        if image.is_file():
            powershell('$vhd=Get-DiskImage -ImagePath $env:NEXTBOOT_TEST_VHD; '
                       'if ($vhd.Attached) { Dismount-DiskImage -ImagePath $env:NEXTBOOT_TEST_VHD }', image)


if __name__ == '__main__':
    main()
