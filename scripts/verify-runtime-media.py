#!/usr/bin/env python3
"""Verify every runtime resource and notice in the actual release disk image."""
import argparse
from pathlib import Path
import sys
from qemu_verify.common import DiskImage, VerifyError, parse_gpt
from qemu_verify.exfat import ExFatVolume
from qemu_verify.fat32 import Fat32Volume
from runtime_assets import DEFAULT_DIRECTORY, release_files


def verify_media(path: str, assets: Path, sector_size: int = 512, data_fs: str = 'exfat') -> int:
    image = DiskImage(path, sector_size)
    try:
        partitions = [p for p in parse_gpt(image) if p.name == 'NEXBOOT_DATA']
        if len(partitions) != 1:
            raise ValueError('expected exactly one release data partition')
        volume = (ExFatVolume if data_fs == 'exfat' else Fat32Volume)(image, partitions[0])
        files = release_files(assets)
        for name, expected in files:
            record = volume.lookup(name)
            if record.is_dir or record.size != len(expected):
                raise ValueError(f'runtime file size mismatch: {name}')
            actual = b''.join(image.read_blocks(e.physical_lba, e.block_count)
                              for e in volume.file_extents(record))[:record.size]
            if actual != expected:
                raise ValueError(f'runtime file contents mismatch: {name}')
        return len(files)
    finally:
        image.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('image')
    parser.add_argument('--assets', type=Path, default=DEFAULT_DIRECTORY)
    parser.add_argument('--sector-size', type=int, choices=(512, 4096), default=512)
    parser.add_argument('--data-fs', choices=('exfat', 'fat32'), default='exfat')
    args = parser.parse_args()
    try:
        count = verify_media(args.image, args.assets, args.sector_size, args.data_fs)
        print(f'verified {count} pinned runtime resources, notices, and provenance files in release media')
        return 0
    except (OSError, ValueError, VerifyError) as error:
        print(f'Release runtime verification failed: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
