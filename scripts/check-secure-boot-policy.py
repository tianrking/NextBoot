#!/usr/bin/env python3
"""Validate the checked-in Secure Boot release policy."""

from __future__ import annotations

import argparse
import copy
import json
import sys
from pathlib import Path
from typing import Any


PROJECT_DIR = Path(__file__).resolve().parents[1]
DEFAULT_POLICY = PROJECT_DIR / "docs" / "secure-boot-release-policy.json"
REQUIRED_ARCHES = {
    "x86_64-unknown-uefi",
    "i686-unknown-uefi",
    "aarch64-unknown-uefi",
}
READY_SHIM_STATES = {"submitted", "accepted", "bundled"}
READY_CA_STATES = {"submitted", "accepted", "signed"}


def as_dict(value: Any) -> dict[str, Any]:
    return value if isinstance(value, dict) else {}


def text(value: Any) -> str:
    return value.strip() if isinstance(value, str) else ""


def text_list(value: Any) -> list[str]:
    if not isinstance(value, list):
        return []
    return [item.strip() for item in value if isinstance(item, str) and item.strip()]


def require_text(errors: list[str], data: dict[str, Any], field: str, label: str) -> None:
    if not text(data.get(field)):
        errors.append(f"{label}.{field} must be a non-empty string")


def require_text_list(
    errors: list[str],
    data: dict[str, Any],
    field: str,
    label: str,
    minimum: int = 1,
) -> list[str]:
    values = text_list(data.get(field))
    if len(values) < minimum:
        errors.append(f"{label}.{field} must contain at least {minimum} item(s)")
    return values


def require_no_placeholder(errors: list[str], value: str, label: str) -> None:
    lowered = value.lower()
    placeholders = ("todo", "tbd", "placeholder", "not-submitted", "not-ready")
    if not value or any(marker in lowered for marker in placeholders):
        errors.append(f"{label} must be finalized for production Secure Boot")


def validate_local_owner_key(policy: dict[str, Any], errors: list[str]) -> None:
    local = as_dict(policy.get("local_owner_key"))
    require_text(errors, local, "certificate_common_name", "local_owner_key")
    methods = set(require_text_list(errors, local, "enrollment_methods", "local_owner_key", 2))
    missing_methods = {"firmware-db", "shim-mok"} - methods
    if missing_methods:
        errors.append(f"local_owner_key.enrollment_methods missing: {sorted(missing_methods)}")

    artifacts = require_text_list(errors, local, "generated_artifacts", "local_owner_key", 4)
    required_suffixes = (".key", ".crt", ".cer", "-signed.efi")
    for suffix in required_suffixes:
        if not any(artifact.endswith(suffix) for artifact in artifacts):
            errors.append(f"local_owner_key.generated_artifacts missing *{suffix}")
    require_text(errors, local, "private_key_storage", "local_owner_key")


def validate_public_requirements(
    policy: dict[str, Any],
    production_ready: bool,
    errors: list[str],
) -> None:
    public = as_dict(policy.get("public_distribution_requirements"))
    sbat = as_dict(public.get("sbat"))
    revocation = as_dict(public.get("revocation"))
    key_mgmt = as_dict(public.get("release_key_management"))

    require_text(errors, public, "shim_strategy", "public_distribution_requirements")
    require_text(errors, public, "microsoft_uefi_ca_status", "public_distribution_requirements")
    require_text(errors, sbat, "vendor", "public_distribution_requirements.sbat")
    require_text(errors, sbat, "product", "public_distribution_requirements.sbat")
    require_text(errors, sbat, "component", "public_distribution_requirements.sbat")
    verification = set(
        require_text_list(
            errors,
            public,
            "artifact_verification",
            "public_distribution_requirements",
        )
    )
    if "sbverify" not in verification:
        errors.append("public_distribution_requirements.artifact_verification must include sbverify")

    if production_ready:
        shim_state = text(public.get("shim_strategy"))
        ca_state = text(public.get("microsoft_uefi_ca_status"))
        if shim_state not in READY_SHIM_STATES:
            errors.append("production policy requires a submitted, accepted, or bundled shim strategy")
        if ca_state not in READY_CA_STATES:
            errors.append("production policy requires a submitted, accepted, or signed CA status")

        generation = sbat.get("generation")
        if not isinstance(generation, int) or generation < 1:
            errors.append("production policy requires public_distribution_requirements.sbat.generation >= 1")
        require_no_placeholder(errors, text(sbat.get("policy_url")), "SBAT policy URL")
        require_no_placeholder(errors, text(revocation.get("owner")), "revocation owner")
        require_no_placeholder(errors, text(revocation.get("contact")), "revocation contact")
        require_no_placeholder(errors, text(revocation.get("policy_url")), "revocation policy URL")
        require_no_placeholder(errors, text(key_mgmt.get("custody")), "release key custody")
        require_no_placeholder(errors, text(key_mgmt.get("rotation")), "release key rotation")
        require_no_placeholder(errors, text(key_mgmt.get("access_control")), "release key access control")
        if public.get("authenticated_variable_updates") is not True:
            errors.append("production policy must enable authenticated variable update generation")


def validate_policy(policy: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    if policy.get("version") != 1:
        errors.append("version must be 1")

    mode = text(policy.get("distribution_mode"))
    if mode not in {"owner-controlled-key", "public-shim"}:
        errors.append("distribution_mode must be owner-controlled-key or public-shim")

    production_ready = policy.get("production_ready")
    if not isinstance(production_ready, bool):
        errors.append("production_ready must be boolean")
        production_ready = False
    if production_ready and mode != "public-shim":
        errors.append("production_ready requires distribution_mode=public-shim")

    arches = set(text_list(policy.get("supported_architectures")))
    missing_arches = REQUIRED_ARCHES - arches
    if missing_arches:
        errors.append(f"supported_architectures missing: {sorted(missing_arches)}")

    validate_local_owner_key(policy, errors)
    validate_public_requirements(policy, bool(production_ready), errors)

    blockers = text_list(policy.get("blockers"))
    if production_ready and blockers:
        errors.append("production_ready policy must not list blockers")
    if not production_ready and len(blockers) < 3:
        errors.append("non-production policy must list at least three blockers")
    if not production_ready and not any("SBAT" in blocker.upper() for blocker in blockers):
        errors.append("non-production blockers must mention SBAT or revocation policy")
    if not production_ready and not any("shim" in blocker.lower() or "CA" in blocker for blocker in blockers):
        errors.append("non-production blockers must mention shim or CA signing")
    return errors


def load_policy(path: Path) -> dict[str, Any]:
    with path.open() as handle:
        data = json.load(handle)
    if not isinstance(data, dict):
        raise ValueError("policy root must be a JSON object")
    return data


def production_fixture() -> dict[str, Any]:
    policy = load_policy(DEFAULT_POLICY)
    policy["distribution_mode"] = "public-shim"
    policy["production_ready"] = True
    policy["blockers"] = []
    public = policy["public_distribution_requirements"]
    public["shim_strategy"] = "submitted"
    public["microsoft_uefi_ca_status"] = "submitted"
    public["sbat"]["generation"] = 1
    public["sbat"]["policy_url"] = "https://example.invalid/nextboot/sbat"
    public["revocation"] = {
        "owner": "NextBoot release engineering",
        "contact": "security@example.invalid",
        "policy_url": "https://example.invalid/nextboot/revocations",
    }
    public["release_key_management"] = {
        "custody": "offline signing token",
        "rotation": "annual or incident-driven",
        "access_control": "two-person release approval",
    }
    public["authenticated_variable_updates"] = True
    return policy


def run_self_test() -> None:
    ready = production_fixture()
    ready_errors = validate_policy(ready)
    if ready_errors:
        raise AssertionError(f"valid production fixture failed: {ready_errors}")

    broken = copy.deepcopy(ready)
    broken["production_ready"] = True
    broken["distribution_mode"] = "owner-controlled-key"
    broken["public_distribution_requirements"]["sbat"]["generation"] = 0
    broken["public_distribution_requirements"]["revocation"]["contact"] = "TODO"
    broken_errors = validate_policy(broken)
    expected = (
        "production_ready requires distribution_mode=public-shim",
        "production policy requires public_distribution_requirements.sbat.generation >= 1",
        "revocation contact must be finalized for production Secure Boot",
    )
    for needle in expected:
        if not any(needle in error for error in broken_errors):
            raise AssertionError(f"negative fixture missed {needle}: {broken_errors}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--policy", type=Path, default=DEFAULT_POLICY, help=f"policy JSON path (default: {DEFAULT_POLICY})")
    parser.add_argument("--no-self-test", action="store_true", help="skip built-in positive and negative validator fixtures")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        policy = load_policy(args.policy)
        errors = validate_policy(policy)
        if not args.no_self_test:
            run_self_test()
    except (OSError, ValueError, json.JSONDecodeError, AssertionError) as error:
        print(f"secure boot policy check failed: {error}", file=sys.stderr)
        return 1

    if errors:
        print("secure boot policy check failed:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1

    print("secure boot policy check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
