#!/usr/bin/env python3
"""Check customer-burnable release media image generation."""

from __future__ import annotations

import lzma
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


PROJECT_DIR = Path(__file__).resolve().parents[1]
RELEASE_SCRIPT = PROJECT_DIR / "scripts" / "create-release-media.sh"
BURN_SCRIPT = PROJECT_DIR / "scripts" / "burn-release-media.sh"
GROW_SCRIPT = PROJECT_DIR / "scripts" / "grow-release-media.py"
VERIFY_SCRIPT = PROJECT_DIR / "scripts" / "verify-qemu-image.py"


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
        command.extend(["--growable-max-size", "512"])
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


def compress_xz(source: Path, target: Path) -> None:
    with source.open("rb") as src, lzma.open(target, "wb", preset=0) as dst:
        shutil.copyfileobj(src, dst, length=1024 * 1024)


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
            require("Growable NEXTDATA target ceiling" in seeded.stdout, seeded.stdout)

            multi = run_case(workdir, with_image=True, multi_efi=True)
            require(multi.returncode == 0, multi.stdout)
            require("verified 3 EFI fallback loader(s)" in multi.stdout, multi.stdout)
            require("BOOTX64.EFI BOOTIA32.EFI BOOTAA64.EFI" in multi.stdout, multi.stdout)
            require("verified 1 /ISO image file(s)" in multi.stdout, multi.stdout)

            compressed_multi = workdir / "release-multi-efi.img.xz"
            compress_xz(workdir / "release-multi-efi.img", compressed_multi)

            burn_target = workdir / "burned-target.img"
            with burn_target.open("wb") as target:
                target.truncate(256 * 1024 * 1024)
            burn = subprocess.run(
                [
                    str(BURN_SCRIPT),
                    "--allow-file",
                    "--no-mount",
                    "-y",
                    "--image",
                    str(compressed_multi),
                    str(burn_target),
                ],
                cwd=PROJECT_DIR,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                check=False,
            )
            require(burn.returncode == 0, burn.stdout)
            require("Done. Open NEXTDATA" in burn.stdout, burn.stdout)

            verify_burn = subprocess.run(
                [
                    str(VERIFY_SCRIPT),
                    "--disk-image",
                    str(burn_target),
                    "--sector-size",
                    "512",
                    "--layout",
                    "split",
                    "--data-fs",
                    "exfat",
                    "--efi-file",
                    str(workdir / "nextboot-boot-x64.efi"),
                    "--efi-loader",
                    f"BOOTIA32.EFI={workdir / 'nextboot-boot-ia32.efi'}",
                    "--efi-loader",
                    f"BOOTAA64.EFI={workdir / 'nextboot-boot-aa64.efi'}",
                    "--image",
                    str(workdir / "customer.iso"),
                ],
                cwd=PROJECT_DIR,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                check=False,
            )
            require(verify_burn.returncode == 0, verify_burn.stdout)

            default_capacity = run_case(
                workdir,
                with_image=False,
                multi_efi=True,
                default_capacity=True,
            )
            require(default_capacity.returncode == 0, default_capacity.stdout)
            require("release-default-capacity.img" in default_capacity.stdout, default_capacity.stdout)
            require("verified 3 EFI fallback loader(s)" in default_capacity.stdout, default_capacity.stdout)

            grow = subprocess.run(
                [
                    str(GROW_SCRIPT),
                    "--disk-image",
                    str(workdir / "release-multi-efi.img"),
                    "--sector-size",
                    "512",
                    "--target-size-mib",
                    "256",
                ],
                cwd=PROJECT_DIR,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                check=False,
            )
            require(grow.returncode == 0, grow.stdout)
            require("grew NEXTDATA" in grow.stdout, grow.stdout)

            verify = subprocess.run(
                [
                    str(VERIFY_SCRIPT),
                    "--disk-image",
                    str(workdir / "release-multi-efi.img"),
                    "--sector-size",
                    "512",
                    "--layout",
                    "split",
                    "--data-fs",
                    "exfat",
                    "--efi-file",
                    str(workdir / "nextboot-boot-x64.efi"),
                    "--efi-loader",
                    f"BOOTIA32.EFI={workdir / 'nextboot-boot-ia32.efi'}",
                    "--efi-loader",
                    f"BOOTAA64.EFI={workdir / 'nextboot-boot-aa64.efi'}",
                    "--image",
                    str(workdir / "customer.iso"),
                ],
                cwd=PROJECT_DIR,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                check=False,
            )
            require(verify.returncode == 0, verify.stdout)
    except AssertionError as error:
        print(f"release media check failed: {error}", file=sys.stderr)
        return 1

    print("release media check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
