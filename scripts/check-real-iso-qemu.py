#!/usr/bin/env python3
"""Boot a small real-ISO compatibility matrix through NextBoot under QEMU."""

from __future__ import annotations

import argparse
import hashlib
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


PROJECT_DIR = Path(__file__).resolve().parents[1]
TARGET_DIR = PROJECT_DIR / "target" / "real-iso"
VENTOY_VERSION = "1.1.16"
VENTOY_REF = f"v{VENTOY_VERSION}"
VENTOY_REPO = "https://github.com/ventoy/Ventoy.git"


@dataclass(frozen=True)
class IsoCase:
    name: str
    filename: str
    url: str
    sha256: str
    disk_size_mib: int
    memory_mib: int
    timeout: int
    expects: tuple[str, ...]


CASES = (
    IsoCase(
        name="alpine-standard",
        filename="alpine-standard-3.24.1-x86_64.iso",
        url="https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/x86_64/alpine-standard-3.24.1-x86_64.iso",
        sha256="f4dd613206676c62949144c8ad75fc64582099f444dd1485bae104a60f51dd26",
        disk_size_mib=1024,
        memory_mib=1536,
        timeout=360,
        expects=("Welcome to Alpine Linux 3.24", "localhost login:"),
    ),
    IsoCase(
        name="ubuntu-26.04-server",
        filename="ubuntu-26.04-live-server-amd64.iso",
        url="https://releases.ubuntu.com/26.04/ubuntu-26.04-live-server-amd64.iso",
        sha256="dec49008a71f6098d0bcfc822021f4d042d5f2db279e4d75bdd981304f1ca5d9",
        disk_size_mib=4096,
        memory_mib=2048,
        timeout=600,
        expects=("Ubuntu 26.04 LTS ubuntu-server ttyS0", "Continue in basic mode"),
    ),
    IsoCase(
        name="kali-2026.2-netinst",
        filename="kali-linux-2026.2-installer-netinst-amd64.iso",
        url="https://cdimage.kali.org/current/kali-linux-2026.2-installer-netinst-amd64.iso",
        sha256="d32f929dacc48134a31461a09f2160d13ad1d26b820cee920446813ca979b39b",
        disk_size_mib=1536,
        memory_mib=1536,
        timeout=240,
        expects=("Select a language",),
    ),
)


def run(command: list[str], env: dict[str, str], timeout: int | None = None) -> subprocess.CompletedProcess[str]:
    print("+", " ".join(command), flush=True)
    return subprocess.run(
        command,
        cwd=PROJECT_DIR,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=timeout,
        check=False,
    )


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def ensure_download(path: Path, url: str, sha256: str, env: dict[str, str], skip_download: bool) -> None:
    if path.exists() and sha256_file(path) == sha256:
        return
    if skip_download:
        raise AssertionError(f"{path} is missing or has the wrong SHA256")
    if path.exists():
        path.unlink()
    path.parent.mkdir(parents=True, exist_ok=True)
    result = run(["curl", "-L", "--fail", "--retry", "3", "-o", str(path), url], env)
    require(result.returncode == 0, result.stdout)
    actual = sha256_file(path)
    require(actual == sha256, f"{path} SHA256 mismatch: expected {sha256}, got {actual}")


def ensure_ventoy_assets(env: dict[str, str], skip_download: bool) -> Path:
    configured = env.get("NEXTBOOT_VENTOY_ASSETS_DIR")
    if configured:
        path = Path(configured)
        require(path.is_dir(), f"NEXTBOOT_VENTOY_ASSETS_DIR is not a directory: {path}")
        return path

    clone_dir = TARGET_DIR / f"Ventoy-{VENTOY_VERSION}"
    assets = clone_dir / "INSTALL" / "ventoy"
    if assets.is_dir():
        return assets

    if skip_download:
        raise AssertionError("NEXTBOOT_VENTOY_ASSETS_DIR is required when --skip-download is set")

    if clone_dir.exists():
        shutil.rmtree(clone_dir)
    result = run(
        [
            "git",
            "clone",
            "--depth",
            "1",
            "--branch",
            VENTOY_REF,
            VENTOY_REPO,
            str(clone_dir),
        ],
        env,
    )
    require(result.returncode == 0, result.stdout)
    require(assets.is_dir(), f"Ventoy assets not found under {clone_dir}")
    return assets


def ovmf_code_path(env: dict[str, str]) -> Path:
    candidates = [
        env.get("NEXTBOOT_OVMF_CODE"),
        "/usr/share/OVMF/OVMF_CODE.fd",
        "/usr/share/OVMF/OVMF_CODE_4M.fd",
        "/usr/share/qemu/OVMF_CODE.fd",
        "/opt/homebrew/share/qemu/edk2-x86_64-code.fd",
        "/usr/local/share/qemu/edk2-x86_64-code.fd",
    ]
    for candidate in candidates:
        if candidate and Path(candidate).is_file():
            return Path(candidate)
    raise AssertionError("OVMF/EDK2 x86_64 firmware code image was not found")


def build_release(env: dict[str, str]) -> None:
    result = run([str(PROJECT_DIR / "scripts" / "build.sh"), "release"], env)
    require(result.returncode == 0, result.stdout)


def create_disk(case: IsoCase, iso: Path, disk: Path, env: dict[str, str]) -> None:
    result = run(
        [
            str(PROJECT_DIR / "scripts" / "run-qemu.sh"),
            "release",
            "--bus",
            "nvme",
            "--layout",
            "split",
            "--data-fs",
            "exfat",
            "--disk-size",
            str(case.disk_size_mib),
            "--memory",
            f"{case.memory_mib}M",
            "--image",
            str(iso),
            "--disk-image",
            str(disk),
            "--no-run",
        ],
        env,
    )
    require(result.returncode == 0, result.stdout)
    require("verified split GPT layout: NEXBOOT_DATA=exfat NEXBOOT_EFI=FAT16-32MiB" in result.stdout, result.stdout)


def boot_case(case: IsoCase, disk: Path, env: dict[str, str]) -> None:
    qemu = env.get("QEMU_BINARY", "qemu-system-x86_64")
    ovmf = ovmf_code_path(env)
    log = TARGET_DIR / f"{case.name}.serial.log"
    expect_args = [
        "--expect",
        "NextBoot v",
        "--expect",
        "QEMU/EDK2 firmware detected; Linux serial console smoke mode enabled",
        "--expect",
        "Found 1 ISO file(s)",
        "--expect",
        f"Selected: /ISO/{case.filename}",
        "--expect",
        "Prepared Ventoy Linux initrd:",
        "--expect",
        "Registered Linux EFI initrd LoadFile2 provider:",
    ]
    for needle in case.expects:
        expect_args.extend(["--expect", needle])

    command = [
        "python3",
        str(PROJECT_DIR / "scripts" / "qemu-boot-smoke.py"),
        "--timeout",
        str(case.timeout),
        "--log",
        str(log),
        "--send-after",
        "Phase 3: Displaying boot menu",
        "--send-key",
        "enter",
        *expect_args,
        "--",
        qemu,
        "-machine",
        "q35,accel=tcg",
        "-m",
        f"{case.memory_mib}M",
        "-net",
        "none",
        "-nographic",
        "-serial",
        "mon:stdio",
        "-drive",
        f"if=pflash,format=raw,readonly=on,file={ovmf}",
        "-drive",
        f"if=none,id=nextboot_disk,format=raw,file={disk}",
        "-device",
        "nvme,drive=nextboot_disk,serial=NEXTBOOT0,bootindex=1",
    ]
    result = run(command, env, timeout=case.timeout + 60)
    require(result.returncode == 0, result.stdout)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-download", action="store_true", help="require all ISO/Ventoy assets to already exist")
    parser.add_argument("--case", choices=[case.name for case in CASES], action="append", help="run only the selected case")
    args = parser.parse_args()

    env = os.environ.copy()
    TARGET_DIR.mkdir(parents=True, exist_ok=True)

    try:
        selected = [case for case in CASES if not args.case or case.name in args.case]
        for case in selected:
            ensure_download(TARGET_DIR / case.filename, case.url, case.sha256, env, args.skip_download)
        env["NEXTBOOT_VENTOY_ASSETS_DIR"] = str(ensure_ventoy_assets(env, args.skip_download))
        build_release(env)
        for case in selected:
            iso = TARGET_DIR / case.filename
            disk = TARGET_DIR / f"{case.name}.nextboot.img"
            create_disk(case, iso, disk, env)
            boot_case(case, disk, env)
            print(f"ok - {case.name}", flush=True)
    except (AssertionError, subprocess.TimeoutExpired) as error:
        print(f"real ISO QEMU check failed: {error}", file=sys.stderr)
        return 1

    print(f"checked {len(selected)} real ISO QEMU case(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
