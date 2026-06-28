#!/usr/bin/env python3
"""Check customer-burnable release media image generation."""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path


PROJECT_DIR = Path(__file__).resolve().parents[1]
RELEASE_SCRIPT = PROJECT_DIR / "scripts" / "create-release-media.sh"


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def run_case(workdir: Path, with_image: bool) -> subprocess.CompletedProcess[str]:
    efi = workdir / "nextboot-boot.efi"
    efi.write_bytes(b"NEXTBOOT RELEASE EFI FIXTURE\n")
    output = workdir / ("release-with-image.img" if with_image else "release-empty.img")
    command = [
        str(RELEASE_SCRIPT),
        "--skip-build",
        "--efi",
        str(efi),
        "--mode",
        "debug",
        "--size",
        "128",
        "--sector-size",
        "512",
        "--data-fs",
        "exfat",
        "--output",
        str(output),
    ]
    if with_image:
        image = workdir / "customer.iso"
        image.write_bytes(b"NEXTBOOT CUSTOMER ISO FIXTURE\n")
        command.extend(["--image", str(image)])

    return subprocess.run(
        command,
        cwd=PROJECT_DIR,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )


def main() -> int:
    try:
        target_dir = PROJECT_DIR / "target"
        target_dir.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="release-media-check-", dir=target_dir) as tmp:
            workdir = Path(tmp)
            empty = run_case(workdir, with_image=False)
            require(empty.returncode == 0, empty.stdout)
            require("verified 0 /ISO image file(s)" in empty.stdout, empty.stdout)
            require("release-empty.img" in empty.stdout, empty.stdout)

            seeded = run_case(workdir, with_image=True)
            require(seeded.returncode == 0, seeded.stdout)
            require("verified 1 /ISO image file(s)" in seeded.stdout, seeded.stdout)
            require("Burn this .img to USB/SSD/SD media" in seeded.stdout, seeded.stdout)
    except AssertionError as error:
        print(f"release media check failed: {error}", file=sys.stderr)
        return 1

    print("release media check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
