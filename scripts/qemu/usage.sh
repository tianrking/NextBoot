# Usage text for scripts/run-qemu.sh.

usage() {
    cat <<USAGE
NextBoot QEMU Test

Usage:
  $0 [debug|release] [options]

Options:
  TARGET=TRIPLE     UEFI target: x86_64-unknown-uefi, i686-unknown-uefi, or aarch64-unknown-uefi
  --mode MODE        Build mode: debug or release
  --bus BUS          Storage bus: virtio, nvme, sata, usb, sd
  --image PATH       Copy an ISO/WIM/VHD image into /ISO (repeatable)
  --disk-size MB     GPT disk image size in MiB (default: 256, 512 for 4K, 1024 for 4K split)
  --sector-size BYTES
                     Logical and physical disk sector size: 512 or 4096
  --layout LAYOUT    Disk layout: single or split (default: single)
  --data-fs FS       Data filesystem for split layout: btrfs, exfat, ext2, ext3, ext4, fat32, ntfs, udf, or xfs (default: exfat)
  --disk-image PATH  Output disk image path
  --memory SIZE      QEMU guest memory (default: 1024M)
  --skip-verify      Do not verify the generated GPT/filesystem image
  --smoke            Run QEMU until NextBoot scan/menu log markers appear
  --smoke-boot       With --smoke, press Enter and verify boot preparation starts
  --smoke-efi-iso    Generate a minimal UEFI ISO and verify its loader starts
  --smoke-vlnk-iso   Generate a minimal UEFI ISO behind a Ventoy .vlnk pointer
  --smoke-raw-img    Generate a bootable raw GPT/FAT32 .img and verify it starts
  --smoke-vhd        Generate a bootable fixed VHD and verify it starts
  --smoke-dynamic-vhd
                     Generate a bootable dynamic VHD and verify it starts
  --smoke-vhdx       Generate a bootable VHDX and verify it starts
  --smoke-sparse-vhdx
                     Generate a sparse bootable VHDX and verify it starts
  --smoke-partial-vhdx
                     Generate a partially-present VHDX with a full sector bitmap
  --smoke-parent-vhdx
                     Generate a sparse VHDX that requires an unsupported parent chain
  --smoke-vdi        Generate a bootable dynamic VDI and verify it starts
  --smoke-static-vdi Generate a bootable static VDI and verify it starts
  --smoke-sparse-vdi Generate a sparse bootable dynamic VDI and verify it starts
  --smoke-discarded-vdi
                     Generate a dynamic VDI with discarded zero blocks
  --smoke-parent-vdi
                     Generate a differencing VDI that requires an unsupported parent chain
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
  TARGET=i686-unknown-uefi $0 --bus virtio --smoke-efi-iso
  TARGET=aarch64-unknown-uefi $0 --bus virtio --smoke-efi-iso
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-vlnk-iso
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-raw-img
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-vhd
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-dynamic-vhd
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-vhdx
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-sparse-vhdx
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-partial-vhdx
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-parent-vhdx
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-vdi
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-static-vdi
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-sparse-vdi
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-discarded-vdi
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-parent-vdi
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-linux-iso
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-linux-plugins
  $0 --bus nvme --layout split --data-fs ext3 --sector-size 4096 --smoke-efi-iso
  $0 --bus nvme --layout split --data-fs ext4 --sector-size 4096 --smoke-efi-iso
  $0 --bus nvme --layout split --data-fs udf --sector-size 4096 --smoke-efi-iso
  $0 --bus nvme --layout split --data-fs xfs --sector-size 4096 --smoke-efi-iso
  $0 --bus nvme --layout split --data-fs btrfs --sector-size 4096 --smoke-efi-iso
  $0 --bus nvme --layout split --data-fs ntfs --sector-size 4096 --smoke-windows-wimboot
  $0 --bus usb --no-run
  $0 --bus sd --layout split --data-fs fat32 --smoke-efi-iso --no-run
  NEXTBOOT_QEMU_SD_BOOT_SMOKE=1 $0 --bus sd --layout split --data-fs fat32 --smoke-efi-iso
USAGE
}
