"""Smoke ISO file layout profiles."""

from __future__ import annotations


PROFILE_GENERIC = "generic"
PROFILE_WINDOWS = "windows"
PROFILE_WINDOWS_WIMBOOT = "windows-wimboot"
PROFILE_LINUX = "linux"
PROFILE_LINUX_GRUB = "linux-grub"

LINUX_SMOKE_INITRD = b"070701NEXTBOOT SMOKE INITRD\n"
LINUX_SMOKE_UCODE = b"070701NEXTBOOT SMOKE MICROCODE\n"


def align_up(value: int, alignment: int) -> int:
    return ((value + alignment - 1) // alignment) * alignment


def read_u16(data: bytes | bytearray, offset: int) -> int:
    return int.from_bytes(data[offset : offset + 2], "little")


def read_u32(data: bytes | bytearray, offset: int) -> int:
    return int.from_bytes(data[offset : offset + 4], "little")


def write_u32(data: bytearray, offset: int, value: int) -> None:
    data[offset : offset + 4] = value.to_bytes(4, "little")


def linux_smoke_kernel(efi_data: bytes) -> bytes:
    if len(efi_data) < 0x208:
        raise ValueError("EFI smoke payload is too small to carry a Linux setup header")
    if efi_data[:2] != b"MZ":
        raise ValueError("EFI smoke payload is not a PE/COFF image")

    original_pe_offset = read_u32(efi_data, 0x3C)
    pe_signature = efi_data[original_pe_offset : original_pe_offset + 4]
    if original_pe_offset + 24 >= len(efi_data) or pe_signature != b"PE\0\0":
        raise ValueError("EFI smoke payload has an invalid PE header")

    section_count = read_u16(efi_data, original_pe_offset + 6)
    pointer_to_symbols = read_u32(efi_data, original_pe_offset + 12)
    optional_size = read_u16(efi_data, original_pe_offset + 20)
    optional_offset = original_pe_offset + 24
    optional_magic = read_u16(efi_data, optional_offset)
    if optional_magic == 0x10B:
        data_directory_offset = optional_offset + 96
    elif optional_magic == 0x20B:
        data_directory_offset = optional_offset + 112
    else:
        raise ValueError("EFI smoke payload has an unsupported optional PE header")

    file_alignment = read_u32(efi_data, optional_offset + 36)
    if file_alignment == 0:
        raise ValueError("EFI smoke payload has an invalid file alignment")

    setup_size = align_up(0x400, file_alignment)
    data = bytearray(setup_size + len(efi_data))
    data[setup_size:] = efi_data

    # Linux EFI stubs keep the Linux setup header before the PE/COFF image.
    new_pe_offset = setup_size + original_pe_offset
    data[0:2] = b"MZ"
    write_u32(data, 0x3C, new_pe_offset)
    data[0x202:0x206] = b"HdrS"
    data[0x206:0x208] = (0x0208).to_bytes(2, "little")

    if pointer_to_symbols:
        write_u32(data, new_pe_offset + 12, pointer_to_symbols + setup_size)

    size_of_headers_offset = setup_size + optional_offset + 60
    new_size_of_headers = align_up(
        read_u32(data, size_of_headers_offset) + setup_size,
        file_alignment,
    )
    write_u32(data, size_of_headers_offset, new_size_of_headers)
    write_u32(data, setup_size + optional_offset + 64, 0)

    security_directory_offset = setup_size + data_directory_offset + 4 * 8
    security_file_pointer = read_u32(data, security_directory_offset)
    if security_file_pointer:
        write_u32(data, security_directory_offset, security_file_pointer + setup_size)

    section_table_offset = setup_size + optional_offset + optional_size
    for index in range(section_count):
        section_offset = section_table_offset + index * 40
        raw_pointer_offset = section_offset + 20
        raw_pointer = read_u32(data, raw_pointer_offset)
        if raw_pointer:
            write_u32(data, raw_pointer_offset, raw_pointer + setup_size)

    return bytes(data)


def make_iso_layout(
    efi_data: bytes,
    profile: str,
    efi_boot_name: str,
) -> tuple[list[dict[str, object]], list[dict[str, object]]]:
    directories: list[dict[str, object]] = [
        {"path": "/", "name": b"\x00", "parent": "/"},
        {"path": "/EFI", "name": b"EFI", "parent": "/"},
    ]
    files: list[dict[str, object]] = []

    if profile == PROFILE_GENERIC:
        directories.extend(
            [
                {"path": "/EFI/BOOT", "name": b"BOOT", "parent": "/EFI"},
                {"path": "/CFG", "name": b"CFG", "parent": "/"},
            ]
        )
        files.extend(
            [
                {
                    "dir": "/EFI/BOOT",
                    "name": f"{efi_boot_name};1".encode("ascii"),
                    "data": efi_data,
                    "eltorito": True,
                },
                {
                    "dir": "/CFG",
                    "name": b"GRUB.CFG;1",
                    "data": b"set timeout=5\nmenuentry 'NextBoot smoke' {}\n",
                    "eltorito": False,
                },
                {
                    "dir": "/CFG",
                    "name": b"KICKSTART.CFG;1",
                    "data": b"# original smoke kickstart\n",
                    "eltorito": False,
                },
                {
                    "dir": "/CFG",
                    "name": b"AUTOINST.CFG;1",
                    "data": b"# original smoke autoinstall\n",
                    "eltorito": False,
                },
            ]
        )
    elif profile == PROFILE_WINDOWS:
        directories.extend(
            [
                {"path": "/EFI/MICROSOFT", "name": b"MICROSOFT", "parent": "/EFI"},
                {
                    "path": "/EFI/MICROSOFT/BOOT",
                    "name": b"BOOT",
                    "parent": "/EFI/MICROSOFT",
                },
                {"path": "/SOURCES", "name": b"SOURCES", "parent": "/"},
            ]
        )
        files.extend(
            [
                {
                    "dir": "/EFI/MICROSOFT/BOOT",
                    "name": b"BOOTMGFW.EFI;1",
                    "data": efi_data,
                    "eltorito": True,
                },
                {
                    "dir": "/SOURCES",
                    "name": b"BOOT.WIM;1",
                    "data": b"NEXTBOOT SMOKE WINDOWS BOOT WIM\n",
                    "eltorito": False,
                },
            ]
        )
    elif profile == PROFILE_WINDOWS_WIMBOOT:
        directories.extend(
            [
                {"path": "/NOBOOT", "name": b"NOBOOT", "parent": "/"},
                {"path": "/SOURCES", "name": b"SOURCES", "parent": "/"},
                {"path": "/BOOT", "name": b"BOOT", "parent": "/"},
            ]
        )
        files.extend(
            [
                {
                    "dir": "/NOBOOT",
                    "name": b"IGNORED.EFI;1",
                    "data": efi_data,
                    "eltorito": True,
                },
                {
                    "dir": "/SOURCES",
                    "name": b"BOOT.WIM;1",
                    "data": b"NEXTBOOT SMOKE WINDOWS WIMBOOT WIM\n",
                    "eltorito": False,
                },
                {
                    "dir": "/BOOT",
                    "name": b"BCD;1",
                    "data": "path\\to\\bootmgr.exe".encode("utf-16le"),
                    "eltorito": False,
                },
                {
                    "dir": "/BOOT",
                    "name": b"BOOT.SDI;1",
                    "data": b"NEXTBOOT SMOKE WINDOWS BOOT SDI\n",
                    "eltorito": False,
                },
            ]
        )
    elif profile == PROFILE_LINUX:
        directories.append({"path": "/BOOT", "name": b"BOOT", "parent": "/"})
        files.extend(
            [
                {
                    "dir": "/BOOT",
                    "name": b"VMLINUZ;1",
                    "data": linux_smoke_kernel(efi_data),
                    "eltorito": True,
                },
                {
                    "dir": "/BOOT",
                    "name": b"INITRD.IMG;1",
                    "data": LINUX_SMOKE_INITRD,
                    "eltorito": False,
                },
                {
                    "dir": "/BOOT",
                    "name": b"GRUB.CFG;1",
                    "data": b"set timeout=5\nmenuentry 'NextBoot smoke' {}\n",
                    "eltorito": False,
                },
                {
                    "dir": "/BOOT",
                    "name": b"KICKSTART.CFG;1",
                    "data": b"# original smoke kickstart\n",
                    "eltorito": False,
                },
                {
                    "dir": "/BOOT",
                    "name": b"AUTOINST.CFG;1",
                    "data": b"# original smoke autoinstall\n",
                    "eltorito": False,
                },
            ]
        )
    elif profile == PROFILE_LINUX_GRUB:
        directories.extend(
            [
                {"path": "/BOOT", "name": b"BOOT", "parent": "/"},
                {"path": "/BOOT/GRUB", "name": b"GRUB", "parent": "/BOOT"},
                {"path": "/CASPER", "name": b"CASPER", "parent": "/"},
            ]
        )
        files.extend(
            [
                {
                    "dir": "/CASPER",
                    "name": b"VMLINUZ;1",
                    "data": linux_smoke_kernel(efi_data),
                    "eltorito": True,
                },
                {
                    "dir": "/CASPER",
                    "name": b"INITRD;1",
                    "data": LINUX_SMOKE_INITRD,
                    "eltorito": False,
                },
                {
                    "dir": "/CASPER",
                    "name": b"UCODE.IMG;1",
                    "data": LINUX_SMOKE_UCODE,
                    "eltorito": False,
                },
                {
                    "dir": "/BOOT/GRUB",
                    "name": b"GRUB.CFG;1",
                    "data": (
                        b"set root='(cd0)'\n"
                        b"set kernel_path=\"/casper/vmlinuz\"\n"
                        b"set initrd_path=\"/casper/initrd\"\n"
                        b"menuentry 'NextBoot complex Linux smoke' {\n"
                        b"  linuxefi ($root)$kernel_path boot=casper quiet splash --- # trailing comment\n"
                        b"  initrdefi ($root)/casper/ucode.img ($root)$initrd_path\n"
                        b"}\n"
                    ),
                    "eltorito": False,
                },
            ]
        )
    else:
        raise ValueError(f"unsupported smoke ISO profile: {profile}")

    return directories, files
