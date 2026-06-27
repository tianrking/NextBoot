# QEMU storage-device helpers for scripts/run-qemu.sh.

qemu_storage_buses() {
    printf 'virtio, nvme, sata, usb, sd'
}

validate_qemu_storage_bus() {
    case "$1" in
        virtio|nvme|sata|usb|sd) ;;
        *) die "Invalid bus '$1'. Use $(qemu_storage_buses)." ;;
    esac
}

validate_qemu_bus_sector_size() {
    bus="$1"
    sector_size="$2"

    if [ "$bus" = "sd" ] && [ "$sector_size" -ne 512 ]; then
        die "--bus sd currently supports only --sector-size 512 because QEMU sd-card does not expose logical_block_size overrides"
    fi
    if [ "$bus" = "sata" ] && [ "$sector_size" -ne 512 ]; then
        die "--bus sata currently supports only --sector-size 512 because QEMU ide-hd requires 512-byte discard granularity"
    fi
}

append_qemu_storage_device() {
    bus="$1"
    disk_img="$2"
    sector_size="$3"

    device_block_opts=""
    device_discard_opts=""
    if [ "$sector_size" -ne 512 ]; then
        device_block_opts=",logical_block_size=${sector_size},physical_block_size=${sector_size}"
        device_discard_opts="${device_block_opts},discard_granularity=${sector_size}"
    fi

    case "$bus" in
        virtio)
            QEMU_OPTS+=(
                -drive "if=none,id=nextboot_disk,format=raw,file=${disk_img}"
                -device "virtio-blk-pci,drive=nextboot_disk,bootindex=1${device_discard_opts:-$device_block_opts}"
            )
            ;;
        nvme)
            QEMU_OPTS+=(
                -drive "if=none,id=nextboot_disk,format=raw,file=${disk_img}"
                -device "nvme,drive=nextboot_disk,serial=NEXTBOOT0,bootindex=1${device_block_opts}"
            )
            ;;
        sata)
            QEMU_OPTS+=(
                -device "ahci,id=ahci0"
                -drive "if=none,id=nextboot_disk,format=raw,file=${disk_img}"
                -device "ide-hd,drive=nextboot_disk,bus=ahci0.0,bootindex=1${device_block_opts}"
            )
            ;;
        usb)
            QEMU_OPTS+=(
                -device "qemu-xhci,id=xhci"
                -drive "if=none,id=nextboot_disk,format=raw,file=${disk_img}"
                -device "usb-storage,drive=nextboot_disk,bootindex=1${device_discard_opts:-$device_block_opts}"
            )
            ;;
        sd)
            QEMU_OPTS+=(
                -device "sdhci-pci,id=sdhci0"
                -drive "if=none,id=nextboot_disk,format=raw,file=${disk_img}"
                -device "sd-card,drive=nextboot_disk"
            )
            ;;
    esac
}
