normalize_macos_device() {
    case "$1" in
        /dev/rdisk*) printf '/dev/disk%s\n' "${1#/dev/rdisk}" ;;
        *) printf '%s\n' "$1" ;;
    esac
}

linux_partition_path() {
    case "$1" in
        *[0-9]) printf '%sp%s\n' "$1" "$2" ;;
        *) printf '%s%s\n' "$1" "$2" ;;
    esac
}

find_linux_exfat_mkfs() {
    if command_exists mkfs.exfat; then
        printf 'mkfs.exfat\n'
    elif command_exists mkexfatfs; then
        printf 'mkexfatfs\n'
    else
        return 1
    fi
}

find_ntfs_mkfs() {
    if command_exists mkfs.ntfs; then
        printf 'mkfs.ntfs\n'
    elif command_exists mkntfs; then
        printf 'mkntfs\n'
    else
        return 1
    fi
}

find_ext_mkfs() {
    local fs_type="$1"
    if command_exists "mkfs.${fs_type}"; then
        printf 'mkfs.%s\n' "$fs_type"
    elif command_exists mke2fs; then
        printf 'mke2fs\n'
    elif command_exists brew && [ -x "$(brew --prefix e2fsprogs 2>/dev/null)/sbin/mkfs.${fs_type}" ]; then
        printf '%s/sbin/mkfs.%s\n' "$(brew --prefix e2fsprogs)" "$fs_type"
    else
        return 1
    fi
}

find_udf_mkfs() {
    if [[ "$HOST_OS" == "darwin"* ]] && command_exists newfs_udf; then
        printf 'newfs_udf\n'
    elif command_exists mkudffs; then
        printf 'mkudffs\n'
    elif command_exists newfs_udf; then
        printf 'newfs_udf\n'
    else
        return 1
    fi
}

find_xfs_mkfs() {
    command_exists mkfs.xfs || return 1
    printf 'mkfs.xfs\n'
}

ntfs_mkfs_command() {
    if [ "$DRY_RUN" -eq 1 ]; then
        printf 'mkfs.ntfs\n'
    else
        find_ntfs_mkfs
    fi
}

ext_mkfs_command() {
    local fs_type="$1"
    if [ "$DRY_RUN" -eq 1 ]; then
        printf 'mkfs.%s\n' "$fs_type"
    else
        find_ext_mkfs "$fs_type"
    fi
}

run_ext_mkfs() {
    local fs_type="$1"
    local device="$2"
    local mkfs_cmd

    mkfs_cmd="$(ext_mkfs_command "$fs_type")"
    if [ "$(basename "$mkfs_cmd")" = "mke2fs" ]; then
        run_sudo "$mkfs_cmd" -t "$fs_type" -F -L NEXTDATA "$device"
    else
        run_sudo "$mkfs_cmd" -F -L NEXTDATA "$device"
    fi
}

udf_mkfs_command() {
    if [ "$DRY_RUN" -eq 1 ]; then
        if [[ "$HOST_OS" == "darwin"* ]]; then
            printf 'newfs_udf\n'
        else
            printf 'mkudffs\n'
        fi
    else
        find_udf_mkfs
    fi
}

run_udf_mkfs() {
    local device="$1"
    local mkfs_cmd

    mkfs_cmd="$(udf_mkfs_command)"
    case "$(basename "$mkfs_cmd")" in
        mkudffs)
            run_sudo "$mkfs_cmd" --media-type=hd --vid=NEXTDATA --lvid=NEXTDATA "$device"
            ;;
        newfs_udf)
            run_sudo "$mkfs_cmd" -v NEXTDATA "$device"
            ;;
        *)
            die "Unsupported UDF formatter: $mkfs_cmd"
            ;;
    esac
}

xfs_mkfs_command() {
    if [ "$DRY_RUN" -eq 1 ]; then
        printf 'mkfs.xfs\n'
    else
        find_xfs_mkfs
    fi
}

detect_ventoy_assets_dir() {
    local candidate
    for candidate in \
        "${PROJECT_DIR}/../Ventoy/INSTALL/ventoy" \
        "${PROJECT_DIR}/Ventoy/INSTALL/ventoy"
    do
        if [ -f "${candidate}/wimboot.x86_64.xz" ]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done
    return 1
}

resolve_ventoy_assets_dir() {
    if [ "$INSTALL_VENTOY_ASSETS" -eq 0 ]; then
        return 1
    fi

    if [ -n "$VENTOY_ASSETS_DIR" ]; then
        [ -d "$VENTOY_ASSETS_DIR" ] || return 1
        [ -f "${VENTOY_ASSETS_DIR}/wimboot.x86_64.xz" ] || return 1
        printf '%s\n' "$VENTOY_ASSETS_DIR"
        return 0
    fi

    detect_ventoy_assets_dir
}

copy_ventoy_assets() {
    local mount_point="$1"
    [ -n "$VENTOY_ASSETS_RESOLVED" ] || return 0

    run_cmd mkdir -p "${mount_point}/ventoy"
    run_cmd cp "${VENTOY_ASSETS_RESOLVED}/wimboot.x86_64.xz" "${mount_point}/ventoy/wimboot.x86_64.xz"
    if [ -f "${VENTOY_ASSETS_RESOLVED}/vtoyjump64.exe" ]; then
        run_cmd cp "${VENTOY_ASSETS_RESOLVED}/vtoyjump64.exe" "${mount_point}/ventoy/vtoyjump64.exe"
    else
        warn "vtoyjump64.exe was not found in ${VENTOY_ASSETS_RESOLVED}; Windows plugin runtime data will not be injected."
    fi
    if [ -f "${VENTOY_ASSETS_RESOLVED}/common_bcd.xz" ]; then
        run_cmd cp "${VENTOY_ASSETS_RESOLVED}/common_bcd.xz" "${mount_point}/ventoy/common_bcd.xz"
    else
        warn "common_bcd.xz was not found in ${VENTOY_ASSETS_RESOLVED}; WIMBOOT will rely on image-provided BCD files."
    fi
}

copy_ventoy_assets_sudo() {
    local mount_point="$1"
    [ -n "$VENTOY_ASSETS_RESOLVED" ] || return 0

    run_sudo mkdir -p "${mount_point}/ventoy"
    run_sudo cp "${VENTOY_ASSETS_RESOLVED}/wimboot.x86_64.xz" "${mount_point}/ventoy/wimboot.x86_64.xz"
    if [ -f "${VENTOY_ASSETS_RESOLVED}/vtoyjump64.exe" ]; then
        run_sudo cp "${VENTOY_ASSETS_RESOLVED}/vtoyjump64.exe" "${mount_point}/ventoy/vtoyjump64.exe"
    else
        warn "vtoyjump64.exe was not found in ${VENTOY_ASSETS_RESOLVED}; Windows plugin runtime data will not be injected."
    fi
    if [ -f "${VENTOY_ASSETS_RESOLVED}/common_bcd.xz" ]; then
        run_sudo cp "${VENTOY_ASSETS_RESOLVED}/common_bcd.xz" "${mount_point}/ventoy/common_bcd.xz"
    else
        warn "common_bcd.xz was not found in ${VENTOY_ASSETS_RESOLVED}; WIMBOOT will rely on image-provided BCD files."
    fi
}

require_linux_tools() {
    command_exists parted || die "parted is required"
    command_exists mkfs.vfat || die "mkfs.vfat is required"
    if [ "$LAYOUT" = "split" ] && [ "$DATA_FS" = "exfat" ]; then
        find_linux_exfat_mkfs >/dev/null || die "mkfs.exfat or mkexfatfs is required for --data-fs exfat"
    fi
    if [ "$LAYOUT" = "split" ] && [[ "$DATA_FS" == ext* ]]; then
        find_ext_mkfs "$DATA_FS" >/dev/null || die "mkfs.${DATA_FS} or mke2fs is required for --data-fs ${DATA_FS}"
    fi
    if [ "$LAYOUT" = "split" ] && [ "$DATA_FS" = "ntfs" ]; then
        find_ntfs_mkfs >/dev/null || die "mkfs.ntfs or mkntfs is required for --data-fs ntfs"
    fi
    if [ "$LAYOUT" = "split" ] && [ "$DATA_FS" = "udf" ]; then
        find_udf_mkfs >/dev/null || die "mkudffs is required for --data-fs udf"
    fi
    if [ "$LAYOUT" = "split" ] && [ "$DATA_FS" = "xfs" ]; then
        find_xfs_mkfs >/dev/null || die "mkfs.xfs is required for --data-fs xfs"
    fi
}

require_macos_tools() {
    if [ "$LAYOUT" = "split" ] && [[ "$DATA_FS" == ext* ]]; then
        find_ext_mkfs "$DATA_FS" >/dev/null || die "mkfs.${DATA_FS} or mke2fs is required for --data-fs ${DATA_FS} on macOS"
        warn "macOS cannot reliably write-mount ext filesystems; the Data partition will be formatted but /ISO and /ventoy must be populated from Linux."
    fi
    if [ "$LAYOUT" = "split" ] && [ "$DATA_FS" = "ntfs" ]; then
        find_ntfs_mkfs >/dev/null || die "mkfs.ntfs or mkntfs is required for --data-fs ntfs on macOS"
        if ! command_exists ntfs-3g; then
            warn "ntfs-3g was not found; macOS may mount the NTFS Data partition read-only after formatting."
            warn "If creating /ISO fails, install a writable NTFS driver or create /ISO from Windows/Linux."
        fi
    fi
    if [ "$LAYOUT" = "split" ] && [ "$DATA_FS" = "udf" ]; then
        find_udf_mkfs >/dev/null || die "newfs_udf or mkudffs is required for --data-fs udf on macOS"
    fi
    if [ "$LAYOUT" = "split" ] && [ "$DATA_FS" = "xfs" ]; then
        find_xfs_mkfs >/dev/null || die "mkfs.xfs is required for --data-fs xfs on macOS"
        warn "macOS cannot reliably write-mount XFS; the Data partition will be formatted but /ISO and /ventoy must be populated from Linux."
    fi
}

mount_point_for_macos_partition() {
    diskutil info "$1" | awk -F': *' '/Mount Point/ {print $2; exit}'
}

ensure_macos_mounted() {
    local partition="$1"
    local mount_point

    mount_point="$(mount_point_for_macos_partition "$partition")"
    if [ -z "$mount_point" ] || [ "$mount_point" = "Not mounted" ]; then
        run_cmd diskutil mount "$partition"
        mount_point="$(mount_point_for_macos_partition "$partition")"
    fi

    [ -n "$mount_point" ] && [ "$mount_point" != "Not mounted" ] || die "Could not mount ${partition}"
    printf '%s\n' "$mount_point"
}

copy_efi_tree() {
    local mount_point="$1"
    local index
    run_cmd mkdir -p "${mount_point}/EFI/BOOT"
    for index in "${!EFI_INSTALL_FILES[@]}"; do
        run_cmd cp "${EFI_INSTALL_FILES[$index]}" "${mount_point}/EFI/BOOT/${EFI_INSTALL_NAMES[$index]}"
    done
}

copy_efi_tree_sudo() {
    local mount_point="$1"
    local index
    run_sudo mkdir -p "${mount_point}/EFI/BOOT"
    for index in "${!EFI_INSTALL_FILES[@]}"; do
        run_sudo cp "${EFI_INSTALL_FILES[$index]}" "${mount_point}/EFI/BOOT/${EFI_INSTALL_NAMES[$index]}"
    done
}
