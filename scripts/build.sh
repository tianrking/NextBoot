#!/usr/bin/env bash
# NextBoot UEFI build script.
#
# Usage:
#   ./scripts/build.sh          - Build in debug mode
#   ./scripts/build.sh check    - Type-check the UEFI binary
#   ./scripts/build.sh release  - Build in release mode
#   TARGET=all ./scripts/build.sh release - Build x86_64, IA32, and AArch64 artifacts

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

TARGET="${TARGET:-x86_64-unknown-uefi}"
RUSTC_BIN="${RUSTC:-}"
CARGO_BIN="${CARGO:-}"
ALL_TARGETS=(x86_64-unknown-uefi i686-unknown-uefi aarch64-unknown-uefi)

if [ "${TARGET}" = "all" ]; then
    for nextboot_target in "${ALL_TARGETS[@]}"; do
        echo ""
        echo -e "${YELLOW}Building ${nextboot_target}...${NC}"
        TARGET="${nextboot_target}" "$0" "$@"
    done
    exit 0
fi

echo -e "${GREEN}NextBoot Build Script${NC}"
echo "========================"

rust_toolchain_channel() {
    awk -F '"' '/channel[[:space:]]*=/ { print $2; exit }' rust-toolchain.toml 2>/dev/null
}

fallback_toolchain_bin() {
    local binary="$1"
    local channel
    channel="$(rust_toolchain_channel)"
    [ -n "${channel}" ] || return 1

    for toolchain_dir in "${HOME}/.rustup/toolchains/${channel}" "${HOME}/.rustup/toolchains/${channel}-"*; do
        if [ -x "${toolchain_dir}/bin/${binary}" ]; then
            printf '%s\n' "${toolchain_dir}/bin/${binary}"
            return 0
        fi
    done

    return 1
}

resolve_rustc() {
    if [ -n "${RUSTC_BIN}" ] && "${RUSTC_BIN}" --print sysroot >/dev/null 2>&1; then
        return
    fi

    local path_rustc
    local path_rustc_link
    path_rustc="$(command -v rustc || true)"
    path_rustc_link="$(readlink "${path_rustc}" 2>/dev/null || true)"
    if [ -n "${path_rustc}" ] && [ "${path_rustc_link}" != "rustup" ]; then
        if "${path_rustc}" --print sysroot >/dev/null 2>&1; then
            RUSTC_BIN="${path_rustc}"
            return
        fi
    fi

    RUSTC_BIN="$(fallback_toolchain_bin rustc || true)"
    if [ -z "${RUSTC_BIN}" ]; then
        echo -e "${RED}Error: Rust is not installed or rustup failed to activate the toolchain${NC}"
        echo "Please install Rust from https://rustup.rs/ or set RUSTC=/path/to/rustc."
        exit 1
    fi
}

resolve_cargo() {
    if [ -n "${CARGO_BIN}" ] && "${CARGO_BIN}" --version >/dev/null 2>&1; then
        return
    fi

    local sibling_cargo
    sibling_cargo="$(dirname "${RUSTC_BIN}")/cargo"
    if [ -x "${sibling_cargo}" ]; then
        CARGO_BIN="${sibling_cargo}"
        return
    fi

    if command -v cargo >/dev/null 2>&1 && cargo --version >/dev/null 2>&1; then
        CARGO_BIN="$(command -v cargo)"
        return
    fi

    echo -e "${RED}Error: Cargo is not installed or rustup failed to activate the toolchain${NC}"
    echo "Please install Rust from https://rustup.rs/ or set CARGO=/path/to/cargo."
    exit 1
}

resolve_rustc
resolve_cargo

SYSROOT="$("${RUSTC_BIN}" --print sysroot)"
TOOLCHAIN="$(basename "${SYSROOT}")"

target_has_core() {
    local target_libdir
    target_libdir="$("${RUSTC_BIN}" --print target-libdir --target "${TARGET}" 2>/dev/null || true)"
    [ -n "${target_libdir}" ] && ls "${target_libdir}"/libcore-*.rlib >/dev/null 2>&1
}

ensure_target() {
    if target_has_core; then
        return
    fi

    if ! command -v rustup &> /dev/null; then
        echo -e "${RED}Error: ${TARGET} target is not installed and rustup is unavailable${NC}"
        exit 1
    fi

    echo -e "${YELLOW}Installing ${TARGET} target for ${TOOLCHAIN}...${NC}"
    if ! rustup target add "${TARGET}" --toolchain "${TOOLCHAIN}"; then
        echo -e "${RED}Error: failed to install ${TARGET} for ${TOOLCHAIN}${NC}"
        echo "Try repairing the toolchain and rerun:"
        echo "  rustup component add rust-src --toolchain ${TOOLCHAIN}"
        echo "  rustup target add ${TARGET} --toolchain ${TOOLCHAIN}"
        exit 1
    fi
}

ensure_rust_src() {
    local rust_src="${SYSROOT}/lib/rustlib/src/rust/library/core/src/lib.rs"
    if [ -f "${rust_src}" ]; then
        return
    fi

    if ! command -v rustup &> /dev/null; then
        echo -e "${YELLOW}Warning: rust-src is not installed and rustup is unavailable${NC}"
        return
    fi

    echo -e "${YELLOW}Installing rust-src component for ${TOOLCHAIN}...${NC}"
    rustup component add rust-src --toolchain "${TOOLCHAIN}" || {
        echo -e "${YELLOW}Warning: rust-src install failed; continuing because prebuilt ${TARGET} core may be enough${NC}"
    }
}

ensure_target
if [ "${NEXTBOOT_REQUIRE_RUST_SRC:-0}" = "1" ]; then
    ensure_rust_src
fi

case "${1:-debug}" in
    debug)
        BUILD_MODE="debug"
        CARGO_ARGS=(build --target "${TARGET}")
        ;;
    check)
        BUILD_MODE="check"
        CARGO_ARGS=(check --target "${TARGET}")
        ;;
    release)
        BUILD_MODE="release"
        CARGO_ARGS=(build --target "${TARGET}" --release)
        ;;
    *)
        echo -e "${RED}Error: unknown build mode '${1}'${NC}"
        echo "Usage: $0 [debug|check|release]"
        exit 1
        ;;
esac

echo -e "${YELLOW}Target: ${TARGET}${NC}"
echo -e "${YELLOW}Toolchain: ${TOOLCHAIN}${NC}"
echo -e "${YELLOW}Rustc: ${RUSTC_BIN}${NC}"
echo -e "${YELLOW}Cargo: ${CARGO_BIN}${NC}"
echo -e "${YELLOW}Running cargo ${CARGO_ARGS[*]}...${NC}"

RUSTC="${RUSTC_BIN}" "${CARGO_BIN}" "${CARGO_ARGS[@]}"

if [ "${BUILD_MODE}" = "check" ]; then
    echo -e "${GREEN}UEFI check successful!${NC}"
    exit 0
fi

echo -e "${GREEN}Build successful!${NC}"

OUTPUT_DIR="target/${TARGET}/${BUILD_MODE}"
OUTPUT_FILE="${OUTPUT_DIR}/nextboot-boot.efi"

if [ -f "$OUTPUT_FILE" ]; then
    SIZE=$(du -h "$OUTPUT_FILE" | cut -f1)
    echo -e "${GREEN}Output: ${OUTPUT_FILE} (${SIZE})${NC}"
fi
