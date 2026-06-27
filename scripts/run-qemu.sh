#!/usr/bin/env bash
# NextBoot QEMU Test Script
#
# Creates GPT test disk images with NextBoot installed as the removable/fallback
# UEFI bootloader.  Split layouts use a FAT32 ESP plus an exFAT, FAT32, or NTFS
# Data partition so fixed-disk, NVMe, SATA, USB, and virtio paths can be tested
# without rewriting real media.
#
# Usage:
#   ./scripts/run-qemu.sh
#   ./scripts/run-qemu.sh release
#   ./scripts/run-qemu.sh --bus nvme --image ~/Downloads/ubuntu.iso
#   ./scripts/run-qemu.sh --bus nvme --sector-size 4096 --no-run
#   ./scripts/run-qemu.sh --bus nvme --layout split --data-fs exfat --image ~/Downloads/ubuntu.iso
#   ./scripts/run-qemu.sh --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-efi-iso
#   ./scripts/run-qemu.sh --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-linux-iso
#   ./scripts/run-qemu.sh --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-linux-plugins
#   ./scripts/run-qemu.sh --bus nvme --layout split --data-fs ntfs --sector-size 4096 --smoke-windows-wimboot
#   ./scripts/run-qemu.sh --bus usb --mode release --no-run

set -eo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
TARGET="x86_64-unknown-uefi"
BUILD_MODE="debug"
BUS="virtio"
DISK_SIZE_MB=256
DISK_SIZE_SET=0
SECTOR_SIZE=512
LAYOUT="single"
DATA_FS="exfat"
DISK_IMG=""
NO_RUN=0
VERIFY_IMAGE=1
SMOKE=0
SMOKE_BOOT=0
SMOKE_EFI_ISO=0
SMOKE_WINDOWS_ISO=0
SMOKE_WINDOWS_WIMBOOT=0
SMOKE_LINUX_ISO=0
SMOKE_LINUX_PLUGINS=0
SMOKE_HELPER_FILE=""
SMOKE_TIMEOUT=20
MEMORY="1024M"
IMAGES=()

usage() {
    cat <<USAGE
NextBoot QEMU Test

Usage:
  $0 [debug|release] [options]

Options:
  --mode MODE        Build mode: debug or release
  --bus BUS          Storage bus: virtio, nvme, sata, usb
  --image PATH       Copy an ISO/WIM/VHD image into /ISO (repeatable)
  --disk-size MB     GPT disk image size in MiB (default: 256, 512 for 4K, 1024 for 4K split)
  --sector-size BYTES
                     Logical and physical disk sector size: 512 or 4096
  --layout LAYOUT    Disk layout: single or split (default: single)
  --data-fs FS       Data filesystem for split layout: exfat, fat32, or ntfs (default: exfat)
  --disk-image PATH  Output disk image path
  --memory SIZE      QEMU guest memory (default: 1024M)
  --skip-verify      Do not verify the generated GPT/filesystem image
  --smoke            Run QEMU until NextBoot scan/menu log markers appear
  --smoke-boot       With --smoke, press Enter and verify boot preparation starts
  --smoke-efi-iso    Generate a minimal UEFI ISO and verify its loader starts
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
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-linux-iso
  $0 --bus nvme --layout split --data-fs exfat --sector-size 4096 --smoke-linux-plugins
  $0 --bus nvme --layout split --data-fs ntfs --sector-size 4096 --smoke-windows-wimboot
  $0 --bus usb --no-run
USAGE
}

die() {
    echo -e "${RED}Error: $*${NC}" >&2
    exit 1
}

info() {
    echo -e "${GREEN}$*${NC}"
}

warn() {
    echo -e "${YELLOW}$*${NC}"
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "$2"
}

while [ $# -gt 0 ]; do
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

case "$BUILD_MODE" in
    debug|release) ;;
    *) die "Invalid build mode: ${BUILD_MODE}" ;;
esac

case "$BUS" in
    virtio|nvme|sata|usb) ;;
    *) die "Invalid bus '${BUS}'. Use virtio, nvme, sata, or usb." ;;
esac

case "$DISK_SIZE_MB" in
    ''|*[!0-9]*) die "--disk-size must be an integer MiB value" ;;
esac

case "$SECTOR_SIZE" in
    512|4096) ;;
    *) die "--sector-size must be 512 or 4096" ;;
esac

case "$SMOKE_TIMEOUT" in
    ''|*[!0-9]*) die "--smoke-timeout must be an integer second value" ;;
esac

case "$LAYOUT" in
    single|split) ;;
    *) die "--layout must be single or split" ;;
esac

case "$DATA_FS" in
    exfat|fat32|ntfs) ;;
    *) die "--data-fs must be exfat, fat32, or ntfs" ;;
esac

if [ "$LAYOUT" = "single" ] && [ "$DATA_FS" != "exfat" ]; then
    warn "--data-fs is ignored for single layout"
fi

if [ "$SMOKE" -eq 1 ] && [ "$NO_RUN" -eq 1 ]; then
    die "--smoke cannot be combined with --no-run"
fi

if [ "$SMOKE_WINDOWS_ISO" -eq 1 ] && [ "$SMOKE_LINUX_ISO" -eq 1 ]; then
    die "--smoke-windows-iso and --smoke-linux-iso cannot be combined"
fi

if [ "$SMOKE_BOOT" -eq 1 ] && [ "$SMOKE_EFI_ISO" -eq 0 ] && [ "${#IMAGES[@]}" -eq 0 ]; then
    die "--smoke-boot requires at least one --image"
fi

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

EFI_FILE="${PROJECT_DIR}/target/${TARGET}/${BUILD_MODE}/nextboot-boot.efi"
if [ ! -f "$EFI_FILE" ]; then
    die "EFI file not found: ${EFI_FILE}. Run ./scripts/build.sh ${BUILD_MODE} first."
fi

if [ "$SMOKE_EFI_ISO" -eq 1 ]; then
    SMOKE_EFI_FILE="${PROJECT_DIR}/target/${TARGET}/${BUILD_MODE}/nextboot-smoke-efi.efi"
    SMOKE_HELPER_FILE="$SMOKE_EFI_FILE"
    SMOKE_ISO_PROFILE="generic"
    SMOKE_ISO_BASENAME="nextboot-smoke-efi.iso"
    if [ "$SMOKE_WINDOWS_ISO" -eq 1 ]; then
        SMOKE_ISO_PROFILE="windows"
        SMOKE_ISO_BASENAME="nextboot-smoke-windows.iso"
    fi
    if [ "$SMOKE_WINDOWS_WIMBOOT" -eq 1 ]; then
        SMOKE_ISO_PROFILE="windows-wimboot"
        SMOKE_ISO_BASENAME="nextboot-smoke-windows-wimboot.iso"
    fi
    if [ "$SMOKE_LINUX_ISO" -eq 1 ]; then
        SMOKE_ISO_PROFILE="linux"
        SMOKE_ISO_BASENAME="nextboot-smoke-linux.iso"
    fi
    SMOKE_ISO_FILE="${PROJECT_DIR}/target/${SMOKE_ISO_BASENAME}"
    if [ ! -f "$SMOKE_EFI_FILE" ]; then
        die "Smoke EFI file not found: ${SMOKE_EFI_FILE}. Run ./scripts/build.sh ${BUILD_MODE} first."
    fi
    require_command python3 "python3 is required to create the smoke ISO"
    warn "Creating ${SMOKE_ISO_PROFILE} UEFI smoke ISO..."
    python3 "${SCRIPT_DIR}/create-smoke-iso.py" \
        --profile "$SMOKE_ISO_PROFILE" \
        --efi "$SMOKE_EFI_FILE" \
        "$SMOKE_ISO_FILE"
    IMAGES=("$SMOKE_ISO_FILE" "${IMAGES[@]}")
fi

for image in "${IMAGES[@]}"; do
    [ -f "$image" ] || die "Image file not found: ${image}"
done

if [ -z "$DISK_IMG" ]; then
    DISK_IMG="${PROJECT_DIR}/target/qemu_${BUS}_${BUILD_MODE}.img"
fi

mkdir -p "$(dirname "$DISK_IMG")"

echo -e "${GREEN}NextBoot QEMU Test${NC}"
echo "=================="
info "EFI file: ${EFI_FILE}"
info "Storage bus: ${BUS}"
info "Sector size: ${SECTOR_SIZE}"
info "Disk layout: ${LAYOUT}"
if [ "$LAYOUT" = "split" ]; then
    info "Data filesystem: ${DATA_FS}"
fi
info "Disk image: ${DISK_IMG}"

require_command python3 "python3 is required to create the GPT disk image"

warn "Creating ${LAYOUT} GPT test disk image..."
PY_ARGS=(
    "$DISK_IMG"
    "$DISK_SIZE_MB"
    "$SECTOR_SIZE"
    "$LAYOUT"
    "$DATA_FS"
    "$EFI_FILE"
    "$SMOKE_LINUX_PLUGINS"
    "$SMOKE_WINDOWS_WIMBOOT"
    "$SMOKE_HELPER_FILE"
)
if [ "${#IMAGES[@]}" -gt 0 ]; then
    PY_ARGS+=("${IMAGES[@]}")
fi
python3 - "${PY_ARGS[@]}" <<'PY'
import math
import lzma
import os
import struct
import sys
import time
import uuid
import zlib

path = sys.argv[1]
size_mb = int(sys.argv[2])
sector_size = int(sys.argv[3])
layout = sys.argv[4]
data_fs = sys.argv[5]
efi_file = sys.argv[6]
smoke_linux_plugins = sys.argv[7] == "1"
smoke_windows_wimboot = sys.argv[8] == "1"
smoke_helper_file = sys.argv[9]
image_files = sys.argv[10:]
if sector_size not in (512, 4096):
    raise SystemExit("sector size must be 512 or 4096")
if layout not in ("single", "split"):
    raise SystemExit("layout must be single or split")
if data_fs not in ("exfat", "fat32", "ntfs"):
    raise SystemExit("data filesystem must be exfat, fat32, or ntfs")
total_bytes = size_mb * 1024 * 1024
if total_bytes % sector_size != 0:
    raise SystemExit("disk size must be aligned to the sector size")
total_sectors = total_bytes // sector_size
last_lba = total_sectors - 1
entry_count = 128
entry_size = 128
entry_array_sectors = math.ceil(entry_count * entry_size / sector_size)
primary_entries_lba = 2
first_usable_lba = primary_entries_lba + entry_array_sectors
backup_entries_lba = last_lba - entry_array_sectors
last_usable_lba = backup_entries_lba - 1
alignment_lba = max(1, 1024 * 1024 // sector_size)

if first_usable_lba >= last_usable_lba:
    raise SystemExit("disk image is too small")

def align_up(value, alignment):
    return ((value + alignment - 1) // alignment) * alignment

def mib_to_sectors(mib):
    return mib * 1024 * 1024 // sector_size

def short_name_checksum(name11):
    checksum = 0
    for byte in name11:
        checksum = (((checksum & 1) << 7) + (checksum >> 1) + byte) & 0xFF
    return checksum

def sanitize_short_component(text):
    allowed = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789$%'-_@~`!(){}^#&"
    out = bytearray()
    for ch in text.upper().encode("ascii", "ignore"):
        out.append(ch if ch in allowed else ord("_"))
    return bytes(out)

def split_name(name):
    if "." in name and not name.startswith("."):
        base, ext = name.rsplit(".", 1)
    else:
        base, ext = name, ""
    return base, ext

def split_virtual_path(path):
    parts = [part for part in path.replace("\\", "/").split("/") if part]
    if not parts:
        raise SystemExit(f"invalid virtual path: {path}")
    return parts

def make_smoke_linux_plugin_files(images):
    linux_image = next(
        (
            os.path.basename(image)
            for image in images
            if "linux" in os.path.basename(image).lower()
        ),
        "nextboot-smoke-linux.iso",
    )
    image_path = f"/ISO/{linux_image}"
    ventoy_json = f"""{{
  "auto_install": [
    {{
      "image": "{image_path}",
      "template": "/ventoy/autoinstall/linux.ks",
      "autosel": 1
    }}
  ],
  "persistence": [
    {{
      "image": "{image_path}",
      "backend": "/persistence/nextboot-linux.dat",
      "autosel": 1
    }}
  ],
  "injection": [
    {{
      "image": "{image_path}",
      "archive": "/ventoy/injection/tools.tar"
    }}
  ],
  "dud": [
    {{
      "image": "{image_path}",
      "dud": ["/ventoy/dud/dd.iso"]
    }}
  ]
}}
""".encode("utf-8")
    persistence_data = b"NEXTBOOT SMOKE PERSISTENCE\n"
    return [
        ("/ventoy/ventoy.json", ventoy_json),
        (
            "/ventoy/autoinstall/linux.ks",
            b"# NextBoot smoke auto-install template\nlang en_US.UTF-8\n",
        ),
        ("/ventoy/injection/tools.tar", b"NEXTBOOT SMOKE INJECTION ARCHIVE\n"),
        ("/ventoy/dud/dd.iso", b"NEXTBOOT SMOKE DUD IMAGE\n"),
        (
            "/persistence/nextboot-linux.dat",
            persistence_data + bytes(8192 - len(persistence_data)),
        ),
    ]

def make_smoke_windows_wimboot_files(helper):
    if not helper:
        raise SystemExit("windows wimboot smoke helper is missing")
    with open(helper, "rb") as src:
        helper_data = src.read()
    compressed = lzma.compress(
        helper_data,
        format=lzma.FORMAT_XZ,
        check=lzma.CHECK_CRC32,
        preset=0,
    )
    return [("/ventoy/wimboot.x86_64.xz", compressed)]

extra_files = []
if smoke_linux_plugins:
    extra_files.extend(make_smoke_linux_plugin_files(image_files))
if smoke_windows_wimboot:
    extra_files.extend(make_smoke_windows_wimboot_files(smoke_helper_file))

def make_short_name(name, used):
    base, ext = split_name(name)
    clean_base = sanitize_short_component(base) or b"FILE"
    clean_ext = sanitize_short_component(ext)

    candidate = clean_base[:8].ljust(8, b" ") + clean_ext[:3].ljust(3, b" ")
    simple_name = clean_base.decode("ascii", "ignore").rstrip()
    simple_ext = clean_ext.decode("ascii", "ignore").rstrip()
    requested = simple_name if not simple_ext else f"{simple_name}.{simple_ext}"
    if requested == name.upper() and candidate not in used:
        used.add(candidate)
        return candidate, False

    stem = clean_base[:6] or b"FILE"
    for index in range(1, 100000):
        suffix = f"~{index}".encode("ascii")
        short_base = (stem[: 8 - len(suffix)] + suffix)[:8]
        candidate = short_base.ljust(8, b" ") + clean_ext[:3].ljust(3, b" ")
        if candidate not in used:
            used.add(candidate)
            return candidate, True
    raise SystemExit(f"cannot allocate short name for {name}")

def lfn_entry(sequence, chunk, checksum):
    values = [0xFFFF] * 13
    for index, codepoint in enumerate(chunk):
        values[index] = codepoint
    if len(chunk) < 13:
        values[len(chunk)] = 0

    entry = bytearray(32)
    entry[0] = sequence
    entry[11] = 0x0F
    entry[13] = checksum
    for i in range(5):
        struct.pack_into("<H", entry, 1 + i * 2, values[i])
    for i in range(6):
        struct.pack_into("<H", entry, 14 + i * 2, values[5 + i])
    for i in range(2):
        struct.pack_into("<H", entry, 28 + i * 2, values[11 + i])
    return bytes(entry)

def directory_entry(name11, attr, first_cluster, size):
    entry = bytearray(32)
    entry[0:11] = name11
    entry[11] = attr
    struct.pack_into("<H", entry, 20, (first_cluster >> 16) & 0xFFFF)
    struct.pack_into("<H", entry, 26, first_cluster & 0xFFFF)
    struct.pack_into("<I", entry, 28, size)
    return bytes(entry)

class Directory:
    def __init__(self, first_cluster):
        self.first_cluster = first_cluster
        self.entries = []
        self.used_short_names = set()

    def add(self, name, attr, first_cluster, size):
        short, needs_lfn = make_short_name(name, self.used_short_names)
        if needs_lfn or name.upper() != short_to_display_name(short):
            checksum = short_name_checksum(short)
            codes = [ord(ch) for ch in name]
            chunks = [codes[i : i + 13] for i in range(0, len(codes), 13)] or [[]]
            for index in range(len(chunks), 0, -1):
                seq = index
                if index == len(chunks):
                    seq |= 0x40
                self.entries.append(lfn_entry(seq, chunks[index - 1], checksum))
        self.entries.append(directory_entry(short, attr, first_cluster, size))

def short_to_display_name(name11):
    base = name11[:8].decode("ascii").rstrip()
    ext = name11[8:].decode("ascii").rstrip()
    return base if not ext else f"{base}.{ext}"

disk_guid = uuid.uuid5(uuid.NAMESPACE_URL, f"nextboot-qemu:{os.path.abspath(path)}").bytes_le
esp_type = uuid.UUID("c12a7328-f81f-11d2-ba4b-00a0c93ec93b").bytes_le
ms_basic_type = uuid.UUID("ebd0a0a2-b9e5-4433-87c0-68b6b72699c7").bytes_le

def fat32_geometry(part_sectors):
    reserved_sectors = 32
    num_fats = 2
    sectors_per_cluster = 1
    fat_size = 1
    while True:
        data_sectors = part_sectors - reserved_sectors - num_fats * fat_size
        if data_sectors <= 0:
            raise SystemExit("partition is too small for FAT32")
        cluster_count = data_sectors // sectors_per_cluster
        required = math.ceil((cluster_count + 2) * 4 / sector_size)
        if required <= fat_size:
            break
        fat_size = required

    if cluster_count < 65525:
        raise SystemExit(
            f"partition is too small for FAT32 with {sector_size}B sectors"
        )

    return reserved_sectors, num_fats, sectors_per_cluster, fat_size, cluster_count

def log2_power_of_two(value):
    if value <= 0 or value & (value - 1):
        raise SystemExit(f"{value} is not a power of two")
    return value.bit_length() - 1

def exfat_geometry(part_sectors):
    boot_region_sectors = 24
    sectors_per_cluster = max(1, 4096 // sector_size)
    fat_offset = boot_region_sectors
    fat_length = 1
    while True:
        cluster_heap_offset = fat_offset + fat_length
        if part_sectors <= cluster_heap_offset:
            raise SystemExit("partition is too small for exFAT")
        cluster_count = (part_sectors - cluster_heap_offset) // sectors_per_cluster
        required = math.ceil((cluster_count + 2) * 4 / sector_size)
        if required <= fat_length:
            break
        fat_length = required
    if cluster_count < 16:
        raise SystemExit("partition is too small for exFAT")
    return fat_offset, fat_length, cluster_heap_offset, cluster_count, sectors_per_cluster

def ntfs_geometry(part_sectors):
    sectors_per_cluster = 1
    cluster_count = part_sectors // sectors_per_cluster
    file_record_size = max(1024, sector_size)
    index_record_size = file_record_size
    if cluster_count < 128:
        raise SystemExit("partition is too small for NTFS")
    return sectors_per_cluster, cluster_count, file_record_size, index_record_size

def make_partition(name, label, fs_type, type_guid, start_lba, end_lba, include_efi, include_images):
    if start_lba < first_usable_lba or end_lba > last_usable_lba or end_lba < start_lba:
        raise SystemExit(f"invalid partition range for {name}")
    part_sectors = end_lba - start_lba + 1
    if fs_type == "fat32":
        fat32_geometry(part_sectors)
    elif fs_type == "exfat":
        exfat_geometry(part_sectors)
    elif fs_type == "ntfs":
        ntfs_geometry(part_sectors)
    else:
        raise SystemExit(f"unsupported test partition filesystem: {fs_type}")
    return {
        "name": name,
        "label": label,
        "fs_type": fs_type,
        "type_guid": type_guid,
        "guid": uuid.uuid5(
            uuid.NAMESPACE_URL,
            f"nextboot-qemu-part:{os.path.abspath(path)}:{name}",
        ).bytes_le,
        "start_lba": start_lba,
        "end_lba": end_lba,
        "include_efi": include_efi,
        "include_images": include_images,
    }

single_start_lba = align_up(first_usable_lba, alignment_lba)
partitions = []
if layout == "single":
    partitions.append(
        make_partition(
            "NEXBOOT",
            "NEXBOOT",
            "fat32",
            esp_type,
            single_start_lba,
            last_usable_lba,
            True,
            True,
        )
    )
else:
    esp_size_mib = 64 if sector_size == 512 else 260
    esp_start_lba = single_start_lba
    esp_end_lba = esp_start_lba + mib_to_sectors(esp_size_mib) - 1
    data_start_lba = align_up(esp_end_lba + 1, alignment_lba)
    data_end_lba = last_usable_lba
    partitions.append(
        make_partition(
            "NEXBOOT_EFI",
            "NEXBOOT",
            "fat32",
            esp_type,
            esp_start_lba,
            esp_end_lba,
            True,
            False,
        )
    )
    partitions.append(
        make_partition(
            "NEXBOOT_DATA",
            "NEXTDATA",
            data_fs,
            ms_basic_type,
            data_start_lba,
            data_end_lba,
            False,
            True,
        )
    )

def fat_label(label):
    return label.upper().encode("ascii", "ignore")[:11].ljust(11, b" ")

def partition_name_bytes(name):
    encoded = name.encode("utf-16le")[:72]
    return encoded + bytes(72 - len(encoded))

def write_fat32_volume(f, part):
    part_start_lba = part["start_lba"]
    part_sectors = part["end_lba"] - part["start_lba"] + 1
    partition_offset = part_start_lba * sector_size
    reserved_sectors, num_fats, sectors_per_cluster, fat_size, cluster_count = (
        fat32_geometry(part_sectors)
    )
    media = 0xF8
    cluster_size = sectors_per_cluster * sector_size
    fat_offset = partition_offset + reserved_sectors * sector_size
    data_offset = partition_offset + (reserved_sectors + num_fats * fat_size) * sector_size
    fat = bytearray(fat_size * sector_size)
    next_cluster = 2

    def set_fat(cluster, value):
        struct.pack_into("<I", fat, cluster * 4, value & 0x0FFFFFFF)

    def cluster_offset(cluster):
        return data_offset + (cluster - 2) * cluster_size

    def allocate_cluster():
        nonlocal next_cluster
        if next_cluster >= cluster_count + 2:
            raise SystemExit(f"{part['name']} is too small for requested files")
        cluster = next_cluster
        next_cluster += 1
        set_fat(cluster, 0x0FFFFFFF)
        return cluster

    def allocate_chain(count):
        if count == 0:
            return []
        chain = [allocate_cluster() for _ in range(count)]
        for current, nxt in zip(chain, chain[1:]):
            set_fat(current, nxt)
        set_fat(chain[-1], 0x0FFFFFFF)
        return chain

    def write_cluster(cluster, data):
        f.seek(cluster_offset(cluster))
        if len(data) > cluster_size:
            raise SystemExit("internal error: cluster write too large")
        f.write(data)
        if len(data) < cluster_size:
            f.write(bytes(cluster_size - len(data)))

    def copy_file(source, target_dir, target_name):
        size = os.path.getsize(source)
        clusters_needed = math.ceil(size / cluster_size) if size else 0
        chain = allocate_chain(clusters_needed)
        with open(source, "rb") as src:
            for cluster in chain:
                write_cluster(cluster, src.read(cluster_size))
        first = chain[0] if chain else 0
        target_dir.add(target_name, 0x20, first, size)

    def copy_bytes(data, target_dir, target_name):
        size = len(data)
        clusters_needed = math.ceil(size / cluster_size) if size else 0
        chain = allocate_chain(clusters_needed)
        for index, cluster in enumerate(chain):
            start = index * cluster_size
            write_cluster(cluster, data[start : start + cluster_size])
        first = chain[0] if chain else 0
        target_dir.add(target_name, 0x20, first, size)

    def flush_directory(directory):
        content = b"".join(directory.entries)
        needed = max(1, math.ceil((len(content) + 32) / cluster_size))
        chain = [directory.first_cluster]
        current = directory.first_cluster
        while True:
            value = struct.unpack_from("<I", fat, current * 4)[0] & 0x0FFFFFFF
            if value >= 0x0FFFFFF8:
                break
            chain.append(value)
            current = value
        while len(chain) < needed:
            new_cluster = allocate_cluster()
            set_fat(chain[-1], new_cluster)
            set_fat(new_cluster, 0x0FFFFFFF)
            chain.append(new_cluster)
        content += b"\x00" * (len(chain) * cluster_size - len(content))
        for index, cluster in enumerate(chain):
            write_cluster(cluster, content[index * cluster_size : (index + 1) * cluster_size])

    set_fat(0, 0x0FFFFF00 | media)
    set_fat(1, 0x0FFFFFFF)

    root = Directory(allocate_cluster())
    directories = [root]
    dirs_by_path = {"/": root}

    def ensure_directory(path):
        current = root
        current_path = "/"
        components = [] if path in ("", "/") else split_virtual_path(path)
        for component in components:
            next_path = current_path.rstrip("/") + "/" + component
            if next_path not in dirs_by_path:
                directory = Directory(allocate_cluster())
                current.add(component, 0x10, directory.first_cluster, 0)
                dirs_by_path[next_path] = directory
                directories.append(directory)
            current = dirs_by_path[next_path]
            current_path = next_path
        return current

    if part["include_efi"]:
        boot = ensure_directory("/EFI/BOOT")
        copy_file(efi_file, boot, "BOOTX64.EFI")

    if part["include_images"]:
        iso = ensure_directory("/ISO")
        for image in image_files:
            copy_file(image, iso, os.path.basename(image))
        for virtual_path, data in extra_files:
            parts = split_virtual_path(virtual_path)
            target_dir = ensure_directory("/" + "/".join(parts[:-1]))
            copy_bytes(data, target_dir, parts[-1])

    for directory in directories:
        flush_directory(directory)

    if part_sectors > 0xFFFFFFFF:
        raise SystemExit("FAT32 test partition is too large")

    volume_id = int(time.time()) & 0xFFFFFFFF
    boot_sector = bytearray(sector_size)
    boot_sector[0:3] = b"\xeb\x58\x90"
    boot_sector[3:11] = b"MSWIN4.1"
    struct.pack_into("<H", boot_sector, 11, sector_size)
    boot_sector[13] = sectors_per_cluster
    struct.pack_into("<H", boot_sector, 14, reserved_sectors)
    boot_sector[16] = num_fats
    boot_sector[21] = media
    struct.pack_into("<H", boot_sector, 24, 63)
    struct.pack_into("<H", boot_sector, 26, 255)
    struct.pack_into("<I", boot_sector, 28, part_start_lba)
    struct.pack_into("<I", boot_sector, 32, part_sectors)
    struct.pack_into("<I", boot_sector, 36, fat_size)
    struct.pack_into("<I", boot_sector, 44, root.first_cluster)
    struct.pack_into("<H", boot_sector, 48, 1)
    struct.pack_into("<H", boot_sector, 50, 6)
    boot_sector[64] = 0x80
    boot_sector[66] = 0x29
    struct.pack_into("<I", boot_sector, 67, volume_id)
    boot_sector[71:82] = fat_label(part["label"])
    boot_sector[82:90] = b"FAT32   "
    boot_sector[510:512] = b"\x55\xaa"

    fsinfo = bytearray(sector_size)
    struct.pack_into("<I", fsinfo, 0, 0x41615252)
    struct.pack_into("<I", fsinfo, 484, 0x61417272)
    struct.pack_into("<I", fsinfo, 488, max(0, cluster_count - next_cluster))
    struct.pack_into("<I", fsinfo, 492, next_cluster)
    struct.pack_into("<I", fsinfo, 508, 0xAA550000)

    f.seek(partition_offset)
    f.write(boot_sector)
    f.seek(partition_offset + sector_size)
    f.write(fsinfo)
    f.seek(partition_offset + 6 * sector_size)
    f.write(boot_sector)
    f.seek(partition_offset + 7 * sector_size)
    f.write(fsinfo)
    for index in range(num_fats):
        f.seek(fat_offset + index * fat_size * sector_size)
        f.write(fat)

def exfat_entry_set(name, attr, first_cluster, size, contiguous):
    encoded_name = name.encode("utf-16le")
    code_units = [encoded_name[i] | (encoded_name[i + 1] << 8) for i in range(0, len(encoded_name), 2)]
    name_entries = max(1, math.ceil(len(code_units) / 15))
    secondary_count = 1 + name_entries

    file_entry = bytearray(32)
    file_entry[0] = 0x85
    file_entry[1] = secondary_count
    struct.pack_into("<H", file_entry, 4, attr)

    stream_entry = bytearray(32)
    stream_entry[0] = 0xC0
    stream_entry[1] = 0x02 if contiguous else 0
    stream_entry[3] = len(code_units)
    struct.pack_into("<Q", stream_entry, 8, size)
    struct.pack_into("<I", stream_entry, 20, first_cluster)
    struct.pack_into("<Q", stream_entry, 24, size)

    entries = [bytes(file_entry), bytes(stream_entry)]
    for index in range(name_entries):
        name_entry = bytearray(32)
        name_entry[0] = 0xC1
        chunk = code_units[index * 15 : (index + 1) * 15]
        for char_index, value in enumerate(chunk):
            struct.pack_into("<H", name_entry, 2 + char_index * 2, value)
        entries.append(bytes(name_entry))

    return b"".join(entries)

def write_exfat_volume(f, part):
    part_start_lba = part["start_lba"]
    part_sectors = part["end_lba"] - part["start_lba"] + 1
    partition_offset = part_start_lba * sector_size
    fat_offset, fat_length, cluster_heap_offset, cluster_count, sectors_per_cluster = (
        exfat_geometry(part_sectors)
    )
    cluster_size = sectors_per_cluster * sector_size
    fat = bytearray(fat_length * sector_size)
    next_cluster = 2

    def set_fat(cluster, value):
        struct.pack_into("<I", fat, cluster * 4, value & 0xFFFFFFFF)

    def cluster_offset(cluster):
        return partition_offset + (
            cluster_heap_offset + (cluster - 2) * sectors_per_cluster
        ) * sector_size

    def allocate_chain(count):
        nonlocal next_cluster
        if count == 0:
            return []
        if next_cluster + count > cluster_count + 2:
            raise SystemExit(f"{part['name']} is too small for requested files")
        chain = list(range(next_cluster, next_cluster + count))
        next_cluster += count
        for current, nxt in zip(chain, chain[1:]):
            set_fat(current, nxt)
        set_fat(chain[-1], 0xFFFFFFFF)
        return chain

    def write_cluster(cluster, data):
        f.seek(cluster_offset(cluster))
        if len(data) > cluster_size:
            raise SystemExit("internal error: exFAT cluster write too large")
        f.write(data)
        if len(data) < cluster_size:
            f.write(bytes(cluster_size - len(data)))

    def write_chain(chain, data):
        content = data + bytes(len(chain) * cluster_size - len(data))
        for index, cluster in enumerate(chain):
            write_cluster(cluster, content[index * cluster_size : (index + 1) * cluster_size])

    def copy_file(source):
        size = os.path.getsize(source)
        clusters_needed = math.ceil(size / cluster_size) if size else 0
        chain = allocate_chain(clusters_needed)
        with open(source, "rb") as src:
            for cluster in chain:
                write_cluster(cluster, src.read(cluster_size))
        return (chain[0] if chain else 0, size)

    def copy_bytes(data):
        size = len(data)
        clusters_needed = math.ceil(size / cluster_size) if size else 0
        chain = allocate_chain(clusters_needed)
        write_chain(chain, data)
        return (chain[0] if chain else 0, size)

    def write_directory(entry_sets):
        content = b"".join(entry_sets)
        clusters_needed = max(1, math.ceil((len(content) + 32) / cluster_size))
        chain = allocate_chain(clusters_needed)
        write_chain(chain, content)
        return chain[0], len(chain) * cluster_size

    set_fat(0, 0xFFFFFFF8)
    set_fat(1, 0xFFFFFFFF)

    class TreeDirectory:
        def __init__(self):
            self.directories = {}
            self.files = []

    root = TreeDirectory()

    def ensure_tree_directory(path):
        current = root
        components = [] if path in ("", "/") else split_virtual_path(path)
        for component in components:
            current = current.directories.setdefault(component, TreeDirectory())
        return current

    def add_tree_file(path, source=None, data=None):
        parts = split_virtual_path(path)
        directory = ensure_tree_directory("/" + "/".join(parts[:-1]))
        directory.files.append((parts[-1], source, data))

    if part["include_efi"]:
        add_tree_file("/EFI/BOOT/BOOTX64.EFI", source=efi_file)

    if part["include_images"]:
        for image in image_files:
            add_tree_file(f"/ISO/{os.path.basename(image)}", source=image)
        for virtual_path, data in extra_files:
            add_tree_file(virtual_path, data=data)

    def write_tree_directory(directory):
        entry_sets = []
        for name, child in directory.directories.items():
            child_cluster, child_size = write_tree_directory(child)
            entry_sets.append(exfat_entry_set(name, 0x0010, child_cluster, child_size, False))
        for name, source, data in directory.files:
            if source is not None:
                file_cluster, file_size = copy_file(source)
            else:
                file_cluster, file_size = copy_bytes(data or b"")
            entry_sets.append(exfat_entry_set(name, 0x0020, file_cluster, file_size, True))
        return write_directory(entry_sets)

    root_cluster, _root_size = write_tree_directory(root)

    bytes_per_sector_shift = log2_power_of_two(sector_size)
    sectors_per_cluster_shift = log2_power_of_two(sectors_per_cluster)
    volume_id = int(time.time()) & 0xFFFFFFFF

    boot_sector = bytearray(sector_size)
    boot_sector[0:3] = b"\xeb\x76\x90"
    boot_sector[3:11] = b"EXFAT   "
    struct.pack_into("<Q", boot_sector, 64, part_start_lba)
    struct.pack_into("<Q", boot_sector, 72, part_sectors)
    struct.pack_into("<I", boot_sector, 80, fat_offset)
    struct.pack_into("<I", boot_sector, 84, fat_length)
    struct.pack_into("<I", boot_sector, 88, cluster_heap_offset)
    struct.pack_into("<I", boot_sector, 92, cluster_count)
    struct.pack_into("<I", boot_sector, 96, root_cluster)
    struct.pack_into("<I", boot_sector, 100, volume_id)
    struct.pack_into("<H", boot_sector, 104, 0x0100)
    struct.pack_into("<H", boot_sector, 106, 0)
    boot_sector[108] = bytes_per_sector_shift
    boot_sector[109] = sectors_per_cluster_shift
    boot_sector[110] = 1
    boot_sector[111] = 0x80
    boot_sector[112] = min(100, int((next_cluster - 2) * 100 / max(1, cluster_count)))
    boot_sector[510:512] = b"\x55\xaa"

    f.seek(partition_offset)
    f.write(boot_sector)
    f.seek(partition_offset + 12 * sector_size)
    f.write(boot_sector)
    f.seek(partition_offset + fat_offset * sector_size)
    f.write(fat)

def write_ntfs_volume(f, part):
    part_start_lba = part["start_lba"]
    part_sectors = part["end_lba"] - part["start_lba"] + 1
    partition_offset = part_start_lba * sector_size
    sectors_per_cluster, cluster_count, file_record_size, index_record_size = ntfs_geometry(
        part_sectors
    )
    cluster_size = sectors_per_cluster * sector_size
    mft_lcn = 4

    attr_type_data = 0x80
    attr_type_index_root = 0x90
    attr_type_file_name = 0x30
    attr_type_end = 0xFFFFFFFF
    file_attribute_archive = 0x00000020
    file_attribute_directory = 0x10000000
    index_entry_last = 0x0002

    class NtfsNode:
        def __init__(self, name, is_dir, source=None, data=None):
            self.name = name
            self.is_dir = is_dir
            self.source = source
            self.data = data
            self.children = []
            self.children_by_name = {}
            self.record = 0
            self.size = 0
            self.lcn = 0
            self.clusters = 0

    root = NtfsNode("", True)
    root.record = 5

    def ensure_ntfs_directory(path):
        current = root
        components = [] if path in ("", "/") else split_virtual_path(path)
        for component in components:
            key = component.lower()
            if key not in current.children_by_name:
                child = NtfsNode(component, True)
                current.children.append(child)
                current.children_by_name[key] = child
            current = current.children_by_name[key]
            if not current.is_dir:
                raise SystemExit(f"NTFS path component is not a directory: {component}")
        return current

    def add_ntfs_file(path, source=None, data=None):
        parts = split_virtual_path(path)
        directory = ensure_ntfs_directory("/" + "/".join(parts[:-1]))
        node = NtfsNode(parts[-1], False, source=source, data=data)
        node.size = os.path.getsize(source) if source is not None else len(data or b"")
        directory.children.append(node)
        directory.children_by_name[node.name.lower()] = node

    if part["include_efi"]:
        add_ntfs_file("/EFI/BOOT/BOOTX64.EFI", source=efi_file)

    if part["include_images"]:
        for image in image_files:
            add_ntfs_file(f"/ISO/{os.path.basename(image)}", source=image)
        for virtual_path, data in extra_files:
            add_ntfs_file(virtual_path, data=data)

    next_record = 6

    def assign_records(directory):
        nonlocal next_record
        for child in directory.children:
            child.record = next_record
            next_record += 1
            if child.is_dir:
                assign_records(child)

    assign_records(root)
    mft_record_count = next_record
    mft_bytes = mft_record_count * file_record_size
    mft_clusters = math.ceil(mft_bytes / cluster_size)
    next_cluster = max(64, mft_lcn + mft_clusters + 8)

    def allocate_clusters(count):
        nonlocal next_cluster
        if count == 0:
            return 0
        if next_cluster + count > cluster_count:
            raise SystemExit(f"{part['name']} is too small for requested NTFS files")
        start = next_cluster
        next_cluster += count
        return start

    def write_node_payloads(node):
        if node.is_dir:
            for child in node.children:
                write_node_payloads(child)
            return

        node.clusters = math.ceil(node.size / cluster_size) if node.size else 0
        node.lcn = allocate_clusters(node.clusters)
        if node.clusters == 0:
            return

        f.seek(partition_offset + node.lcn * cluster_size)
        if node.source is not None:
            with open(node.source, "rb") as src:
                remaining = node.size
                while remaining > 0:
                    chunk = src.read(min(cluster_size, remaining))
                    if not chunk:
                        raise SystemExit(f"short read while copying {node.source}")
                    f.write(chunk)
                    remaining -= len(chunk)
                padding = node.clusters * cluster_size - node.size
                if padding:
                    f.write(bytes(padding))
        else:
            content = node.data or b""
            f.write(content)
            padding = node.clusters * cluster_size - len(content)
            if padding:
                f.write(bytes(padding))

    write_node_payloads(root)

    def ntfs_record_size_code(byte_size):
        if byte_size >= cluster_size and byte_size % cluster_size == 0:
            value = byte_size // cluster_size
            if 1 <= value <= 127:
                return value
        if byte_size > 0 and byte_size & (byte_size - 1) == 0:
            shift = log2_power_of_two(byte_size)
            if shift < 128:
                return (-shift) & 0xFF
        raise SystemExit(f"unsupported NTFS record size: {byte_size}")

    def uint_le_bytes(value):
        if value < 0:
            raise SystemExit("negative unsigned NTFS run value")
        size = max(1, (value.bit_length() + 7) // 8)
        return value.to_bytes(size, "little")

    def int_le_bytes(value):
        for size in range(1, 9):
            try:
                encoded = value.to_bytes(size, "little", signed=True)
            except OverflowError:
                continue
            if int.from_bytes(encoded, "little", signed=True) == value:
                return encoded
        raise SystemExit("NTFS run delta is too large")

    def ntfs_data_runs(runs):
        out = bytearray()
        previous_lcn = 0
        for lcn, length in runs:
            length_bytes = uint_le_bytes(length)
            delta_bytes = int_le_bytes(lcn - previous_lcn)
            previous_lcn = lcn
            if len(length_bytes) > 8 or len(delta_bytes) > 8:
                raise SystemExit("NTFS run field is too large")
            out.append((len(delta_bytes) << 4) | len(length_bytes))
            out.extend(length_bytes)
            out.extend(delta_bytes)
        out.append(0)
        return bytes(out)

    def ntfs_data_attr_nonresident(real_size, runs):
        runlist = ntfs_data_runs(runs)
        total_clusters = sum(length for _lcn, length in runs)
        allocated_size = total_clusters * cluster_size
        runlist_offset = 0x40
        attr_len = align_up(runlist_offset + len(runlist), 8)
        attr = bytearray(attr_len)
        struct.pack_into("<I", attr, 0, attr_type_data)
        struct.pack_into("<I", attr, 4, attr_len)
        attr[8] = 1
        highest_vcn = total_clusters - 1 if total_clusters else 0
        struct.pack_into("<Q", attr, 0x18, highest_vcn)
        struct.pack_into("<H", attr, 0x20, runlist_offset)
        struct.pack_into("<Q", attr, 0x28, allocated_size)
        struct.pack_into("<Q", attr, 0x30, real_size)
        struct.pack_into("<Q", attr, 0x38, real_size)
        attr[runlist_offset : runlist_offset + len(runlist)] = runlist
        return bytes(attr)

    def ntfs_resident_attr(attr_type, value):
        value_offset = 0x18
        attr_len = align_up(value_offset + len(value), 8)
        attr = bytearray(attr_len)
        struct.pack_into("<I", attr, 0, attr_type)
        struct.pack_into("<I", attr, 4, attr_len)
        struct.pack_into("<I", attr, 0x10, len(value))
        struct.pack_into("<H", attr, 0x14, value_offset)
        attr[value_offset : value_offset + len(value)] = value
        return bytes(attr)

    def ntfs_index_entry(node):
        attrs = file_attribute_directory if node.is_dir else file_attribute_archive
        allocated = 0 if node.is_dir else align_up(node.size, cluster_size)
        name_units = list(node.name.encode("utf-16le"))
        name_len = len(name_units) // 2
        file_name = bytearray(66 + len(name_units))
        struct.pack_into("<Q", file_name, 40, allocated)
        struct.pack_into("<Q", file_name, 48, node.size)
        struct.pack_into("<I", file_name, 56, attrs)
        file_name[64] = name_len
        file_name[65] = 1
        file_name[66 : 66 + len(name_units)] = bytes(name_units)

        entry_len = align_up(16 + len(file_name), 8)
        entry = bytearray(entry_len)
        entry[0:6] = node.record.to_bytes(8, "little")[:6]
        struct.pack_into("<H", entry, 8, entry_len)
        struct.pack_into("<H", entry, 10, len(file_name))
        entry[16 : 16 + len(file_name)] = file_name
        return bytes(entry)

    def ntfs_index_root_attr(children):
        entries = [ntfs_index_entry(child) for child in children]
        value = bytearray(32)
        struct.pack_into("<I", value, 0, attr_type_file_name)
        struct.pack_into("<I", value, 8, index_record_size)
        value[12] = 1
        struct.pack_into("<I", value, 16, 16)
        entries_len = sum(len(entry) for entry in entries)
        for entry in entries:
            value.extend(entry)
        last = bytearray(16)
        struct.pack_into("<H", last, 8, 16)
        struct.pack_into("<H", last, 12, index_entry_last)
        entries_len += len(last)
        value.extend(last)
        total = 16 + entries_len
        struct.pack_into("<I", value, 20, total)
        struct.pack_into("<I", value, 24, total)
        return ntfs_resident_attr(attr_type_index_root, value)

    def ntfs_apply_fixup(record):
        sector_count = len(record) // sector_size
        if sector_count == 0 or len(record) % sector_size != 0:
            raise SystemExit("NTFS record size is not sector aligned")
        usa_offset = 0x30
        usa_count = sector_count + 1
        sequence = 0xA55A
        struct.pack_into("<H", record, 4, usa_offset)
        struct.pack_into("<H", record, 6, usa_count)
        struct.pack_into("<H", record, usa_offset, sequence)
        for sector in range(sector_count):
            tail = (sector + 1) * sector_size - 2
            original = record[tail : tail + 2]
            record[usa_offset + 2 * (sector + 1) : usa_offset + 2 * (sector + 2)] = original
            struct.pack_into("<H", record, tail, sequence)

    def ntfs_mft_record(is_dir, attrs):
        record = bytearray(file_record_size)
        record[0:4] = b"FILE"
        attrs_offset = 0x38
        struct.pack_into("<H", record, 0x10, 1)
        struct.pack_into("<H", record, 0x14, attrs_offset)
        struct.pack_into("<H", record, 0x16, 0x0003 if is_dir else 0x0001)
        cursor = attrs_offset
        for attr in attrs:
            end = cursor + len(attr)
            if end + 4 > len(record):
                raise SystemExit("NTFS MFT record is too small for generated attributes")
            record[cursor:end] = attr
            cursor = end
        struct.pack_into("<I", record, cursor, attr_type_end)
        cursor += 4
        struct.pack_into("<I", record, 0x18, cursor)
        struct.pack_into("<I", record, 0x1C, file_record_size)
        ntfs_apply_fixup(record)
        return record

    def write_record(record_number, is_dir, attrs):
        offset = partition_offset + mft_lcn * cluster_size + record_number * file_record_size
        f.seek(offset)
        f.write(ntfs_mft_record(is_dir, attrs))

    mft_attr = ntfs_data_attr_nonresident(
        mft_clusters * cluster_size, [(mft_lcn, mft_clusters)]
    )
    write_record(0, False, [mft_attr])

    def write_node_records(node):
        if node.is_dir:
            write_record(node.record, True, [ntfs_index_root_attr(node.children)])
            for child in node.children:
                write_node_records(child)
        else:
            runs = [(node.lcn, node.clusters)] if node.clusters else []
            write_record(node.record, False, [ntfs_data_attr_nonresident(node.size, runs)])

    write_node_records(root)

    boot_sector = bytearray(sector_size)
    boot_sector[0:3] = b"\xeb\x52\x90"
    boot_sector[3:11] = b"NTFS    "
    struct.pack_into("<H", boot_sector, 0x0B, sector_size)
    boot_sector[0x0D] = sectors_per_cluster
    struct.pack_into("<Q", boot_sector, 0x28, part_sectors)
    struct.pack_into("<Q", boot_sector, 0x30, mft_lcn)
    struct.pack_into("<Q", boot_sector, 0x38, max(8, cluster_count // 2))
    boot_sector[0x40] = ntfs_record_size_code(file_record_size)
    boot_sector[0x44] = ntfs_record_size_code(index_record_size)
    struct.pack_into("<Q", boot_sector, 0x48, int(time.time()))
    boot_sector[510:512] = b"\x55\xaa"

    f.seek(partition_offset)
    f.write(boot_sector)
    f.seek(partition_offset + (part_sectors - 1) * sector_size)
    f.write(boot_sector)

with open(path, "wb") as f:
    f.truncate(total_sectors * sector_size)

    mbr = bytearray(sector_size)
    mbr[0x1BE] = 0x00
    mbr[0x1BE + 4] = 0xEE
    mbr[0x1BE + 8:0x1BE + 12] = struct.pack("<I", 1)
    protective_size = min(total_sectors - 1, 0xFFFFFFFF)
    mbr[0x1BE + 12:0x1BE + 16] = struct.pack("<I", protective_size)
    mbr[510:512] = b"\x55\xaa"
    f.seek(0)
    f.write(mbr)

    entries = bytearray(entry_count * entry_size)
    for index, part in enumerate(partitions):
        entry = bytearray(entry_size)
        entry[0:16] = part["type_guid"]
        entry[16:32] = part["guid"]
        entry[32:40] = struct.pack("<Q", part["start_lba"])
        entry[40:48] = struct.pack("<Q", part["end_lba"])
        entry[56:128] = partition_name_bytes(part["name"])
        start = index * entry_size
        entries[start:start + entry_size] = entry
    entries_crc = zlib.crc32(entries) & 0xFFFFFFFF

    def make_header(current_lba, backup_lba, entries_lba):
        header = bytearray(sector_size)
        header[0:8] = b"EFI PART"
        header[8:12] = struct.pack("<I", 0x00010000)
        header[12:16] = struct.pack("<I", 92)
        header[24:32] = struct.pack("<Q", current_lba)
        header[32:40] = struct.pack("<Q", backup_lba)
        header[40:48] = struct.pack("<Q", first_usable_lba)
        header[48:56] = struct.pack("<Q", last_usable_lba)
        header[56:72] = disk_guid
        header[72:80] = struct.pack("<Q", entries_lba)
        header[80:84] = struct.pack("<I", entry_count)
        header[84:88] = struct.pack("<I", entry_size)
        header[88:92] = struct.pack("<I", entries_crc)
        crc = zlib.crc32(header[:92]) & 0xFFFFFFFF
        header[16:20] = struct.pack("<I", crc)
        return header

    f.seek(primary_entries_lba * sector_size)
    f.write(entries)
    f.seek(backup_entries_lba * sector_size)
    f.write(entries)
    f.seek(sector_size)
    f.write(make_header(1, last_lba, primary_entries_lba))
    f.seek(last_lba * sector_size)
    f.write(make_header(last_lba, 1, backup_entries_lba))

    for part in partitions:
        if part["fs_type"] == "exfat":
            write_exfat_volume(f, part)
        elif part["fs_type"] == "ntfs":
            write_ntfs_volume(f, part)
        else:
            write_fat32_volume(f, part)
PY

info "Disk image created: ${DISK_IMG}"

if [ "$VERIFY_IMAGE" -eq 1 ]; then
    VERIFY_SCRIPT="${SCRIPT_DIR}/verify-qemu-image.py"
    [ -f "$VERIFY_SCRIPT" ] || die "QEMU image verifier not found: ${VERIFY_SCRIPT}"
    warn "Verifying GPT/filesystem layout..."
    VERIFY_ARGS=(
        --disk-image "$DISK_IMG"
        --sector-size "$SECTOR_SIZE"
        --layout "$LAYOUT"
        --data-fs "$DATA_FS"
        --efi-file "$EFI_FILE"
    )
    for image in "${IMAGES[@]}"; do
        VERIFY_ARGS+=(--image "$image")
    done
    python3 "$VERIFY_SCRIPT" "${VERIFY_ARGS[@]}"
fi

DEVICE_BLOCK_OPTS=""
if [ "$SECTOR_SIZE" -ne 512 ]; then
    DEVICE_BLOCK_OPTS=",logical_block_size=${SECTOR_SIZE},physical_block_size=${SECTOR_SIZE}"
fi

QEMU_OPTS=(
    -machine q35,accel=tcg
    -m "$MEMORY"
    -net none
    -nographic
    -serial mon:stdio
)

OVMF_PATHS=(
    "/usr/share/OVMF/OVMF_CODE.fd"
    "/usr/share/ovmf/OVMF.fd"
    "/usr/share/qemu/OVMF.fd"
    "/opt/homebrew/share/qemu/edk2-x86_64-code.fd"
    "/opt/homebrew/opt/qemu/share/qemu/edk2-x86_64-code.fd"
)

OVMF_CODE=""
for path in "${OVMF_PATHS[@]}"; do
    if [ -f "$path" ]; then
        OVMF_CODE="$path"
        break
    fi
done

if [ -n "$OVMF_CODE" ]; then
    QEMU_OPTS+=(-drive "if=pflash,format=raw,readonly=on,file=${OVMF_CODE}")
elif [ "$NO_RUN" -eq 0 ]; then
    die "OVMF firmware not found. Install OVMF/edk2-ovmf or use --no-run to only create the image."
fi

case "$BUS" in
    virtio)
        QEMU_OPTS+=(
            -drive "if=none,id=nextboot_disk,format=raw,file=${DISK_IMG}"
            -device "virtio-blk-pci,drive=nextboot_disk,bootindex=1${DEVICE_BLOCK_OPTS}"
        )
        ;;
    nvme)
        QEMU_OPTS+=(
            -drive "if=none,id=nextboot_disk,format=raw,file=${DISK_IMG}"
            -device "nvme,drive=nextboot_disk,serial=NEXTBOOT0,bootindex=1${DEVICE_BLOCK_OPTS}"
        )
        ;;
    sata)
        QEMU_OPTS+=(
            -device "ahci,id=ahci0"
            -drive "if=none,id=nextboot_disk,format=raw,file=${DISK_IMG}"
            -device "ide-hd,drive=nextboot_disk,bus=ahci0.0,bootindex=1${DEVICE_BLOCK_OPTS}"
        )
        ;;
    usb)
        QEMU_OPTS+=(
            -device "qemu-xhci,id=xhci"
            -drive "if=none,id=nextboot_disk,format=raw,file=${DISK_IMG}"
            -device "usb-storage,drive=nextboot_disk,bootindex=1${DEVICE_BLOCK_OPTS}"
        )
        ;;
esac

echo -e "${BLUE}QEMU command:${NC}"
printf 'qemu-system-x86_64'
for opt in "${QEMU_OPTS[@]}"; do
    printf ' %q' "$opt"
done
printf '\n'

if [ "$NO_RUN" -eq 1 ]; then
    warn "--no-run set; image is ready for manual testing."
    exit 0
fi

require_command qemu-system-x86_64 "qemu-system-x86_64 is required to run the VM"

if [ -n "$OVMF_CODE" ]; then
    info "Using OVMF: ${OVMF_CODE}"
fi

if [ "$SMOKE" -eq 1 ]; then
    SMOKE_SCRIPT="${SCRIPT_DIR}/qemu-boot-smoke.py"
    [ -f "$SMOKE_SCRIPT" ] || die "QEMU smoke runner not found: ${SMOKE_SCRIPT}"
    EXPECT_ARGS=(
        --expect "NextBoot v"
        --expect "Phase 2: Scanning for ISO files"
    )
    if [ "${#IMAGES[@]}" -gt 0 ]; then
        EXPECT_ARGS+=(--expect "Found ${#IMAGES[@]} ISO file(s)")
        for image in "${IMAGES[@]}"; do
            EXPECT_ARGS+=(--expect "$(basename "$image")")
        done
        EXPECT_ARGS+=(--expect "Phase 3: Displaying boot menu")
        if [ "$SMOKE_BOOT" -eq 1 ]; then
            EXPECT_ARGS+=(
                --send-after "Phase 3: Displaying boot menu"
                --send-key enter
                --expect "Selected:"
                --expect "Phase 4: Booting selected ISO"
                --expect "Preparing to boot:"
                --expect "Creating virtual Block IO"
                --expect "Virtual Block IO installed"
            )
            if [ "$SMOKE_EFI_ISO" -eq 1 ]; then
                EXPECT_ARGS+=(
                    --expect "Using EFI El Torito boot image"
                )
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
            fi
        fi
    else
        EXPECT_ARGS+=(--expect "No ISO files found")
    fi
    warn "Running QEMU boot smoke for ${SMOKE_TIMEOUT}s..."
    python3 "$SMOKE_SCRIPT" --timeout "$SMOKE_TIMEOUT" "${EXPECT_ARGS[@]}" -- \
        qemu-system-x86_64 "${QEMU_OPTS[@]}"
    exit 0
fi

warn "Starting QEMU. Press Ctrl+A then X to exit."
qemu-system-x86_64 "${QEMU_OPTS[@]}"

echo ""
info "QEMU exited"
