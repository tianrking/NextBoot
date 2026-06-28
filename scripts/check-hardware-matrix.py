#!/usr/bin/env python3
"""Check whether the real-hardware compatibility matrix covers required rows."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from hardware_matrix import DEFAULT_CSV, covered_requirements, load_rows


def print_requirements(label: str, requirements: list[Requirement]) -> None:
    print(f"{label}: {len(requirements)}")
    for requirement in requirements:
        print(f"  - {requirement.name}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--csv", type=Path, default=DEFAULT_CSV, help=f"matrix CSV path (default: {DEFAULT_CSV})")
    parser.add_argument(
        "--allow-partial",
        action="store_true",
        help="count partial rows as covered; default requires pass rows",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if not args.csv.exists():
        print(f"hardware matrix not found: {args.csv}", file=sys.stderr)
        return 2

    rows, errors = load_rows(args.csv)
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1

    covered, missing = covered_requirements(rows, args.allow_partial)
    print(f"Checked {args.csv} ({len(rows)} data row(s))")
    print_requirements("Covered required rows", covered)
    print_requirements("Missing required rows", missing)
    return 1 if missing else 0


if __name__ == "__main__":
    raise SystemExit(main())
