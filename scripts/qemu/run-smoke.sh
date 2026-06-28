# QEMU smoke expectation runner for scripts/run-qemu.sh.

run_qemu_smoke() {
    SMOKE_SCRIPT="${SCRIPT_DIR}/qemu-boot-smoke.py"
    [ -f "$SMOKE_SCRIPT" ] || die "QEMU smoke runner not found: ${SMOKE_SCRIPT}"
    SMOKE_ISO_MENU_PATH="/ISO/${SMOKE_ISO_BASENAME:-nextboot-smoke-efi.iso}"
    EXPECT_ARGS=(
        --expect "NextBoot v"
        --expect "Phase 2: Scanning for ISO files"
    )
    if [ "${#IMAGES[@]}" -gt 0 ]; then
        EXPECT_ARGS+=(--expect "Found ${#IMAGES[@]} ISO file(s)")
        if [ "$SMOKE_VLNK_ISO" -eq 1 ]; then
            EXPECT_ARGS+=(
                --expect "Resolved Ventoy VLNK /ISO/nextboot-smoke-vlnk.vlnk.iso -> /ventoy/vlnk-target.iso"
                --expect "nextboot-smoke-vlnk.vlnk.iso"
            )
        else
            for image in "${IMAGES[@]}"; do
                EXPECT_ARGS+=(--expect "$(basename "$image")")
            done
        fi
        EXPECT_ARGS+=(--expect "Phase 3: Displaying boot menu")
        if [ "$SMOKE_BOOT" -eq 1 ]; then
            EXPECT_ARGS+=(
                --send-after "Phase 3: Displaying boot menu"
                --expect "Selected:"
                --expect "Phase 4: Booting selected ISO"
                --expect "Preparing to boot:"
                --expect "Creating virtual Block IO"
            )
            if [ "$SMOKE_PARENT_VHDX" -eq 0 ]; then
                EXPECT_ARGS+=(--expect "Virtual Block IO installed")
            fi
            if [ "$SMOKE_MENU_MEMDISK" -eq 1 ]; then
                EXPECT_ARGS+=(
                    --send-text "m"
                    --expect "Manual Ventoy memdisk mode requested for ${SMOKE_ISO_MENU_PATH}"
                )
            else
                EXPECT_ARGS+=(--send-key enter)
            fi
            if [ "$SMOKE_EFI_ISO" -eq 1 ]; then
                EXPECT_ARGS+=(
                    --expect "Using EFI El Torito boot image"
                )
                if [ "$SMOKE_AUTO_MEMDISK" -eq 1 ] || [ "$SMOKE_MENU_MEMDISK" -eq 1 ]; then
                    EXPECT_ARGS+=(
                        --expect "Using Ventoy auto_memdisk for ${SMOKE_ISO_MENU_PATH}"
                    )
                fi
                if [ "$SMOKE_WINDOWS_ISO" -eq 1 ]; then
                    if [ "$SMOKE_WINDOWS_WIMBOOT" -eq 1 ]; then
                        EXPECT_ARGS+=(
                            --expect "device_type: DvdRom"
                            --expect "Booting Windows ISO"
                            --expect "Windows default EFI chain-load paths failed"
                            --expect "Loaded compressed WIMBOOT helper /ventoy/wimboot.x86_64"
                            --expect "Prepared Windows ISO WIMBOOT fallback"
                            --expect "pfsize=0x"
                            --expect "pfread=0x"
                            --expect "Chain loading: /ventoy/wimboot.x86_64"
                            --expect "Loaded chained EFI image"
                        )
                    else
                        EXPECT_ARGS+=(
                            --expect "device_type: DvdRom"
                            --expect "Booting Windows ISO"
                            --expect "Chain loading: /efi/microsoft/boot/bootmgfw.efi"
                            --expect "Loaded chained EFI image"
                        )
                    fi
                elif [ "$SMOKE_LINUX_ISO" -eq 1 ]; then
                    EXPECT_ARGS+=(
                        --expect "Booting Linux ISO"
                        --expect "Using distro Linux defaults: kernel=/boot/vmlinuz initrd=/boot/initrd.img"
                        --expect "Kernel: /boot/vmlinuz"
                        --expect "Initrd: /boot/initrd.img"
                        --expect "Loaded Linux kernel:"
                        --expect "Loaded initrd:"
                        --expect "Prepared Linux EFI stub:"
                        --expect "Registered Linux EFI initrd LoadFile2 provider:"
                        --expect "Trying Linux EFI stub EFI loader path: /boot/vmlinuz"
                        --expect "Loaded EFI image"
                    )
                    if [ "$SMOKE_LINUX_PLUGINS" -eq 1 ]; then
                        EXPECT_ARGS+=(
                            --expect "Mapped Ventoy persistence backend /persistence/nextboot-linux.dat"
                            --expect "auto_install=true"
                            --expect "persistence=1"
                            --expect "injection=true"
                            --expect "dud_files=1"
                        )
                    fi
                else
                    EXPECT_ARGS+=(--expect "Loaded EFI image")
                fi
                EXPECT_ARGS+=(--expect "NEXTBOOT_SMOKE_EFI_STARTED")
            elif [ "$SMOKE_RAW_IMG" -eq 1 ] || [ "$SMOKE_FIXED_VHD" -eq 1 ] || [ "$SMOKE_DYNAMIC_VHD" -eq 1 ] || [ "$SMOKE_VHDX" -eq 1 ] || [ "$SMOKE_VDI" -eq 1 ]; then
                if [ "$SMOKE_PARENT_VHDX" -eq 1 ]; then
                    EXPECT_ARGS+=(
                        --expect "requires an unsupported parent chain"
                        --expect "Boot failed: Error { status: UNSUPPORTED"
                    )
                else
                    EXPECT_ARGS+=(
                        --expect "Found virtual disk filesystem partition"
                        --expect "Trying virtual disk partition EFI loader path"
                        --expect "Loaded EFI image"
                        --expect "NEXTBOOT_SMOKE_EFI_STARTED"
                    )
                fi
            fi
        fi
    else
        EXPECT_ARGS+=(--expect "No ISO files found")
    fi
    warn "Running QEMU boot smoke for ${SMOKE_TIMEOUT}s..."
    python3 "$SMOKE_SCRIPT" --timeout "$SMOKE_TIMEOUT" "${EXPECT_ARGS[@]}" -- \
        "$QEMU_BINARY" "${QEMU_OPTS[@]}"
}
