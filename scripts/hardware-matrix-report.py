#!/usr/bin/env python3
"""Render a Markdown readiness report from the hardware compatibility matrix."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from hardware_matrix import (
    DEFAULT_CSV,
    REQUIREMENTS,
    covered_requirements,
    load_rows,
    matching_rows,
    result_counts,
)


DEFAULT_OUTPUT = Path("docs/hardware/hardware-matrix-status.md")


def join_values(values: tuple[str, ...]) -> str:
    return ", ".join(values)


def coverage_line(covered: int, total: int) -> str:
    percent = 0 if total == 0 else round((covered / total) * 100)
    return f"{covered}/{total} ({percent}%)"


def render_report(csv_path: Path, rows: list[dict[str, str]], allow_partial: bool) -> str:
    covered, missing = covered_requirements(rows, allow_partial)
    counts = result_counts(rows)
    claim = "ready" if not missing else "blocked"
    lines = [
        "# Hardware Matrix Status",
        "",
        "This file is generated from `docs/hardware/hardware-matrix.csv`.",
        "Run `./scripts/hardware-matrix-report.py` after adding real hardware rows.",
        "",
        "## Summary",
        "",
        "| Field | Value |",
        "| --- | --- |",
        f"| Source CSV | `{csv_path}` |",
        f"| Data rows | {len(rows)} |",
        f"| Required coverage | {coverage_line(len(covered), len(REQUIREMENTS))} |",
        f"| Production hardware claim | {claim} |",
        f"| Partial rows count as covered | {'yes' if allow_partial else 'no'} |",
        "",
        "## Results",
        "",
        "| Result | Rows |",
        "| --- | ---: |",
    ]
    for result in ("pass", "partial", "fail", "blocked", "unknown"):
        lines.append(f"| {result} | {counts[result]} |")

    lines.extend(
        [
            "",
            "## Required Coverage",
            "",
            "| Requirement | Status | Matching evidence |",
            "| --- | --- | --- |",
        ]
    )
    for requirement in REQUIREMENTS:
        matches = matching_rows(requirement, rows, allow_partial)
        status = "covered" if matches else "missing"
        if matches:
            evidence = ", ".join(row.get("report", "").strip() or row.get("device", "unknown") for row in matches)
        else:
            evidence = (
                f"media={join_values(requirement.media)}; bus={join_values(requirement.bus)}; "
                f"sector={join_values(requirement.sector_size)}; fs={join_values(requirement.data_fs)}; "
                f"image={join_values(requirement.image_type)}"
            )
        lines.append(f"| {requirement.name} | {status} | {evidence} |")

    lines.extend(["", "## Next Evidence To Collect", ""])
    if missing:
        for requirement in missing:
            lines.append(f"- {requirement.name}")
    else:
        lines.append("- No required rows are missing.")
    lines.append("")
    return "\n".join(lines)


def check_output(path: Path, expected: str) -> bool:
    if not path.exists():
        print(f"hardware matrix status report missing: {path}", file=sys.stderr)
        return False
    actual = path.read_text()
    if actual != expected:
        print(f"hardware matrix status report is stale: {path}", file=sys.stderr)
        print(f"regenerate with: ./scripts/hardware-matrix-report.py --output {path}", file=sys.stderr)
        return False
    return True


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--csv", type=Path, default=DEFAULT_CSV, help=f"matrix CSV path (default: {DEFAULT_CSV})")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT, help=f"Markdown output path (default: {DEFAULT_OUTPUT})")
    parser.add_argument("--allow-partial", action="store_true", help="count partial rows as covered")
    parser.add_argument("--check", action="store_true", help="verify that --output already matches generated content")
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

    rendered = render_report(args.csv, rows, args.allow_partial)
    if args.check:
        return 0 if check_output(args.output, rendered) else 1

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(rendered)
    print(f"Wrote {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
