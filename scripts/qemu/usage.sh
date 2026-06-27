# Usage text for scripts/run-qemu.sh.

usage() {
    cat <<USAGE
NextBoot QEMU Test

Usage:
  $0 [debug|release] [options]

Options:
  --mode MODE        Build mode: debug or release
  --bus BUS          Storage bus: virtio, nvme, sata, usb, sd
  --image PATH       Copy an ISO/WIM/VHD image into /ISO (repeatable)
  --disk-size MB     GPT disk image size in MiB (default: 256, 512 for 4K, 1024 for 4K split)
  --sector-size BYTES
                     Logical and physical disk sector size: 512 or 4096
  --layout LAYOUT    Disk layout: single or split (default: single)
  --data-fs FS       Data filesystem for split layout: exfat, fat32, ntfs, or udf (default: exfat)
  --disk-image PATH  Output disk image path
  --memory SIZE      QEMU guest memory (default: 1024M)
  --skip-verify      Do not verify the generated GPT/filesystem image
  --smoke            Run QEMU until NextBoot scan/menu log markers appear
  --smoke-boot       With --smoke, press Enter and verify boot preparation starts
  --smoke-efi-iso    Generate a minimal UEFI ISO and verify its loader starts
  --smoke-vlnk-iso   Generate a minimal UEFI ISO behind a Ventoy .vlnk pointer
  --smoke-auto-memdisk
                     Generate a minimal UEFI ISO and force Ventoy auto_memdisk
  --smoke-menu-memdisk
                     Generate a minimal UEFI ISO and press M for menu memdisk mode
  --smoke-windows-iso
                     Generate a Windows-style smoke ISO and verify bootmgfw starts
  --smoke-windows-wimboot
                     Generate a Windows-style smoke ISO and verify WIMBOOT fallback
  --smoke-linux-iso  Generate a Linux-style smoke ISO and verify EFI stub/initrd starts
  --smoke-linux-plugins
                     Generate Linux smoke ISO plus Ventoy plugin payloads
  --smoke-timeout S  Seconds to wait for --smoke markers (default: 20)
  --no-run           Create the disk image and print the QEMU command only
  -h, --help         Show this help

Examples:
  $0 --bus nvme --image ~/Downloads/Win11.iso
  $0 release --bus sata --disk-size 4096
  $0 --bus nvme --sector-size 4096 --no-run
  $0 --bus nvme --layout split --data-fs exfat --image ~/Downloads/Win11.iso
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-efi-iso
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-vlnk-iso
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-linux-iso
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-linux-plugins
  $0 --bus nvme --layout split --data-fs udf --sector-size 4096 --smoke-efi-iso
  $0 --bus nvme --layout split --data-fs ntfs --sector-size 4096 --smoke-windows-wimboot
  $0 --bus usb --no-run
  $0 --bus sd --layout split --data-fs fat32 --smoke-efi-iso
USAGE
}
