#!/usr/bin/env python3
"""Render verified QEMU serial-console boot evidence as SVG images."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from datetime import datetime, timezone
from html import escape
from pathlib import Path


PROJECT_DIR = Path(__file__).resolve().parents[1]
EVIDENCE_DIR = PROJECT_DIR / "target" / "real-iso"
DEFAULT_OUTPUT_DIR = PROJECT_DIR / "docs" / "assets" / "qemu"

CASES = {
    "alpine-standard": ("Alpine Linux 3.24.1", "localhost login:"),
    "debian-13.6-netinst": ("Debian 13.6 netinst", "Select a language"),
    "fedora-44-workstation": ("Fedora Workstation 44", "Started gdm.service - GNOME Display Manager."),
    "ubuntu-26.04-server": ("Ubuntu Server 26.04 LTS", "Continue in basic mode"),
    "kali-2026.2-netinst": ("Kali Linux 2026.2 netinst", "Select a language"),
}

ANSI_ESCAPE = re.compile(r"(?:\x1b[@-_][0-?]*[ -/]*[@-~]|\x1b\][^\x1b]*(?:\x1b\\|\x07))")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def plain_lines(path: Path) -> list[str]:
    raw = path.read_text(encoding="utf-8", errors="replace")
    raw = ANSI_ESCAPE.sub("", raw).replace("\r", "")
    return ["".join(char if char.isprintable() or char == "\t" else " " for char in line) for line in raw.splitlines()]


def excerpt(lines: list[str], marker: str) -> list[str]:
    index = next((index for index, line in enumerate(lines) if marker in line), None)
    if index is None:
        raise ValueError(f"terminal marker was not found: {marker!r}")
    selected = lines[max(0, index - 12) : index + 12]
    rendered = []
    for line in selected:
        line = line.expandtabs(4)
        if marker in line and len(line) > 118:
            marker_at = line.index(marker)
            start = max(0, marker_at - 20)
            end = min(len(line), start + 116)
            prefix = "…" if start else ""
            suffix = "…" if end < len(line) else ""
            rendered.append(f"{prefix}{line[start:end]}{suffix}")
        else:
            rendered.append(line[:118])
    return rendered


def svg(title: str, marker: str, lines: list[str], captured: str) -> str:
    line_height = 22
    height = 154 + line_height * len(lines)
    texts = []
    for index, line in enumerate(lines):
        color = "#f8fafc" if marker in line else "#cbd5e1"
        texts.append(
            f'<text x="42" y="{138 + index * line_height}" fill="{color}" '
            f'font-family="ui-monospace, SFMono-Regular, Menlo, Consolas, monospace" font-size="15">{escape(line)}</text>'
        )
    joined = "\n  ".join(texts)
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="1120" height="{height}" viewBox="0 0 1120 {height}" role="img" aria-labelledby="title desc">
  <title id="title">{escape(title)} QEMU serial-console capture</title>
  <desc id="desc">QEMU and OVMF boot capture from a generated NextBoot release-media image. The highlighted line is the asserted terminal marker.</desc>
  <rect width="1120" height="{height}" rx="14" fill="#0f172a"/>
  <rect width="1120" height="72" rx="14" fill="#1e293b"/>
  <circle cx="38" cy="36" r="9" fill="#fb7185"/><circle cx="66" cy="36" r="9" fill="#fbbf24"/><circle cx="94" cy="36" r="9" fill="#4ade80"/>
  <text x="128" y="33" fill="#f8fafc" font-family="system-ui, sans-serif" font-size="20" font-weight="700">{escape(title)}</text>
  <text x="128" y="55" fill="#94a3b8" font-family="system-ui, sans-serif" font-size="14">QEMU + OVMF · generated GPT/exFAT release media · NVMe</text>
  <text x="42" y="100" fill="#38bdf8" font-family="system-ui, sans-serif" font-size="14">Captured {escape(captured)} · asserted boot marker highlighted</text>
  {joined}
</svg>
'''


def render_case(case: str, output_dir: Path) -> Path:
    title, marker = CASES[case]
    evidence_path = EVIDENCE_DIR / f"{case}.evidence.json"
    evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
    if evidence.get("status") != "pass":
        raise ValueError(f"{case}: evidence status is not pass")
    log_path = EVIDENCE_DIR / evidence["serial_log"]
    expected_hash = evidence.get("serial_log_sha256")
    actual_hash = sha256_file(log_path)
    if expected_hash != actual_hash:
        raise ValueError(f"{case}: serial log SHA256 does not match its evidence record")
    captured = evidence.get("started_at", datetime.now(timezone.utc).isoformat()).replace("T", " ").replace("+00:00", " UTC")
    output_dir.mkdir(parents=True, exist_ok=True)
    destination = output_dir / f"{case}.svg"
    destination.write_text(svg(title, marker, excerpt(plain_lines(log_path), marker), captured), encoding="utf-8")
    return destination


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", choices=CASES, action="append", help="render only a selected case")
    parser.add_argument("--output-dir", type=Path, default=DEFAULT_OUTPUT_DIR)
    args = parser.parse_args()
    for case in args.case or CASES:
        print(render_case(case, args.output_dir).relative_to(PROJECT_DIR))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
