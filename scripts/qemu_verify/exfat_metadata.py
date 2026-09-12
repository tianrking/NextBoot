"""Independent on-disk exFAT metadata validation."""
from .common import require, u16


def checksum(data, bits, skip=()):
    value = 0
    for index, byte in enumerate(data):
        if index not in skip:
            value = (((value >> 1) | ((value & 1) << (bits - 1))) + byte) & ((1 << bits) - 1)
    return value


def decode_upcase(data, expected_checksum):
    require(0 < len(data) <= 131072 and len(data) % 2 == 0, 'invalid exFAT upcase size')
    require(checksum(data, 32) == expected_checksum, 'exFAT upcase checksum mismatch')
    table = []
    offset = 0
    while offset < len(data):
        value = u16(data, offset)
        offset += 2
        if value == 0xFFFF and offset < len(data):
            count = u16(data, offset)
            offset += 2
            require(count > 0 and len(table) + count <= 65536, 'invalid exFAT identity run')
            table.extend(range(len(table), len(table) + count))
        else:
            table.append(value)
        require(len(table) <= 65536, 'exFAT upcase table overflow')
    require(len(table) == 65536, 'incomplete exFAT upcase table')
    for unit in range(128):
        require(table[unit] == (unit - 32 if 97 <= unit <= 122 else unit),
                'invalid mandatory exFAT ASCII mapping')
    return table


def name_hash(name_bytes, table):
    mapped = b''.join(table[u16(name_bytes, index)].to_bytes(2, 'little')
                      for index in range(0, len(name_bytes), 2))
    return checksum(mapped, 16)
