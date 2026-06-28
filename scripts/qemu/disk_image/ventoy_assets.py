"""Ventoy asset copying and NextBoot-specific initrd patching."""

import lzma
import os


VENTOY_ASSET_NAMES = (
    "ventoy.cpio",
    "ventoy_x86.cpio",
    "ventoy_arm64.cpio",
    "wimboot.x86_64.xz",
    "vtoyjump64.exe",
    "common_bcd.xz",
)

CPIO_NEWC_MAGIC = b"070701"
CPIO_HEADER_LEN = 110
CPIO_TRAILER = "TRAILER!!!"

NEXTBOOT_DM_TABLE_PATCH = r'''
ventoy_nextboot_fix_dm_table() {
    NB_DISK="${1#/dev/}"
    NB_TABLE="$2"
    NB_PART_ID=1

    if echo "$NB_DISK" | $EGREP -q "nvme|mmc|nbd"; then
        NB_PART_NAME="${NB_DISK}p${NB_PART_ID}"
    else
        NB_PART_NAME="${NB_DISK}${NB_PART_ID}"
    fi

    NB_START_FILE="/sys/class/block/${NB_PART_NAME}/start"
    [ -f "$NB_START_FILE" ] || return

    NB_PART_START=$($CAT "$NB_START_FILE" 2>/dev/null)
    case "$NB_PART_START" in
        ''|*[!0-9]*) return ;;
    esac
    [ "$NB_PART_START" = "2048" ] && return

    NB_DELTA=$((2048 - NB_PART_START))
    $AWK -v delta="$NB_DELTA" '{
        if ($3 == "linear" && $5 ~ /^[0-9]+$/) {
            $5 = $5 + delta
            if ($5 < 0) {
                exit 2
            }
        }
        print
    }' "$NB_TABLE" > "${NB_TABLE}.nextboot" && \
        $BUSYBOX_PATH/mv "${NB_TABLE}.nextboot" "$NB_TABLE"
    $BUSYBOX_PATH/rm -f "${NB_TABLE}.nextboot"
    vtlog "NextBoot adjusted dm table for ${NB_PART_NAME}: start=${NB_PART_START} delta=${NB_DELTA}"
}
'''


def copy_ventoy_assets(ventoy_assets_dir):
    if not os.path.isdir(ventoy_assets_dir):
        raise SystemExit(f"NEXTBOOT_VENTOY_ASSETS_DIR is not a directory: {ventoy_assets_dir}")

    extra_files = []
    for asset_name in VENTOY_ASSET_NAMES:
        asset_path = os.path.join(ventoy_assets_dir, asset_name)
        if not os.path.isfile(asset_path):
            continue
        with open(asset_path, "rb") as asset:
            asset_data = asset.read()
        if asset_name == "ventoy.cpio":
            asset_data = patch_ventoy_cpio(asset_data)
        extra_files.append((f"/ventoy/{asset_name}", asset_data))

    if not extra_files:
        raise SystemExit(f"no known Ventoy assets found in {ventoy_assets_dir}")
    return extra_files


def align4(value):
    return (value + 3) & ~3


def parse_cpio_newc(data):
    entries = []
    offset = 0
    while offset + CPIO_HEADER_LEN <= len(data):
        header = data[offset : offset + CPIO_HEADER_LEN]
        if header[:6] != CPIO_NEWC_MAGIC:
            raise SystemExit("unsupported Ventoy cpio archive")
        fields = [int(header[index : index + 8], 16) for index in range(6, CPIO_HEADER_LEN, 8)]
        file_size = fields[6]
        name_size = fields[11]
        if name_size == 0:
            raise SystemExit("invalid Ventoy cpio entry name")
        name_start = offset + CPIO_HEADER_LEN
        name_end = name_start + name_size
        name_raw = data[name_start:name_end]
        if len(name_raw) != name_size or name_raw[-1:] != b"\0":
            raise SystemExit("truncated Ventoy cpio entry name")
        name = name_raw[:-1].decode("utf-8")
        data_start = align4(name_end)
        data_end = data_start + file_size
        if data_end > len(data):
            raise SystemExit("truncated Ventoy cpio entry data")
        entries.append({"name": name, "fields": fields, "data": data[data_start:data_end]})
        offset = align4(data_end)
        if name == CPIO_TRAILER:
            return entries
    raise SystemExit("Ventoy cpio archive has no trailer")


def build_cpio_newc(entries):
    out = bytearray()
    for entry in entries:
        name = entry["name"].encode("utf-8") + b"\0"
        contents = entry["data"]
        fields = list(entry["fields"])
        fields[6] = len(contents)
        fields[11] = len(name)
        out.extend(CPIO_NEWC_MAGIC)
        for value in fields:
            out.extend(f"{value & 0xFFFFFFFF:08x}".encode("ascii"))
        out.extend(name)
        out.extend(b"\0" * (align4(len(out)) - len(out)))
        out.extend(contents)
        out.extend(b"\0" * (align4(len(out)) - len(out)))
    out.extend(b"\0" * ((512 - len(out) % 512) % 512))
    return bytes(out)


def patch_ventoy_hook_lib(script):
    text = script.decode("utf-8")
    if "ventoy_nextboot_fix_dm_table()" in text:
        return script

    function_marker = "create_ventoy_device_mapper() {\n"
    if function_marker not in text:
        raise SystemExit("Ventoy hook library patch point not found")
    text = text.replace(function_marker, NEXTBOOT_DM_TABLE_PATCH + "\n" + function_marker, 1)

    dm_marker = "    $VTOY_PATH/tool/vtoydm -p -f $VTOY_PATH/ventoy_image_map -d $1 > $VTOY_PATH/ventoy_dm_table\n"
    if dm_marker not in text:
        raise SystemExit("Ventoy dm table patch point not found")
    text = text.replace(
        dm_marker,
        dm_marker + '    ventoy_nextboot_fix_dm_table "$1" "$VTOY_PATH/ventoy_dm_table"\n',
        1,
    )
    return text.encode("utf-8")


def patch_ventoy_cpio(data):
    outer_entries = parse_cpio_newc(data)
    hook_entry = next(
        (entry for entry in outer_entries if entry["name"] == "ventoy/hook.cpio.xz"),
        None,
    )
    if hook_entry is None:
        raise SystemExit("Ventoy cpio is missing ventoy/hook.cpio.xz")

    hook_archive = lzma.decompress(hook_entry["data"])
    hook_entries = parse_cpio_newc(hook_archive)
    script_entry = next(
        (entry for entry in hook_entries if entry["name"] == "hook/ventoy-hook-lib.sh"),
        None,
    )
    if script_entry is None:
        raise SystemExit("Ventoy hook cpio is missing hook/ventoy-hook-lib.sh")

    patched_script = patch_ventoy_hook_lib(script_entry["data"])
    if patched_script == script_entry["data"]:
        return data

    script_entry["data"] = patched_script
    patched_hook_archive = build_cpio_newc(hook_entries)
    hook_entry["data"] = lzma.compress(
        patched_hook_archive,
        format=lzma.FORMAT_XZ,
        check=lzma.CHECK_CRC32,
        preset=0,
    )
    return build_cpio_newc(outer_entries)
