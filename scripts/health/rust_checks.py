"""Rust toolchain and build health checks."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from command_utils import shell_command

from health.common import CheckResult, HOST_TEST_PACKAGES, PROJECT_DIR


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
        if os.name == "nt":
            candidate = candidate.with_suffix(".exe")
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
    if os.name == "nt":
        sibling = sibling.with_suffix(".exe")
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
    outputs: list[str] = []
    failed = False
    for package in HOST_TEST_PACKAGES:
        result = subprocess.run(
            [
                str(cargo),
                "test",
                "--locked",
                "-p",
                package,
                "--lib",
                "--target",
                host_target,
            ],
            cwd=PROJECT_DIR,
                env=env,
                text=True,
                encoding="utf-8",
                errors="replace",
                stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            check=False,
        )
        outputs.append(f"== {package} ==\n{result.stdout}")
        failed = failed or result.returncode != 0
    name = f"Rust host unit tests ({', '.join(HOST_TEST_PACKAGES)}, {host_target})"
    return CheckResult(name, not failed, "\n".join(outputs))


def check_build(build_target: str) -> CheckResult:
    env = os.environ.copy()
    env["TARGET"] = build_target
    build_script = PROJECT_DIR / "scripts" / "build.sh"
    command = [str(build_script), "check"]
    if os.name == "nt":
        # build.sh is a Bash script.  Executing it directly with CreateProcess
        # yields WinError 193 even when the supported Git Bash environment is
        # installed and the UEFI toolchain itself is healthy.
        git_bash = Path(os.environ.get("ProgramFiles", r"C:\\Program Files")) / "Git" / "bin" / "bash.exe"
        if not git_bash.is_file():
            return CheckResult(
                f"UEFI build check ({build_target})",
                False,
                "Git Bash is required on Windows to execute scripts/build.sh",
            )
        command = shell_command(str(build_script), "check")
    result = subprocess.run(
        command,
        cwd=PROJECT_DIR,
        env=env,
        text=True,
        encoding="utf-8",
        errors="replace",
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    return CheckResult(
        f"UEFI build check ({build_target})",
        result.returncode == 0,
        result.stdout,
    )
