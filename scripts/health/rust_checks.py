"""Rust toolchain and build health checks."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from health.common import CheckResult, HOST_TEST_PACKAGE, PROJECT_DIR


def rust_toolchain_channel() -> str | None:
    toolchain = PROJECT_DIR / "rust-toolchain.toml"
    if not toolchain.exists():
        return None
    for line in toolchain.read_text().splitlines():
        stripped = line.strip()
        if stripped.startswith("channel") and '"' in stripped:
            return stripped.split('"', 2)[1]
    return None


def fallback_toolchain_bin(binary: str) -> Path | None:
    channel = rust_toolchain_channel()
    if not channel:
        return None
    toolchains = Path.home() / ".rustup" / "toolchains"
    for directory in sorted(toolchains.glob(f"{channel}*")):
        candidate = directory / "bin" / binary
        if candidate.exists() and os.access(candidate, os.X_OK):
            return candidate
    return None


def usable_binary(path: str | Path, args: list[str]) -> bool:
    try:
        result = subprocess.run(
            [str(path), *args],
            cwd=PROJECT_DIR,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
    except OSError:
        return False
    return result.returncode == 0


def resolve_rustc() -> Path | None:
    env_rustc = os.environ.get("RUSTC")
    if env_rustc and usable_binary(env_rustc, ["--print", "sysroot"]):
        return Path(env_rustc)
    return fallback_toolchain_bin("rustc")


def resolve_cargo(rustc: Path) -> Path | None:
    env_cargo = os.environ.get("CARGO")
    if env_cargo and usable_binary(env_cargo, ["--version"]):
        return Path(env_cargo)
    sibling = rustc.parent / "cargo"
    if sibling.exists() and os.access(sibling, os.X_OK):
        return sibling
    return fallback_toolchain_bin("cargo")


def rustc_host_target(rustc: Path) -> str | None:
    result = subprocess.run(
        [str(rustc), "-vV"],
        cwd=PROJECT_DIR,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    if result.returncode != 0:
        return None
    for line in result.stdout.splitlines():
        if line.startswith("host: "):
            return line.split(":", 1)[1].strip()
    return None


def check_host_tests() -> CheckResult:
    rustc = resolve_rustc()
    if rustc is None:
        return CheckResult("Rust host unit tests", False, "could not resolve rustc")
    cargo = resolve_cargo(rustc)
    if cargo is None:
        return CheckResult("Rust host unit tests", False, "could not resolve cargo")
    host_target = rustc_host_target(rustc)
    if not host_target:
        return CheckResult(
            "Rust host unit tests",
            False,
            "could not resolve rustc host target",
        )

    env = os.environ.copy()
    env["RUSTC"] = str(rustc)
    result = subprocess.run(
        [
            str(cargo),
            "test",
            "-p",
            HOST_TEST_PACKAGE,
            "--lib",
            "--target",
            host_target,
        ],
        cwd=PROJECT_DIR,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    name = f"Rust host unit tests ({HOST_TEST_PACKAGE}, {host_target})"
    return CheckResult(name, result.returncode == 0, result.stdout)


def check_build(build_target: str) -> CheckResult:
    env = os.environ.copy()
    env["TARGET"] = build_target
    result = subprocess.run(
        [str(PROJECT_DIR / "scripts" / "build.sh"), "check"],
        cwd=PROJECT_DIR,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    return CheckResult(
        f"UEFI build check ({build_target})",
        result.returncode == 0,
        result.stdout,
    )
