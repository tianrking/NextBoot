#!/usr/bin/env bash
# NextBoot QEMU Test Script
#
# Creates a GPT-partitioned FAT32 disk image with NextBoot installed as the
# removable/fallback UEFI bootloader.  The image can be attached through several
# QEMU storage buses so fixed-disk, NVMe, SATA, USB, and virtio paths can be
# tested without rewriting real media.
#
# Usage:
#   ./scripts/run-qemu.sh
#   ./scripts/run-qemu.sh release
#   ./scripts/run-qemu.sh --bus nvme --image ~/Downloads/ubuntu.iso
#   ./scripts/run-qemu.sh --bus nvme --sector-size 4096 --no-run
#   ./scripts/run-qemu.sh --bus nvme --layout split --image ~/Downloads/ubuntu.iso
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
DISK_IMG=""
NO_RUN=0
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
  --disk-image PATH  Output disk image path
  --memory SIZE      QEMU guest memory (default: 1024M)
  --no-run           Create the disk image and print the QEMU command only
  -h, --help         Show this help

Examples:
  $0 --bus nvme --image ~/Downloads/Win11.iso
  $0 release --bus sata --disk-size 4096
  $0 --bus nvme --sector-size 4096 --no-run
  $0 --bus nvme --layout split --image ~/Downloads/Win11.iso
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

case "$LAYOUT" in
    single|split) ;;
    *) die "--layout must be single or split" ;;
esac

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
info "Disk image: ${DISK_IMG}"

require_command python3 "python3 is required to create the GPT disk image"

warn "Creating ${LAYOUT} GPT/FAT32 test disk image..."
PY_ARGS=("$DISK_IMG" "$DISK_SIZE_MB" "$SECTOR_SIZE" "$LAYOUT" "$EFI_FILE")
if [ "${#IMAGES[@]}" -gt 0 ]; then
    PY_ARGS+=("${IMAGES[@]}")
fi
python3 - "${PY_ARGS[@]}" <<'PY'
import math
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
efi_file = sys.argv[5]
image_files = sys.argv[6:]
if sector_size not in (512, 4096):
    raise SystemExit("sector size must be 512 or 4096")
if layout not in ("single", "split"):
    raise SystemExit("layout must be single or split")
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

def make_partition(name, label, type_guid, start_lba, end_lba, include_efi, include_images):
    if start_lba < first_usable_lba or end_lba > last_usable_lba or end_lba < start_lba:
        raise SystemExit(f"invalid partition range for {name}")
    part_sectors = end_lba - start_lba + 1
    fat32_geometry(part_sectors)
    return {
        "name": name,
        "label": label,
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

    if part["include_efi"]:
        efi = Directory(allocate_cluster())
        boot = Directory(allocate_cluster())
        copy_file(efi_file, boot, "BOOTX64.EFI")
        efi.add("BOOT", 0x10, boot.first_cluster, 0)
        root.add("EFI", 0x10, efi.first_cluster, 0)
        directories.extend([efi, boot])

    if part["include_images"]:
        iso = Directory(allocate_cluster())
        for image in image_files:
            copy_file(image, iso, os.path.basename(image))
        root.add("ISO", 0x10, iso.first_cluster, 0)
        directories.append(iso)

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
        write_fat32_volume(f, part)
PY

info "Disk image created: ${DISK_IMG}"

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
warn "Starting QEMU. Press Ctrl+A then X to exit."
qemu-system-x86_64 "${QEMU_OPTS[@]}"

echo ""
info "QEMU exited"
