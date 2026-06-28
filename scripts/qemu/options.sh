# Option parsing for scripts/run-qemu.sh.

parse_qemu_args() {
    while [ "$#" -gt 0 ]; do
        case "$1" in
            debug|release)
                BUILD_MODE="$1"
                shift
                ;;
            --mode)
                [ $# -ge 2 ] || die "--mode requires a value"
                BUILD_MODE="$2"
                shift 2
                ;;
            --bus)
                [ $# -ge 2 ] || die "--bus requires a value"
                BUS="$2"
                shift 2
                ;;
            --image)
                [ $# -ge 2 ] || die "--image requires a path"
                IMAGES+=("$2")
                shift 2
                ;;
            --disk-size)
                [ $# -ge 2 ] || die "--disk-size requires a value"
                DISK_SIZE_MB="$2"
                DISK_SIZE_SET=1
                shift 2
                ;;
            --sector-size)
                [ $# -ge 2 ] || die "--sector-size requires a value"
                SECTOR_SIZE="$2"
                shift 2
                ;;
            --layout)
                [ $# -ge 2 ] || die "--layout requires a value"
                LAYOUT="$2"
                shift 2
                ;;
            --data-fs)
                [ $# -ge 2 ] || die "--data-fs requires a value"
                DATA_FS="$2"
                shift 2
                ;;
            --disk-image)
                [ $# -ge 2 ] || die "--disk-image requires a path"
                DISK_IMG="$2"
                shift 2
                ;;
            --memory)
                [ $# -ge 2 ] || die "--memory requires a value"
                MEMORY="$2"
                shift 2
                ;;
            --skip-verify)
                VERIFY_IMAGE=0
                shift
                ;;
            --smoke)
                SMOKE=1
                shift
                ;;
            --smoke-boot)
                SMOKE=1
                SMOKE_BOOT=1
                shift
                ;;
            --smoke-efi-iso)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_EFI_ISO=1
                shift
                ;;
            --smoke-vlnk-iso)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_EFI_ISO=1
                SMOKE_VLNK_ISO=1
                shift
                ;;
            --smoke-raw-img)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_RAW_IMG=1
                shift
                ;;
            --smoke-vhd)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_FIXED_VHD=1
                shift
                ;;
            --smoke-dynamic-vhd)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_DYNAMIC_VHD=1
                shift
                ;;
            --smoke-vhdx)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_VHDX=1
                shift
                ;;
            --smoke-sparse-vhdx)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_VHDX=1
                SMOKE_SPARSE_VHDX=1
                shift
                ;;
            --smoke-partial-vhdx)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_VHDX=1
                SMOKE_PARTIAL_VHDX=1
                shift
                ;;
            --smoke-parent-vhdx)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_VHDX=1
                SMOKE_SPARSE_VHDX=1
                SMOKE_PARENT_VHDX=1
                shift
                ;;
            --smoke-parent-chain-vhdx)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_VHDX=1
                SMOKE_SPARSE_VHDX=1
                SMOKE_PARENT_VHDX=1
                SMOKE_PARENT_CHAIN_VHDX=1
                shift
                ;;
            --smoke-missing-parent-vhdx)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_VHDX=1
                SMOKE_SPARSE_VHDX=1
                SMOKE_PARENT_VHDX=1
                SMOKE_MISSING_PARENT_VHDX=1
                shift
                ;;
            --smoke-parent-partial-vhdx)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_VHDX=1
                SMOKE_PARTIAL_VHDX=1
                SMOKE_PARENT_VHDX=1
                SMOKE_PARENT_PARTIAL_VHDX=1
                shift
                ;;
            --smoke-vdi)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_VDI=1
                shift
                ;;
            --smoke-static-vdi)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_VDI=1
                SMOKE_STATIC_VDI=1
                shift
                ;;
            --smoke-sparse-vdi)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_VDI=1
                SMOKE_SPARSE_VDI=1
                shift
                ;;
            --smoke-discarded-vdi)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_VDI=1
                SMOKE_DISCARDED_VDI=1
                shift
                ;;
            --smoke-parent-vdi)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_VDI=1
                SMOKE_PARENT_VDI=1
                shift
                ;;
            --smoke-parent-chain-vdi)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_VDI=1
                SMOKE_PARENT_VDI=1
                SMOKE_PARENT_CHAIN_VDI=1
                shift
                ;;
            --smoke-missing-parent-vdi)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_VDI=1
                SMOKE_PARENT_VDI=1
                SMOKE_MISSING_PARENT_VDI=1
                shift
                ;;
            --smoke-parent-chain-depth)
                [ $# -ge 2 ] || die "--smoke-parent-chain-depth requires a value"
                SMOKE_PARENT_CHAIN_DEPTH="$2"
                shift 2
                ;;
            --smoke-auto-memdisk)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_EFI_ISO=1
                SMOKE_AUTO_MEMDISK=1
                shift
                ;;
            --smoke-menu-memdisk)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_EFI_ISO=1
                SMOKE_MENU_MEMDISK=1
                shift
                ;;
            --smoke-windows-iso)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_EFI_ISO=1
                SMOKE_WINDOWS_ISO=1
                shift
                ;;
            --smoke-windows-wimboot)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_EFI_ISO=1
                SMOKE_WINDOWS_ISO=1
                SMOKE_WINDOWS_WIMBOOT=1
                shift
                ;;
            --smoke-linux-iso)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_EFI_ISO=1
                SMOKE_LINUX_ISO=1
                shift
                ;;
            --smoke-linux-plugins)
                SMOKE=1
                SMOKE_BOOT=1
                SMOKE_EFI_ISO=1
                SMOKE_LINUX_ISO=1
                SMOKE_LINUX_PLUGINS=1
                shift
                ;;
            --smoke-timeout)
                [ $# -ge 2 ] || die "--smoke-timeout requires a value"
                SMOKE_TIMEOUT="$2"
                shift 2
                ;;
            --no-run)
                NO_RUN=1
                shift
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            *)
                die "Unknown argument: $1"
                ;;
        esac
    done
}
