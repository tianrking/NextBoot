# Mount and file-copy helpers for scripts/flash.sh.

populate_target_media() {
    warn "Copying files..."
    if [[ "$HOST_OS" == "darwin"* ]]; then
        populate_macos_media
    else
        populate_linux_media
    fi
}

populate_macos_media() {
    local esp_part="${DEVICE}s1"
    local esp_mount

    esp_mount="$(ensure_macos_mounted "$esp_part")"
    copy_efi_tree "$esp_mount"

    if [ "$LAYOUT" = "split" ]; then
        populate_macos_data_partition
    else
        run_cmd mkdir -p "${esp_mount}/ISO"
        copy_boot_images "$esp_mount"
        copy_ventoy_assets "$esp_mount"
        sync
    fi

    run_cmd diskutil unmount "$esp_part"
}

populate_macos_data_partition() {
    local data_part="${DEVICE}s2"
    local data_mount

    if [[ "$DATA_FS" == ext* ]] || [ "$DATA_FS" = "xfs" ]; then
        warn "Skipping Data partition population on macOS ${DATA_FS}; copy ISO files into the Data partition from Linux."
        if [ "${#IMAGE_INSTALL_FILES[@]}" -gt 0 ]; then
            warn "--image files were not copied because macOS cannot write-mount ${DATA_FS} here."
        fi
        return
    fi

    if [ "$DATA_FS" = "ntfs" ] && command_exists ntfs-3g; then
        data_mount="/tmp/nextboot_flash_data"
        run_sudo mkdir -p "$data_mount"
        run_sudo ntfs-3g "$data_part" "$data_mount"
        run_sudo mkdir -p "${data_mount}/ISO"
        copy_boot_images_sudo "$data_mount"
        copy_ventoy_assets_sudo "$data_mount"
        sync
        run_sudo umount "$data_mount"
        return
    fi

    data_mount="$(ensure_macos_mounted "$data_part")"
    run_cmd mkdir -p "${data_mount}/ISO"
    copy_boot_images "$data_mount"
    copy_ventoy_assets "$data_mount"
    sync
    run_cmd diskutil unmount "$data_part"
}

populate_linux_media() {
    local esp_part
    local esp_mount="/tmp/nextboot_flash_esp"
    local data_mount="/tmp/nextboot_flash_data"

    esp_part="$(linux_partition_path "$DEVICE" 1)"
    run_sudo mkdir -p "$esp_mount"
    run_sudo mount "$esp_part" "$esp_mount"
    copy_efi_tree_sudo "$esp_mount"

    if [ "$LAYOUT" = "split" ]; then
        populate_linux_data_partition "$data_mount"
    else
        run_sudo mkdir -p "${esp_mount}/ISO"
        copy_boot_images_sudo "$esp_mount"
        copy_ventoy_assets_sudo "$esp_mount"
        sync
    fi

    run_sudo umount "$esp_mount"
}

populate_linux_data_partition() {
    local data_mount="$1"
    local data_part

    data_part="$(linux_partition_path "$DEVICE" 2)"
    run_sudo mkdir -p "$data_mount"
    run_sudo mount "$data_part" "$data_mount"
    run_sudo mkdir -p "${data_mount}/ISO"
    copy_boot_images_sudo "$data_mount"
    copy_ventoy_assets_sudo "$data_mount"
    sync
    run_sudo umount "$data_mount"
}
