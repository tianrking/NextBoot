#!/usr/bin/env bash
# Generate local Secure Boot material and sign/verify NextBoot EFI binaries.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${TARGET:-x86_64-unknown-uefi}"
SECURE_BOOT_DIR="${NEXTBOOT_SB_DIR:-${ROOT_DIR}/target/secure-boot}"
CERT_NAME="${NEXTBOOT_SB_NAME:-NextBoot Test Secure Boot}"
CERT_DAYS="${NEXTBOOT_SB_DAYS:-3650}"
CERT_PEM="${NEXTBOOT_SB_CERT:-${SECURE_BOOT_DIR}/nextboot-db.crt}"
CERT_DER="${NEXTBOOT_SB_DER:-${SECURE_BOOT_DIR}/nextboot-db.cer}"
KEY_PEM="${NEXTBOOT_SB_KEY:-${SECURE_BOOT_DIR}/nextboot-db.key}"

usage() {
    cat <<EOF
Usage:
  scripts/secure-boot.sh status
  scripts/secure-boot.sh generate-test-cert [--out-dir DIR] [--name CN] [--days N]
  scripts/secure-boot.sh sign [--input EFI] [--output EFI] [--cert CRT] [--key KEY]
  scripts/secure-boot.sh verify [--input EFI] [--cert CRT]

Environment:
  TARGET              UEFI Rust target, default: x86_64-unknown-uefi
  NEXTBOOT_SB_DIR     Certificate/output directory, default: target/secure-boot
  NEXTBOOT_SB_NAME    Test certificate common name
  NEXTBOOT_SB_DAYS    Test certificate validity days
  NEXTBOOT_SB_CERT    PEM certificate path
  NEXTBOOT_SB_DER     DER certificate path for firmware/MOK enrollment
  NEXTBOOT_SB_KEY     PEM private key path

Notes:
  The generated certificate is for local testing and firmware/MOK enrollment.
  It is not a production Microsoft UEFI CA or shim signing workflow.
EOF
}

fail() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

have() {
    command -v "$1" >/dev/null 2>&1
}

need() {
    local tool="$1"
    local hint="$2"
    have "${tool}" || fail "missing ${tool}. ${hint}"
}

install_hint() {
    local tool="$1"

    case "${tool}" in
        openssl)
            printf 'Install OpenSSL: brew install openssl, apt install openssl, or dnf install openssl\n'
            ;;
        sbsign|sbverify)
            printf 'Install sbsigntools: brew install sbsigntool, apt install sbsigntool, or dnf install sbsigntools\n'
            ;;
        *)
            printf 'Install %s with your platform package manager\n' "${tool}"
            ;;
    esac
}

default_input() {
    local release="${ROOT_DIR}/target/${TARGET}/release/nextboot-boot.efi"
    local debug="${ROOT_DIR}/target/${TARGET}/debug/nextboot-boot.efi"

    if [ -f "${release}" ]; then
        printf '%s\n' "${release}"
    elif [ -f "${debug}" ]; then
        printf '%s\n' "${debug}"
    else
        printf '%s\n' "${release}"
    fi
}

default_output() {
    local input="$1"
    local name
    name="$(basename "${input}" .efi)"
    printf '%s/%s-signed.efi\n' "${SECURE_BOOT_DIR}" "${name}"
}

print_tool_status() {
    local tool="$1"

    if have "${tool}"; then
        printf '  %-8s %s\n' "${tool}" "$(command -v "${tool}")"
    else
        printf '  %-8s missing - %s\n' "${tool}" "$(install_hint "${tool}")"
    fi
}

cmd_status() {
    printf 'NextBoot Secure Boot status\n'
    printf '  target   %s\n' "${TARGET}"
    printf '  efi      %s\n' "$(default_input)"
    printf '  cert pem %s\n' "${CERT_PEM}"
    printf '  cert der %s\n' "${CERT_DER}"
    printf '  key      %s\n' "${KEY_PEM}"
    printf 'Tools:\n'
    print_tool_status openssl
    print_tool_status sbsign
    print_tool_status sbverify
}

cmd_generate_test_cert() {
    need openssl "$(install_hint openssl)"
    mkdir -p "${SECURE_BOOT_DIR}"

    local cn="${CERT_NAME//\//-}"
    local tmp_key="${KEY_PEM}.tmp"
    local tmp_cert="${CERT_PEM}.tmp"
    local tmp_der="${CERT_DER}.tmp"

    openssl req \
        -new -x509 -newkey rsa:2048 \
        -nodes -sha256 -days "${CERT_DAYS}" \
        -subj "/CN=${cn}/" \
        -keyout "${tmp_key}" \
        -out "${tmp_cert}" >/dev/null 2>&1

    openssl x509 -in "${tmp_cert}" -outform DER -out "${tmp_der}"
    chmod 0600 "${tmp_key}"
    mv "${tmp_key}" "${KEY_PEM}"
    mv "${tmp_cert}" "${CERT_PEM}"
    mv "${tmp_der}" "${CERT_DER}"

    printf 'Generated local Secure Boot test material:\n'
    printf '  key      %s\n' "${KEY_PEM}"
    printf '  cert pem %s\n' "${CERT_PEM}"
    printf '  cert der %s\n' "${CERT_DER}"
    printf 'Enroll the DER certificate into firmware db or MOK before booting the signed EFI.\n'
}

cmd_sign() {
    local input="$(default_input)"
    local output=""
    local cert="${CERT_PEM}"
    local key="${KEY_PEM}"

    while [ "$#" -gt 0 ]; do
        case "$1" in
            --input)
                input="${2:-}"
                shift 2
                ;;
            --output)
                output="${2:-}"
                shift 2
                ;;
            --cert)
                cert="${2:-}"
                shift 2
                ;;
            --key)
                key="${2:-}"
                shift 2
                ;;
            *)
                fail "unknown sign option: $1"
                ;;
        esac
    done

    [ -n "${output}" ] || output="$(default_output "${input}")"
    [ -f "${input}" ] || fail "input EFI not found: ${input}. Run scripts/build.sh release first."
    [ -f "${cert}" ] || fail "certificate not found: ${cert}. Run generate-test-cert first."
    [ -f "${key}" ] || fail "private key not found: ${key}. Run generate-test-cert first."
    need sbsign "$(install_hint sbsign)"

    mkdir -p "$(dirname "${output}")"
    sbsign --key "${key}" --cert "${cert}" --output "${output}" "${input}"
    printf 'Signed EFI:\n'
    printf '  input  %s\n' "${input}"
    printf '  output %s\n' "${output}"
}

cmd_verify() {
    local input="$(default_output "$(default_input)")"
    local cert="${CERT_PEM}"

    while [ "$#" -gt 0 ]; do
        case "$1" in
            --input)
                input="${2:-}"
                shift 2
                ;;
            --cert)
                cert="${2:-}"
                shift 2
                ;;
            *)
                fail "unknown verify option: $1"
                ;;
        esac
    done

    [ -f "${input}" ] || fail "signed EFI not found: ${input}"
    [ -f "${cert}" ] || fail "certificate not found: ${cert}"
    need sbverify "$(install_hint sbverify)"

    sbverify --cert "${cert}" "${input}"
}

parse_common_options() {
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --out-dir)
                SECURE_BOOT_DIR="${2:-}"
                CERT_PEM="${NEXTBOOT_SB_CERT:-${SECURE_BOOT_DIR}/nextboot-db.crt}"
                CERT_DER="${NEXTBOOT_SB_DER:-${SECURE_BOOT_DIR}/nextboot-db.cer}"
                KEY_PEM="${NEXTBOOT_SB_KEY:-${SECURE_BOOT_DIR}/nextboot-db.key}"
                shift 2
                ;;
            --name)
                CERT_NAME="${2:-}"
                shift 2
                ;;
            --days)
                CERT_DAYS="${2:-}"
                shift 2
                ;;
            *)
                fail "unknown generate-test-cert option: $1"
                ;;
        esac
    done
}

case "${1:-}" in
    -h|--help|help)
        usage
        ;;
    status)
        cmd_status
        ;;
    generate-test-cert|cert)
        shift
        parse_common_options "$@"
        cmd_generate_test_cert
        ;;
    sign)
        shift
        cmd_sign "$@"
        ;;
    verify)
        shift
        cmd_verify "$@"
        ;;
    "")
        usage
        exit 1
        ;;
    *)
        fail "unknown command: $1"
        ;;
esac
