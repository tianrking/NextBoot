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


def run_case(
    workdir: Path,
    with_image: bool,
    multi_efi: bool,
    default_capacity: bool = False,
) -> subprocess.CompletedProcess[str]:
    efi = workdir / "nextboot-boot-x64.efi"
    efi.write_bytes(b"NEXTBOOT RELEASE EFI X64 FIXTURE\n")
    if default_capacity:
        output_name = "release-default-capacity.img"
    elif multi_efi:
        output_name = "release-multi-efi.img"
    else:
        output_name = "release-with-image.img" if with_image else "release-empty.img"
    output = workdir / output_name
    command = [
        str(RELEASE_SCRIPT),
        "--skip-build",
        "--efi",
        str(efi),
        "--mode",
        "debug",
        "--sector-size",
        "512",
        "--data-fs",
        "exfat",
        "--output",
        str(output),
    ]
    if not default_capacity:
        command.extend(["--size", "128"])
    if multi_efi:
        ia32 = workdir / "nextboot-boot-ia32.efi"
        aa64 = workdir / "nextboot-boot-aa64.efi"
        ia32.write_bytes(b"NEXTBOOT RELEASE EFI IA32 FIXTURE\n")
        aa64.write_bytes(b"NEXTBOOT RELEASE EFI AA64 FIXTURE\n")
        command.extend(["--extra-efi", f"BOOTIA32.EFI={ia32}"])
        command.extend(["--extra-efi", f"BOOTAA64.EFI={aa64}"])
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
            empty = run_case(workdir, with_image=False, multi_efi=False)
            require(empty.returncode == 0, empty.stdout)
            require("verified 1 EFI fallback loader(s)" in empty.stdout, empty.stdout)
            require("verified 0 /ISO image file(s)" in empty.stdout, empty.stdout)
            require("release-empty.img" in empty.stdout, empty.stdout)

            seeded = run_case(workdir, with_image=True, multi_efi=False)
            require(seeded.returncode == 0, seeded.stdout)
            require("verified 1 /ISO image file(s)" in seeded.stdout, seeded.stdout)
            require("Burn this .img to USB/SSD/SD media" in seeded.stdout, seeded.stdout)

            multi = run_case(workdir, with_image=True, multi_efi=True)
            require(multi.returncode == 0, multi.stdout)
            require("verified 3 EFI fallback loader(s)" in multi.stdout, multi.stdout)
            require("BOOTX64.EFI BOOTIA32.EFI BOOTAA64.EFI" in multi.stdout, multi.stdout)
            require("verified 1 /ISO image file(s)" in multi.stdout, multi.stdout)

            default_capacity = run_case(
                workdir,
                with_image=False,
                multi_efi=True,
                default_capacity=True,
            )
            require(default_capacity.returncode == 0, default_capacity.stdout)
            require("release-default-capacity.img" in default_capacity.stdout, default_capacity.stdout)
            require("verified 3 EFI fallback loader(s)" in default_capacity.stdout, default_capacity.stdout)
    except AssertionError as error:
        print(f"release media check failed: {error}", file=sys.stderr)
        return 1

    print("release media check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
