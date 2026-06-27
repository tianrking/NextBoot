# UEFI target selection helpers for scripts/flash.sh.

configure_flash_target() {
    case "$TARGET" in
        x86_64-unknown-uefi)
            EFI_INSTALL_TARGETS=("x86_64-unknown-uefi")
            EFI_INSTALL_NAMES=("BOOTX64.EFI")
            ;;
        i686-unknown-uefi)
            EFI_INSTALL_TARGETS=("i686-unknown-uefi")
            EFI_INSTALL_NAMES=("BOOTIA32.EFI")
            ;;
        aarch64-unknown-uefi)
            EFI_INSTALL_TARGETS=("aarch64-unknown-uefi")
            EFI_INSTALL_NAMES=("BOOTAA64.EFI")
            ;;
        all)
            EFI_INSTALL_TARGETS=("x86_64-unknown-uefi" "i686-unknown-uefi" "aarch64-unknown-uefi")
            EFI_INSTALL_NAMES=("BOOTX64.EFI" "BOOTIA32.EFI" "BOOTAA64.EFI")
            ;;
        *)
            die "Unsupported UEFI target '${TARGET}'. Supported: x86_64-unknown-uefi, i686-unknown-uefi, aarch64-unknown-uefi, all"
            ;;
    esac
}

find_built_efi() {
    local target="$1"
    local release="${PROJECT_DIR}/target/${target}/release/nextboot-boot.efi"
    local debug="${PROJECT_DIR}/target/${target}/debug/nextboot-boot.efi"

    if [ -f "$release" ]; then
        printf '%s\n' "$release"
    elif [ -f "$debug" ]; then
        printf '%s\n' "$debug"
    else
        return 1
    fi
}

resolve_efi_files() {
    EFI_INSTALL_FILES=()
    local index
    for index in "${!EFI_INSTALL_TARGETS[@]}"; do
        local install_target="${EFI_INSTALL_TARGETS[$index]}"
        local efi_file
        efi_file="$(find_built_efi "$install_target")" || {
            die "EFI file not found for ${install_target}. Run TARGET=${TARGET} ./scripts/build.sh first."
        }
        EFI_INSTALL_FILES+=("$efi_file")
    done
}
