#!/usr/bin/env python3
"""Boot a small real-ISO compatibility matrix through NextBoot under QEMU."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import socket
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from runtime_assets import prepare


PROJECT_DIR = Path(__file__).resolve().parents[1]
TARGET_DIR = PROJECT_DIR / "target" / "real-iso"


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
    qemu_args: tuple[str, ...] = ()


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
        name="debian-13.6-netinst",
        filename="debian-13.6.0-amd64-netinst.iso",
        url="https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/debian-13.6.0-amd64-netinst.iso",
        sha256="65273beed27b2df543b68b65630ba525cfbad8df2b12035732b2dff87d6664e7",
        disk_size_mib=1536,
        memory_mib=1536,
        timeout=300,
        expects=("Select a language",),
    ),
    IsoCase(
        name="ubuntu-26.04-server",
        filename="ubuntu-26.04-live-server-amd64.iso",
        url="https://releases.ubuntu.com/26.04/ubuntu-26.04-live-server-amd64.iso",
        sha256="dec49008a71f6098d0bcfc822021f4d042d5f2db279e4d75bdd981304f1ca5d9",
        disk_size_mib=4096,
        memory_mib=4096,
        timeout=600,
        expects=("Ubuntu 26.04 LTS ubuntu-server ttyS0", "Continue in basic mode"),
        qemu_args=("-smp", "2", "-cpu", "max", "-device", "virtio-rng-pci"),
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
    if os.name == "nt" and command[0].endswith(".sh"):
        command = [env.get("NEXTBOOT_BASH", "C:/Program Files/Git/bin/bash.exe"), *command]
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
    return prepare(source=Path(configured) if configured else None, offline=skip_download)


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
    result = run([project_argument(PROJECT_DIR / "scripts" / "build.sh"), "release"], env)
    require(result.returncode == 0, result.stdout)


def project_argument(path: Path) -> str:
    """Give Git Bash relative paths for repository files on Windows."""
    if os.name == "nt":
        try:
            return path.resolve().relative_to(PROJECT_DIR).as_posix()
        except ValueError:
            pass
    return str(path)


def create_disk(case: IsoCase, iso: Path, disk: Path, env: dict[str, str]) -> None:
    result = run(
        [
            project_argument(PROJECT_DIR / "scripts" / "create-release-media.sh"),
            "--skip-build",
            "--target",
            "x86_64-unknown-uefi",
            "--mode",
            "release",
            "--data-fs",
            "exfat",
            "--size",
            str(case.disk_size_mib),
            "--image",
            project_argument(iso),
            "--output",
            project_argument(disk),
            "--ventoy-assets",
            project_argument(Path(env["NEXTBOOT_VENTOY_ASSETS_DIR"])),
        ],
        env,
    )
    require(result.returncode == 0, result.stdout)
    require("verified split GPT layout: NEXBOOT_DATA=exfat NEXBOOT_EFI=FAT16-32MiB" in result.stdout, result.stdout)
    require("pinned runtime resources, notices, and provenance files in release media" in result.stdout, result.stdout)


def boot_case(case: IsoCase, disk: Path, env: dict[str, str]) -> None:
    qemu = env.get("QEMU_BINARY", "qemu-system-x86_64")
    ovmf = ovmf_code_path(env)
    log = TARGET_DIR / f"{case.name}.serial.log"
    # A launcher failure must not attach a previous run's log as fresh evidence.
    log.unlink(missing_ok=True)
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        qmp_port = reservation.getsockname()[1]
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
        sys.executable,
        str(PROJECT_DIR / "scripts" / "qemu-boot-smoke.py"),
        "--timeout",
        str(case.timeout),
        "--log",
        str(log),
        "--send-after",
        "Phase 3: Displaying boot menu",
        "--send-key",
        "enter",
        "--qmp-port",
        str(qmp_port),
        "--terminal-probes",
        *expect_args,
        "--",
        qemu,
        "-machine",
        "q35,accel=tcg",
        "-m",
        f"{case.memory_mib}M",
        *case.qemu_args,
        "-net",
        "none",
        "-nographic",
        "-serial",
        "mon:stdio",
        "-qmp",
        f"tcp:127.0.0.1:{qmp_port},server=on,wait=off",
        "-drive",
        f"if=pflash,format=raw,readonly=on,file={ovmf}",
        "-drive",
        f"if=none,id=nextboot_disk,format=raw,file={disk}",
        "-device",
        "nvme,drive=nextboot_disk,serial=NEXTBOOT0,bootindex=1",
    ]
    evidence_path = TARGET_DIR / f"{case.name}.evidence.json"
    loader = PROJECT_DIR / "target/x86_64-unknown-uefi/release/nextboot-boot.efi"
    evidence = {
        "schema": 1,
        "case": case.name,
        "status": "running",
        "started_at": datetime.now(timezone.utc).isoformat(),
        "validation_level": "qemu-boot-markers-only",
        "host_platform": sys.platform,
        "source_commit": run(["git", "rev-parse", "HEAD"], env).stdout.strip(),
        "working_tree_clean": not run(["git", "status", "--porcelain"], env).stdout.strip(),
        "iso": {"filename": case.filename, "url": case.url, "sha256": sha256_file(TARGET_DIR / case.filename)},
        "loader_sha256": sha256_file(loader),
        "firmware": {"filename": ovmf.name, "sha256": sha256_file(ovmf)},
        "qemu_version": run([qemu, "--version"], env).stdout.splitlines()[0],
        "machine": "q35,accel=tcg",
        "memory_mib": case.memory_mib,
        "extra_qemu_args": case.qemu_args,
        "media": "release builder, GPT/exFAT, NVMe, 512-byte sectors",
        "disk_size_mib": case.disk_size_mib,
        "timeout_seconds": case.timeout,
        "expected_markers": expect_args[1::2],
        "serial_log": log.name,
    }
    started = time.monotonic()
    evidence_path.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8")
    try:
        result = run(command, env, timeout=case.timeout + 60)
        require(result.returncode == 0, result.stdout)
        evidence["status"] = "pass"
    except (AssertionError, OSError, subprocess.TimeoutExpired):
        evidence["status"] = "fail"
        raise
    finally:
        evidence["elapsed_seconds"] = round(time.monotonic() - started, 2)
        if log.exists():
            evidence["serial_log_sha256"] = sha256_file(log)
        evidence_path.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-download", action="store_true", help="require all ISO/Ventoy assets to already exist")
    parser.add_argument("--skip-build", action="store_true", help="use an already built release loader")
    parser.add_argument("--case", choices=[case.name for case in CASES], action="append", help="run only the selected case")
    args = parser.parse_args()

    env = os.environ.copy()
    if os.name == "nt":
        # Git Bash accepts this portable spelling and does not depend on the
        # Microsoft Store's optional python3 app-execution alias.
        env["PYTHON"] = Path(sys.executable).as_posix()
    TARGET_DIR.mkdir(parents=True, exist_ok=True)

    try:
        selected = [case for case in CASES if not args.case or case.name in args.case]
        for case in selected:
            ensure_download(TARGET_DIR / case.filename, case.url, case.sha256, env, args.skip_download)
        env["NEXTBOOT_VENTOY_ASSETS_DIR"] = str(ensure_ventoy_assets(env, args.skip_download))
        if not args.skip_build:
            build_release(env)
        for case in selected:
            iso = TARGET_DIR / case.filename
            disk = TARGET_DIR / f"{case.name}.nextboot.img"
            create_disk(case, iso, disk, env)
            boot_case(case, disk, env)
            print(f"ok - {case.name}", flush=True)
    except (AssertionError, OSError, ValueError, subprocess.TimeoutExpired) as error:
        print(f"real ISO QEMU check failed: {error}", file=sys.stderr)
        return 1

    print(f"checked {len(selected)} real ISO QEMU case(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
