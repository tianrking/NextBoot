# UEFI target to QEMU firmware mapping for scripts/run-qemu.sh.

configure_qemu_arch() {
    case "$TARGET" in
        x86_64-unknown-uefi)
            EFI_BOOT_NAME="BOOTX64.EFI"
            SMOKE_ARCH_TAG="x64"
            QEMU_BINARY="qemu-system-x86_64"
            QEMU_OPTS=(-machine q35,accel=tcg)
            OVMF_PATHS=(
                "/usr/share/OVMF/OVMF_CODE.fd"
                "/usr/share/ovmf/OVMF.fd"
                "/usr/share/qemu/OVMF.fd"
                "/opt/homebrew/share/qemu/edk2-x86_64-code.fd"
                "/opt/homebrew/opt/qemu/share/qemu/edk2-x86_64-code.fd"
            )
            ;;
        i686-unknown-uefi)
            EFI_BOOT_NAME="BOOTIA32.EFI"
            SMOKE_ARCH_TAG="ia32"
            QEMU_BINARY="qemu-system-i386"
            QEMU_OPTS=(-machine q35,accel=tcg)
            OVMF_PATHS=(
                "/usr/share/OVMF/OVMF32_CODE.fd"
                "/usr/share/ovmf/OVMF32.fd"
                "/usr/share/qemu/edk2-i386-code.fd"
                "/opt/homebrew/share/qemu/edk2-i386-code.fd"
                "/opt/homebrew/opt/qemu/share/qemu/edk2-i386-code.fd"
            )
            ;;
        aarch64-unknown-uefi)
            EFI_BOOT_NAME="BOOTAA64.EFI"
            SMOKE_ARCH_TAG="aa64"
            QEMU_BINARY="qemu-system-aarch64"
            QEMU_OPTS=(-machine virt,accel=tcg -cpu cortex-a72)
            OVMF_PATHS=(
                "/usr/share/AAVMF/AAVMF_CODE.fd"
                "/usr/share/AAVMF/AAVMF_CODE.ms.fd"
                "/usr/share/edk2/aarch64/QEMU_EFI.fd"
                "/usr/share/qemu-efi-aarch64/QEMU_EFI.fd"
                "/usr/share/qemu/edk2-aarch64-code.fd"
                "/opt/homebrew/share/qemu/edk2-aarch64-code.fd"
                "/opt/homebrew/opt/qemu/share/qemu/edk2-aarch64-code.fd"
            )
            ;;
        *)
            die "Unsupported UEFI QEMU target '${TARGET}'. Supported: x86_64-unknown-uefi, i686-unknown-uefi, aarch64-unknown-uefi"
            ;;
    esac
}
