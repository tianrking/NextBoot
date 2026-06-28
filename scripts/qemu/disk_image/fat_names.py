import struct


def split_virtual_path(path):
    parts = [part for part in path.replace("\\", "/").split("/") if part]
    if not parts:
        raise SystemExit(f"invalid virtual path: {path}")
    return parts


def fat_label(label):
    return label.upper().encode("ascii", "ignore")[:11].ljust(11, b" ")


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


def split_name(name):
    if "." in name and not name.startswith("."):
        base, ext = name.rsplit(".", 1)
    else:
        base, ext = name, ""
    return base, ext


def sanitize_short_component(text):
    allowed = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789$%'-_@~`!(){}^#&"
    out = bytearray()
    for ch in text.upper().encode("ascii", "ignore"):
        out.append(ch if ch in allowed else ord("_"))
    return bytes(out)


def short_name_checksum(name11):
    checksum = 0
    for byte in name11:
        checksum = (((checksum & 1) << 7) + (checksum >> 1) + byte) & 0xFF
    return checksum


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


def short_to_display_name(name11):
    base = name11[:8].decode("ascii").rstrip()
    ext = name11[8:].decode("ascii").rstrip()
    return base if not ext else f"{base}.{ext}"
