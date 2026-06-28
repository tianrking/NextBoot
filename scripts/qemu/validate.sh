# Argument validation and default sizing for scripts/run-qemu.sh.

validate_qemu_args() {
    case "$BUILD_MODE" in
        debug|release) ;;
        *) die "Invalid build mode: ${BUILD_MODE}" ;;
    esac

    validate_qemu_storage_bus "$BUS"

    case "$DISK_SIZE_MB" in
        ''|*[!0-9]*) die "--disk-size must be an integer MiB value" ;;
    esac

    case "$SECTOR_SIZE" in
        512|4096) ;;
        *) die "--sector-size must be 512 or 4096" ;;
    esac
    validate_qemu_bus_sector_size "$BUS" "$SECTOR_SIZE"

    case "$SMOKE_TIMEOUT" in
        ''|*[!0-9]*) die "--smoke-timeout must be an integer second value" ;;
    esac
    case "$SMOKE_PARENT_CHAIN_DEPTH" in
        ''|*[!0-9]*) die "--smoke-parent-chain-depth must be an integer parent count" ;;
    esac

    case "$LAYOUT" in
        single|split) ;;
        *) die "--layout must be single or split" ;;
    esac

    case "$DATA_FS" in
        btrfs|exfat|ext2|ext3|ext4|fat32|ntfs|udf|xfs) ;;
        *) die "--data-fs must be btrfs, exfat, ext2, ext3, ext4, fat32, ntfs, udf, or xfs" ;;
    esac

    if [[ "$DATA_FS" == ext* ]] && [ "$SECTOR_SIZE" -ne 4096 ]; then
        die "--data-fs ext2/ext3/ext4 currently requires --sector-size 4096 in the QEMU generator"
    fi

    if [ "$LAYOUT" = "single" ] && [ "$DATA_FS" != "exfat" ]; then
        warn "--data-fs is ignored for single layout"
    fi

    validate_qemu_smoke_args
    apply_qemu_disk_size_defaults
}

validate_qemu_smoke_args() {
    if [ "$SMOKE" -eq 1 ] && [ "$NO_RUN" -eq 1 ] && [ "$SMOKE_EFI_ISO" -eq 0 ] && [ "$SMOKE_RAW_IMG" -eq 0 ] && [ "$SMOKE_FIXED_VHD" -eq 0 ] && [ "$SMOKE_DYNAMIC_VHD" -eq 0 ] && [ "$SMOKE_VHDX" -eq 0 ] && [ "$SMOKE_VDI" -eq 0 ]; then
        die "--smoke without a generated smoke image cannot be combined with --no-run"
    fi

    if [ "$BUS" = "sd" ] && [ "$SMOKE" -eq 1 ] && [ "$NO_RUN" -eq 0 ] && [ "${NEXTBOOT_QEMU_SD_BOOT_SMOKE:-0}" != "1" ]; then
        die "--bus sd boot smoke is experimental with current QEMU/EDK2 firmware; use --no-run for SD image/filesystem verification, or set NEXTBOOT_QEMU_SD_BOOT_SMOKE=1 to force the experimental boot attempt"
    fi

    if [ "$SMOKE_WINDOWS_ISO" -eq 1 ] && [ "$SMOKE_LINUX_ISO" -eq 1 ]; then
        die "--smoke-windows-iso and --smoke-linux-iso cannot be combined"
    fi

    if { [ "$SMOKE_RAW_IMG" -eq 1 ] || [ "$SMOKE_FIXED_VHD" -eq 1 ] || [ "$SMOKE_DYNAMIC_VHD" -eq 1 ] || [ "$SMOKE_VHDX" -eq 1 ] || [ "$SMOKE_VDI" -eq 1 ]; } && [ "$SMOKE_EFI_ISO" -eq 1 ]; then
        die "--smoke-raw-img/--smoke-vhd/--smoke-dynamic-vhd/--smoke-vhdx/--smoke-vdi cannot be combined with ISO smoke generators"
    fi

    VHDX_VARIANT_COUNT=$((SMOKE_SPARSE_VHDX + SMOKE_PARTIAL_VHDX + SMOKE_PARENT_VHDX))
    if [ "$VHDX_VARIANT_COUNT" -gt 1 ] && [ "$SMOKE_PARENT_VHDX" -eq 0 ]; then
        die "--smoke-sparse-vhdx and --smoke-partial-vhdx cannot be combined"
    fi
    if [ "$SMOKE_PARENT_PARTIAL_VHDX" -eq 1 ] && [ "$SMOKE_SPARSE_VHDX" -eq 1 ]; then
        die "--smoke-parent-partial-vhdx and --smoke-sparse-vhdx cannot be combined"
    fi
    if [ "$SMOKE_PARENT_VHDX" -eq 1 ] && [ "$SMOKE_SPARSE_VHDX" -eq 1 ] && [ "$SMOKE_PARTIAL_VHDX" -eq 1 ]; then
        die "--smoke-parent-vhdx and --smoke-parent-partial-vhdx cannot be combined"
    fi
    if { [ "$SMOKE_PARENT_CHAIN_VHDX" -eq 1 ] || [ "$SMOKE_PARENT_CHAIN_VDI" -eq 1 ]; } && { [ "$SMOKE_PARENT_CHAIN_DEPTH" -lt 2 ] || [ "$SMOKE_PARENT_CHAIN_DEPTH" -gt 8 ]; }; then
        die "--smoke-parent-chain-depth must be between 2 and 8"
    fi

    VDI_VARIANT_COUNT=$((SMOKE_STATIC_VDI + SMOKE_SPARSE_VDI + SMOKE_DISCARDED_VDI + SMOKE_PARENT_VDI))
    if [ "$VDI_VARIANT_COUNT" -gt 1 ]; then
        die "--smoke-static-vdi, --smoke-sparse-vdi, --smoke-discarded-vdi, and --smoke-parent-vdi are mutually exclusive"
    fi

    SMOKE_DISK_IMAGE_COUNT=$((SMOKE_RAW_IMG + SMOKE_FIXED_VHD + SMOKE_DYNAMIC_VHD + SMOKE_VHDX + SMOKE_VDI))
    if [ "$SMOKE_DISK_IMAGE_COUNT" -gt 1 ]; then
        die "--smoke-raw-img, --smoke-vhd, --smoke-dynamic-vhd, --smoke-vhdx, and --smoke-vdi are mutually exclusive"
    fi

    if [ "$SMOKE_BOOT" -eq 1 ] && [ "$SMOKE_EFI_ISO" -eq 0 ] && [ "$SMOKE_RAW_IMG" -eq 0 ] && [ "$SMOKE_FIXED_VHD" -eq 0 ] && [ "$SMOKE_DYNAMIC_VHD" -eq 0 ] && [ "$SMOKE_VHDX" -eq 0 ] && [ "$SMOKE_VDI" -eq 0 ] && [ "${#IMAGES[@]}" -eq 0 ]; then
        die "--smoke-boot requires at least one --image"
    fi
}

apply_qemu_disk_size_defaults() {
    if [ "$DISK_SIZE_SET" -eq 0 ]; then
        if [ "$SECTOR_SIZE" -eq 4096 ]; then
            if [ "$LAYOUT" = "split" ]; then
                DISK_SIZE_MB=1024
            else
                DISK_SIZE_MB=512
            fi
        fi
    fi

    MIN_DISK_SIZE_MB=64
    if [ "$LAYOUT" = "split" ]; then
        MIN_DISK_SIZE_MB=128
    fi
    if [ "$SECTOR_SIZE" -eq 4096 ]; then
        MIN_DISK_SIZE_MB=260
        if [ "$LAYOUT" = "split" ]; then
            MIN_DISK_SIZE_MB=544
        fi
    fi
    if [ "$DISK_SIZE_MB" -lt "$MIN_DISK_SIZE_MB" ]; then
        die "--disk-size must be at least ${MIN_DISK_SIZE_MB} MiB for ${LAYOUT} layout with ${SECTOR_SIZE}B sectors"
    fi
}
