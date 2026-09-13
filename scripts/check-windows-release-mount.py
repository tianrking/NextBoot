#!/usr/bin/env python3
"""Mount a generated raw release image through the native Windows exFAT driver.

The fixture is converted only into a fixed VHD container. Its GPT and all
filesystem bytes remain unchanged. The test never accesses a physical disk.
"""
from __future__ import annotations

import argparse
import ctypes
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import time
import uuid


def run(command: list[str]) -> str:
    result = subprocess.run(command, capture_output=True, text=True, encoding="utf-8")
    if result.returncode:
        raise AssertionError(f"{' '.join(command)} failed:\n{result.stdout}\n{result.stderr}")
    return result.stdout.strip()


def powershell(script: str, image: Path) -> str:
    prefix = "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); "
    return run([
        "powershell.exe", "-NoProfile", "-NonInteractive", "-Command", prefix + script,
    ])


def powershell_failure(script: str) -> str:
    """Run a negative writer case and return all diagnostic output."""
    prefix = "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); "
    result = subprocess.run(
        ["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", prefix + script],
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    if result.returncode == 0:
        raise AssertionError(f"expected PowerShell failure: {script}")
    return result.stdout + result.stderr


def be32(buffer: bytearray, offset: int, value: int) -> None:
    struct.pack_into(">I", buffer, offset, value)


def be64(buffer: bytearray, offset: int, value: int) -> None:
    struct.pack_into(">Q", buffer, offset, value)


def fixed_vhd_footer(size: int) -> bytes:
    footer = bytearray(512)
    footer[:8] = b"conectix"
    be32(footer, 8, 2)
    be32(footer, 12, 0x00010000)
    be64(footer, 16, 0xFFFFFFFFFFFFFFFF)
    be32(footer, 24, int(time.time()) - 946684800)
    footer[28:32] = b"nbot"
    be32(footer, 32, 0x00010000)
    footer[36:40] = b"Wi2k"
    be64(footer, 40, size)
    be64(footer, 48, size)
    sectors, heads = 63, 16
    cylinders = min(65535, size // 512 // (heads * sectors))
    struct.pack_into(">HBB", footer, 56, cylinders, heads, sectors)
    be32(footer, 60, 2)
    footer[68:84] = uuid.uuid4().bytes
    footer[84] = 0
    be32(footer, 64, (~sum(footer)) & 0xFFFFFFFF)
    return bytes(footer)


def wrap_fixed_vhd(raw: Path, vhd: Path) -> None:
    if vhd.exists():
        raise ValueError(f"refusing to overwrite {vhd}")
    shutil.copyfile(raw, vhd)
    with vhd.open("ab") as stream:
        stream.write(fixed_vhd_footer(raw.stat().st_size))


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", required=True, type=Path)
    args = parser.parse_args()
    if os.name != "nt" or not ctypes.windll.shell32.IsUserAnAdmin():
        raise ValueError("native Windows mount validation requires an administrator Windows runner")
    raw = args.image.resolve()
    if not raw.is_file():
        raise ValueError(f"missing image: {raw}")
    vhd = raw.with_suffix(raw.suffix + ".windows-mount-test.vhd")
    verification = raw.with_suffix(raw.suffix + ".writer-verify.img")
    writer = Path(__file__).with_name("Write-NextBootMedia.ps1").resolve()
    if not writer.is_file():
        raise ValueError(f"missing Windows verified-writer script: {writer}")
    if verification.exists():
        raise ValueError(f"refusing to overwrite verification target: {verification}")
    shutil.copyfile(raw, verification)
    attached = False
    try:
        writer_script = (
            f"& '{writer}' -ImagePath '{raw}' -TargetPath '{verification}' -VerifyOnly"
        )
        print(powershell(writer_script, vhd))
        invalid_fast_mode = powershell_failure(
            f"& '{writer}' -ImagePath '{raw}' -TargetPath '{verification}' "
            "-VerifyOnly -SkipFullVerification"
        )
        if "SkipFullVerification is available only when writing a physical DiskNumber" not in invalid_fast_mode:
            raise AssertionError(
                "writer did not reject fast mode outside a physical write:\n"
                + invalid_fast_mode
            )
        # Firmware growth and user ISO copies change NEXTDATA, so the full raw
        # digest correctly ceases to match.  Change only a disposable byte in
        # the data partition and prove that the immutable ESP verifier still
        # identifies the exact release loader.
        with verification.open("r+b") as stream:
            stream.seek(64 * 1024 * 1024)
            original = stream.read(1)
            if len(original) != 1:
                raise AssertionError("verification image is unexpectedly smaller than 64 MiB")
            stream.seek(64 * 1024 * 1024)
            stream.write(bytes([original[0] ^ 0x01]))
        esp_writer_script = (
            f"& '{writer}' -ImagePath '{raw}' -TargetPath '{verification}' "
            "-VerifyOnly -VerifyBootPartitionOnly"
        )
        output = powershell(esp_writer_script, vhd)
        if "immutable NextBoot EFI boot partition matches" not in output:
            raise AssertionError(f"immutable EFI verification did not report success: {output}")
        print(output)
        wrap_fixed_vhd(raw, vhd)
        powershell(f"Mount-DiskImage -ImagePath '{vhd}' | Out-Null", vhd)
        attached = True
        script = (
            f"$disk=Get-DiskImage -ImagePath '{vhd}' | Get-Disk; "
            "$volume=Get-Partition -DiskNumber $disk.Number | Get-Volume | "
            "Where-Object { $_.FileSystemLabel -eq 'NEXTDATA' }; "
            "if ($null -eq $volume) { throw 'NEXTDATA volume is missing' }; "
            "if ($volume.FileSystem -ne 'exFAT') { throw ('NEXTDATA filesystem is ' + $volume.FileSystem) }; "
            "if ([string]::IsNullOrEmpty($volume.DriveLetter)) { throw 'NEXTDATA has no drive letter' }; "
            "$root=$volume.DriveLetter + ':\\'; "
            "if (!(Test-Path ($root + 'ISO'))) { throw 'NEXTDATA/ISO is missing' }; "
            "$probe=$root + 'ISO\\nextboot-windows-mount-probe.txt'; "
            "[IO.File]::WriteAllText($probe, 'native Windows exFAT mount check'); "
            "if ([IO.File]::ReadAllText($probe) -ne 'native Windows exFAT mount check') { throw 'write/read probe failed' }; "
            "Remove-Item -LiteralPath $probe -Force; "
            "Write-Output ('passed: NEXTDATA=' + $root + ' filesystem=' + $volume.FileSystem)"
        )
        print(powershell(script, vhd))
    finally:
        if attached:
            powershell(
                f"$image=Get-DiskImage -ImagePath '{vhd}'; if ($image.Attached) {{ Dismount-DiskImage -ImagePath '{vhd}' }}",
                vhd,
            )
        vhd.unlink(missing_ok=True)
        verification.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
