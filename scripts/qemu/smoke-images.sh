# Generated smoke image helpers for scripts/run-qemu.sh.

create_smoke_raw_disk() {
    output="$1"
    require_command python3 "python3 is required to create the smoke raw disk image"
    python3 "${SCRIPT_DIR}/qemu/create-disk-image.py" \
        "$output" 64 512 single exfat "$SMOKE_EFI_FILE" \
        0 0 0 "" "" 0 "$EFI_BOOT_NAME"
}

create_generated_smoke_images() {
    if [ "$SMOKE_EFI_ISO" -eq 1 ] || [ "$SMOKE_RAW_IMG" -eq 1 ] || [ "$SMOKE_FIXED_VHD" -eq 1 ] || [ "$SMOKE_DYNAMIC_VHD" -eq 1 ] || [ "$SMOKE_VHDX" -eq 1 ] || [ "$SMOKE_VDI" -eq 1 ]; then
        SMOKE_EFI_FILE="${PROJECT_DIR}/target/${TARGET}/${BUILD_MODE}/nextboot-smoke-efi.efi"
        SMOKE_HELPER_FILE="$SMOKE_EFI_FILE"
        if [ ! -f "$SMOKE_EFI_FILE" ]; then
            die "Smoke EFI file not found: ${SMOKE_EFI_FILE}. Run ./scripts/build.sh ${BUILD_MODE} first."
        fi
    fi

    if [ "$SMOKE_EFI_ISO" -eq 1 ]; then
        SMOKE_ISO_PROFILE="generic"
        SMOKE_ISO_BASENAME="nextboot-smoke-${SMOKE_ARCH_TAG}${SMOKE_ARTIFACT_SUFFIX}-efi.iso"
        if [ "$SMOKE_WINDOWS_ISO" -eq 1 ]; then
            SMOKE_ISO_PROFILE="windows"
            SMOKE_ISO_BASENAME="nextboot-smoke-${SMOKE_ARCH_TAG}${SMOKE_ARTIFACT_SUFFIX}-windows.iso"
        fi
        if [ "$SMOKE_WINDOWS_WIMBOOT" -eq 1 ]; then
            SMOKE_ISO_PROFILE="windows-wimboot"
            SMOKE_ISO_BASENAME="nextboot-smoke-${SMOKE_ARCH_TAG}${SMOKE_ARTIFACT_SUFFIX}-windows-wimboot.iso"
        fi
        if [ "$SMOKE_LINUX_ISO" -eq 1 ]; then
            SMOKE_ISO_PROFILE="linux"
            SMOKE_ISO_BASENAME="nextboot-smoke-${SMOKE_ARCH_TAG}${SMOKE_ARTIFACT_SUFFIX}-linux.iso"
        fi
        SMOKE_ISO_FILE="${PROJECT_DIR}/target/${SMOKE_ISO_BASENAME}"
        require_command python3 "python3 is required to create the smoke ISO"
        warn "Creating ${SMOKE_ISO_PROFILE} UEFI smoke ISO..."
        python3 "${SCRIPT_DIR}/create-smoke-iso.py" \
            --profile "$SMOKE_ISO_PROFILE" \
            --efi "$SMOKE_EFI_FILE" \
            --boot-file-name "$EFI_BOOT_NAME" \
            "$SMOKE_ISO_FILE"
        IMAGES=("$SMOKE_ISO_FILE" "${IMAGES[@]}")
    fi

    if [ "$SMOKE_RAW_IMG" -eq 1 ] || [ "$SMOKE_FIXED_VHD" -eq 1 ] || [ "$SMOKE_DYNAMIC_VHD" -eq 1 ] || [ "$SMOKE_VHDX" -eq 1 ] || [ "$SMOKE_VDI" -eq 1 ]; then
        SMOKE_RAW_IMG_FILE="${PROJECT_DIR}/target/nextboot-smoke-${SMOKE_ARCH_TAG}${SMOKE_ARTIFACT_SUFFIX}-raw.img"
        warn "Creating raw GPT/FAT32 smoke disk image..."
        create_smoke_raw_disk "$SMOKE_RAW_IMG_FILE"
    fi

    if [ "$SMOKE_RAW_IMG" -eq 1 ]; then
        IMAGES=("$SMOKE_RAW_IMG_FILE" "${IMAGES[@]}")
    fi

    if [ "$SMOKE_FIXED_VHD" -eq 1 ]; then
        SMOKE_VHD_FILE="${PROJECT_DIR}/target/nextboot-smoke-${SMOKE_ARCH_TAG}${SMOKE_ARTIFACT_SUFFIX}-fixed.vhd"
        require_command python3 "python3 is required to create the smoke fixed VHD"
        warn "Wrapping smoke disk image as fixed VHD..."
        python3 "${SCRIPT_DIR}/create-smoke-vhd.py" --format fixed "$SMOKE_RAW_IMG_FILE" "$SMOKE_VHD_FILE"
        IMAGES=("$SMOKE_VHD_FILE" "${IMAGES[@]}")
    fi

    if [ "$SMOKE_DYNAMIC_VHD" -eq 1 ]; then
        SMOKE_VHD_FILE="${PROJECT_DIR}/target/nextboot-smoke-${SMOKE_ARCH_TAG}${SMOKE_ARTIFACT_SUFFIX}-dynamic.vhd"
        require_command python3 "python3 is required to create the smoke dynamic VHD"
        warn "Wrapping smoke disk image as dynamic VHD..."
        python3 "${SCRIPT_DIR}/create-smoke-vhd.py" --format dynamic "$SMOKE_RAW_IMG_FILE" "$SMOKE_VHD_FILE"
        IMAGES=("$SMOKE_VHD_FILE" "${IMAGES[@]}")
    fi

    if [ "$SMOKE_VHDX" -eq 1 ]; then
        SMOKE_VHDX_FILE="${PROJECT_DIR}/target/nextboot-smoke-${SMOKE_ARCH_TAG}${SMOKE_ARTIFACT_SUFFIX}.vhdx"
        VHDX_ARGS=()
        if [ "$SMOKE_SPARSE_VHDX" -eq 1 ]; then
            SMOKE_VHDX_FILE="${PROJECT_DIR}/target/nextboot-smoke-${SMOKE_ARCH_TAG}${SMOKE_ARTIFACT_SUFFIX}-sparse.vhdx"
            VHDX_ARGS+=(--sparse)
        fi
        if [ "$SMOKE_PARTIAL_VHDX" -eq 1 ]; then
            SMOKE_VHDX_FILE="${PROJECT_DIR}/target/nextboot-smoke-${SMOKE_ARCH_TAG}${SMOKE_ARTIFACT_SUFFIX}-partial.vhdx"
            VHDX_ARGS+=(--partial-present)
        fi
        if [ "$SMOKE_PARENT_VHDX" -eq 1 ]; then
            SMOKE_VHDX_FILE="${PROJECT_DIR}/target/nextboot-smoke-${SMOKE_ARCH_TAG}${SMOKE_ARTIFACT_SUFFIX}-parent.vhdx"
            SMOKE_VHDX_PARENT_FILE="${PROJECT_DIR}/target/nextboot-smoke-${SMOKE_ARCH_TAG}${SMOKE_ARTIFACT_SUFFIX}-parent-base.vhdbase"
            VHDX_ARGS+=(--parent-required --parent-path "$(basename "$SMOKE_VHDX_PARENT_FILE")")
            if [ "$SMOKE_PARENT_PARTIAL_VHDX" -eq 1 ]; then
                SMOKE_VHDX_FILE="${PROJECT_DIR}/target/nextboot-smoke-${SMOKE_ARCH_TAG}${SMOKE_ARTIFACT_SUFFIX}-parent-partial.vhdx"
                VHDX_ARGS+=(--partial-parent-mix)
            fi
        fi
        require_command python3 "python3 is required to create the smoke VHDX"
        warn "Wrapping smoke disk image as VHDX..."
        if [ "$SMOKE_PARENT_VHDX" -eq 1 ]; then
            warn "Wrapping smoke disk image as same-volume VHDX parent..."
            python3 "${SCRIPT_DIR}/create-smoke-vhdx.py" "$SMOKE_RAW_IMG_FILE" "$SMOKE_VHDX_PARENT_FILE"
            SUPPORT_IMAGES=("$SMOKE_VHDX_PARENT_FILE" "${SUPPORT_IMAGES[@]}")
        fi
        python3 "${SCRIPT_DIR}/create-smoke-vhdx.py" "${VHDX_ARGS[@]}" "$SMOKE_RAW_IMG_FILE" "$SMOKE_VHDX_FILE"
        IMAGES=("$SMOKE_VHDX_FILE" "${IMAGES[@]}")
    fi

    if [ "$SMOKE_VDI" -eq 1 ]; then
        SMOKE_VDI_FILE="${PROJECT_DIR}/target/nextboot-smoke-${SMOKE_ARCH_TAG}${SMOKE_ARTIFACT_SUFFIX}-dynamic.vdi"
        VDI_ARGS=()
        if [ "$SMOKE_STATIC_VDI" -eq 1 ]; then
            SMOKE_VDI_FILE="${PROJECT_DIR}/target/nextboot-smoke-${SMOKE_ARCH_TAG}${SMOKE_ARTIFACT_SUFFIX}-static.vdi"
            VDI_ARGS+=(--format static)
        fi
        if [ "$SMOKE_SPARSE_VDI" -eq 1 ]; then
            SMOKE_VDI_FILE="${PROJECT_DIR}/target/nextboot-smoke-${SMOKE_ARCH_TAG}${SMOKE_ARTIFACT_SUFFIX}-sparse.vdi"
            VDI_ARGS+=(--sparse-mode unallocated)
        fi
        if [ "$SMOKE_DISCARDED_VDI" -eq 1 ]; then
            SMOKE_VDI_FILE="${PROJECT_DIR}/target/nextboot-smoke-${SMOKE_ARCH_TAG}${SMOKE_ARTIFACT_SUFFIX}-discarded.vdi"
            VDI_ARGS+=(--sparse-mode discarded)
        fi
        if [ "$SMOKE_PARENT_VDI" -eq 1 ]; then
            SMOKE_VDI_FILE="${PROJECT_DIR}/target/nextboot-smoke-${SMOKE_ARCH_TAG}${SMOKE_ARTIFACT_SUFFIX}-parent.vdi"
            VDI_ARGS+=(--format differencing --sparse-mode unallocated)
        fi
        require_command python3 "python3 is required to create the smoke VDI"
        warn "Wrapping smoke disk image as VDI..."
        python3 "${SCRIPT_DIR}/create-smoke-vdi.py" "${VDI_ARGS[@]}" "$SMOKE_RAW_IMG_FILE" "$SMOKE_VDI_FILE"
        IMAGES=("$SMOKE_VDI_FILE" "${IMAGES[@]}")
    fi
}
