#!/usr/bin/env python3
"""Prepare the checked runtime bundle used by official media and real ISO tests."""
import argparse
from pathlib import Path
import sys
from runtime_assets import DEFAULT_DIRECTORY, prepare


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=DEFAULT_DIRECTORY)
    parser.add_argument('--source', type=Path, help='import pinned unmodified assets from a local directory')
    parser.add_argument('--offline', action='store_true')
    args = parser.parse_args()
    try:
        print(prepare(args.output, args.source, args.offline).resolve())
        return 0
    except (OSError, ValueError) as error:
        print(f'Runtime preparation failed: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
