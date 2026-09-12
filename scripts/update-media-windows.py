#!/usr/bin/env python3
"""Native Windows NextBoot update/recovery; no formatting or drive-letter assignment."""
import argparse
import ctypes
from pathlib import Path
import subprocess
import sys

import media_update
from media_update_io import validate_efi
from runtime_assets import DEFAULT_DIRECTORY, PROJECT_DIR, prepare, release_files
from windows_media import checked_roots, inventory, select_disk

TARGETS = {'x64': ('x86_64-unknown-uefi', 'BOOTX64.EFI'),
           'ia32': ('i686-unknown-uefi', 'BOOTIA32.EFI'),
           'aa64': ('aarch64-unknown-uefi', 'BOOTAA64.EFI')}


def execute(args):
    disks = inventory()
    if args.list:
        for disk in disks:
            role = 'Windows system/boot - excluded' if disk['IsSystem'] or disk['IsBoot'] else 'inspect before updating'
            print(f'Disk {disk["Number"]}: {disk["BusType"]}, {disk["Size"] / 1024**3:.1f} GiB, {role}')
        return
    if args.disk is None or args.disk < 0:
        raise ValueError('select an explicit disk number with --disk N; use --list first')
    select = lambda: select_disk(inventory(), args.disk, args.allow_fixed, args.allow_virtual)
    selection = select_disk(disks, args.disk, args.allow_fixed, args.allow_virtual)
    if not args.dry_run and not ctypes.windll.shell32.IsUserAnAdmin():
        raise ValueError('run Windows Terminal as administrator to update or recover media')
    if args.rollback and args.dry_run:
        raise ValueError('rollback cannot be combined with --dry-run')
    roots = checked_roots(selection)
    payload = []
    if not args.rollback:
        for target in (TARGETS if args.target == 'all' else [args.target]):
            triple, name = TARGETS[target]
            content = (args.artifacts / triple / 'release/nextboot-boot.efi').read_bytes()
            validate_efi(name, content)
            payload.append(('esp', 'EFI/BOOT/' + name, content))
        if not args.loaders_only:
            # Dry-run never downloads or creates cache files. A prepared cache
            # is required for its precise changed-file preview.
            if not args.dry_run:
                prepare(args.runtime_dir, offline=args.offline)
            payload.extend(('data', path.removeprefix('/'), content)
                           for path, content in release_files(args.runtime_dir))
        changes = media_update.apply(roots['esp'], roots['data'], payload, dry_run=True)
        for change in changes:
            print(f'{change["area"]}: {change["path"]}')
        print(f'Disk {args.disk}: {len(changes)} file(s) would change.')
        if args.dry_run:
            print('Dry run complete; no files were written.')
            return
    print(f'Selected disk {args.disk}, {selection["disk"]["Size"] / 1024**3:.1f} GiB; ISO and configuration are preserved.')
    if not args.yes and input(f'Type {args.disk} to continue: ').strip() != str(args.disk):
        raise ValueError('operation cancelled')
    current = select()
    if current['identity'] != selection['identity'] or checked_roots(current) != roots:
        raise ValueError('disk or volume identity changed; inspect it again')
    if args.rollback:
        identifier = media_update.rollback(roots['esp'], roots['data'],
                                           None if args.rollback == 'pending' else args.rollback)
        print(f'Original files restored. Transaction: {identifier}')
    else:
        identifier = media_update.apply(roots['esp'], roots['data'], payload)
        print(f'Update verified. Backups retained. Transaction: {identifier}' if identifier
              else 'Already up to date; no file replacement needed.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--list', action='store_true')
    parser.add_argument('--disk', type=int)
    parser.add_argument('--target', choices=(*TARGETS, 'all'), default='x64')
    parser.add_argument('--artifacts', type=Path, default=PROJECT_DIR / 'target')
    parser.add_argument('--runtime-dir', type=Path, default=DEFAULT_DIRECTORY)
    parser.add_argument('--loaders-only', action='store_true')
    parser.add_argument('--offline', action='store_true')
    parser.add_argument('--dry-run', action='store_true')
    parser.add_argument('--rollback', nargs='?', const='pending')
    parser.add_argument('--allow-fixed', action='store_true', help='explicitly allow a non-system fixed disk')
    parser.add_argument('--allow-virtual', action='store_true', help='explicitly allow VHD testing')
    parser.add_argument('--yes', action='store_true')
    args = parser.parse_args()
    try:
        execute(args)
        return 0
    except (OSError, ValueError, RuntimeError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        print(f'Windows media update failed: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
