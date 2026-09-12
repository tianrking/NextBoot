#!/usr/bin/env python3
"""Update or roll back files on ESP/DATA volumes already verified by the host frontend."""
import argparse
from pathlib import Path
import sys

import media_update
from media_update_io import EFI_MACHINES, validate_efi
from runtime_assets import DEFAULT_DIRECTORY, release_files


def payload_for(loaders, runtime_directory):
    payload = []
    for specification in loaders:
        name, separator, filename = specification.partition('=')
        if not separator or name not in EFI_MACHINES:
            raise ValueError('loader must be BOOTX64.EFI=PATH, BOOTIA32.EFI=PATH or BOOTAA64.EFI=PATH')
        content = Path(filename).read_bytes()
        validate_efi(name, content)
        payload.append(('esp', 'EFI/BOOT/' + name, content))
    if runtime_directory is not None:
        payload.extend(('data', name.removeprefix('/'), content) for name, content in release_files(runtime_directory))
    if not payload:
        raise ValueError('no loader or runtime update was selected')
    return payload


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--esp', type=Path, required=True, help='verified mounted ESP root')
    parser.add_argument('--data', type=Path, help='verified mounted NEXTDATA root')
    parser.add_argument('--loader', action='append', default=[], help='fallback filename=local EFI artifact; repeatable')
    parser.add_argument('--runtime-dir', type=Path, default=DEFAULT_DIRECTORY, help='already cached, checksum-pinned runtime')
    parser.add_argument('--loaders-only', action='store_true', help='explicitly omit runtime migration')
    parser.add_argument('--dry-run', action='store_true', help='inspect and list changed files without writing')
    parser.add_argument('--rollback', nargs='?', const='pending', help='recover interrupted update, or restore a transaction ID')
    args = parser.parse_args()
    try:
        if args.rollback:
            if args.loader or args.dry_run:
                raise ValueError('rollback cannot be combined with loaders or dry-run')
            identifier = media_update.rollback(args.esp, args.data, None if args.rollback == 'pending' else args.rollback)
            print(f'Original files restored. Transaction: {identifier}')
        else:
            if not args.loaders_only and args.data is None:
                raise ValueError('runtime migration requires the verified DATA volume; use --loaders-only explicitly to omit it')
            payload = payload_for(args.loader, None if args.loaders_only else args.runtime_dir)
            result = media_update.apply(args.esp, args.data, payload, args.dry_run)
            if args.dry_run:
                for entry in result:
                    print(f'{entry["area"]}: {entry["path"]} {entry["old_sha256"] or "new"} -> {entry["new_sha256"]}')
                print(f'Dry run: {len(result)} file(s) would change; nothing was written.')
            elif result is None:
                print('Already up to date. No file replacement was needed.')
            else:
                print(f'Update verified. Backups retained. Transaction: {result}')
    except (OSError, ValueError, RuntimeError, KeyError, TypeError) as error:
        print(f'Media update failed: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
