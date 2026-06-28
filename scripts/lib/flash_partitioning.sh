# Partitioning and formatting helpers for scripts/flash.sh.

unmount_target_device() {
    warn "Unmounting device..."
    if [[ "$HOST_OS" == "darwin"* ]]; then
        if [ "$DRY_RUN" -eq 0 ]; then
            require_macos_tools
        fi
        DEVICE="$(normalize_macos_device "$DEVICE")"
        run_cmd diskutil unmountDisk "$DEVICE" || true
    else
        if [ "$DRY_RUN" -eq 0 ]; then
            require_linux_tools
        fi
        run_sudo umount "${DEVICE}"* || true
    fi
}

create_target_partitions() {
    warn "Creating GPT partition table..."
    if [[ "$HOST_OS" == "darwin"* ]]; then
        create_macos_partitions
    else
        create_linux_partitions
    fi
}

create_macos_partitions() {
    if [ "$LAYOUT" = "split" ]; then
        run_sudo diskutil partitionDisk "$DEVICE" GPT FAT32 NEXBOOT "${ESP_SIZE_MB}MiB" "$(macos_placeholder_data_fs)" NEXTDATA R
        format_macos_data_partition
    else
        run_sudo diskutil partitionDisk "$DEVICE" GPT FAT32 NEXBOOT 100%
    fi
}

macos_placeholder_data_fs() {
    if [ "$DATA_FS" = "fat32" ]; then
        printf 'FAT32\n'
    else
        printf 'ExFAT\n'
    fi
}

format_macos_data_partition() {
    local data_part="${DEVICE}s2"
    local mkfs_cmd

    case "$DATA_FS" in
        ntfs)
            run_cmd diskutil unmount "$data_part" || true
            mkfs_cmd="$(ntfs_mkfs_command)"
            run_sudo "$mkfs_cmd" -Q -F -L NEXTDATA "$data_part"
            ;;
        ext2|ext3|ext4)
            run_cmd diskutil unmount "$data_part" || true
            run_ext_mkfs "$DATA_FS" "$data_part"
            ;;
        udf)
            run_cmd diskutil unmount "$data_part" || true
            run_udf_mkfs "$data_part"
            ;;
        xfs)
            run_cmd diskutil unmount "$data_part" || true
            mkfs_cmd="$(xfs_mkfs_command)"
            run_sudo "$mkfs_cmd" -f -L NEXTDATA "$data_part"
            ;;
    esac
}

create_linux_partitions() {
    run_sudo parted -s "$DEVICE" mklabel gpt
    if [ "$LAYOUT" = "split" ]; then
        create_linux_split_partitions
    else
        run_sudo parted -s "$DEVICE" mkpart NEXBOOT fat32 1MiB 100%
        run_sudo parted -s "$DEVICE" set 1 esp on
    fi

    run_sudo partprobe "$DEVICE" || true
    if [ "$DRY_RUN" -eq 0 ]; then
        sleep 2
    fi

    format_linux_partitions
}

create_linux_split_partitions() {
    local esp_end="${ESP_SIZE_MB}MiB"
    local data_type

    case "$DATA_FS" in
        ntfs) data_type="ntfs" ;;
        ext2|ext3|ext4) data_type="$DATA_FS" ;;
        xfs) data_type="xfs" ;;
        *) data_type="fat32" ;;
    esac

    run_sudo parted -s "$DEVICE" mkpart NEXBOOT fat32 1MiB "$esp_end"
    run_sudo parted -s "$DEVICE" set 1 esp on
    run_sudo parted -s "$DEVICE" mkpart NEXTDATA "$data_type" "$esp_end" 100%
}

format_linux_partitions() {
    local esp_part
    esp_part="$(linux_partition_path "$DEVICE" 1)"
    run_sudo mkfs.vfat -F 32 -n NEXBOOT "$esp_part"

    if [ "$LAYOUT" = "split" ]; then
        format_linux_data_partition "$(linux_partition_path "$DEVICE" 2)"
    fi
}

format_linux_data_partition() {
    local data_part="$1"
    local mkfs_cmd

    case "$DATA_FS" in
        exfat)
            if [ "$DRY_RUN" -eq 1 ]; then
                mkfs_cmd="mkfs.exfat"
            else
                mkfs_cmd="$(find_linux_exfat_mkfs)"
            fi
            run_sudo "$mkfs_cmd" -n NEXTDATA "$data_part"
            ;;
        ext2|ext3|ext4)
            run_ext_mkfs "$DATA_FS" "$data_part"
            ;;
        ntfs)
            mkfs_cmd="$(ntfs_mkfs_command)"
            run_sudo "$mkfs_cmd" -Q -F -L NEXTDATA "$data_part"
            ;;
        udf)
            run_udf_mkfs "$data_part"
            ;;
        xfs)
            mkfs_cmd="$(xfs_mkfs_command)"
            run_sudo "$mkfs_cmd" -f -L NEXTDATA "$data_part"
            ;;
        fat32)
            run_sudo mkfs.vfat -F 32 -n NEXTDATA "$data_part"
            ;;
    esac
}
