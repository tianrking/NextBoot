"""Minimal serial terminal geometry replies for unattended installer smoke tests.

Only cursor/status probes are implemented, not a terminal renderer. Semantics:
https://invisible-island.net/xterm/ctlseqs/ctlseqs.html (CUP, CHA, DSR).
"""


class TerminalProbe:
    def __init__(self, rows=24, columns=80):
        self.rows, self.columns = rows, columns
        self.row, self.column = 1, 1
        self.sequence = bytearray()

    def feed(self, data: bytes) -> bytes:
        replies = bytearray()
        for byte in data:
            if byte == 27:
                self.sequence = bytearray([byte])
            elif self.sequence == b'\x1b' and byte == ord('['):
                self.sequence.append(byte)
            elif self.sequence.startswith(b'\x1b['):
                if 0x40 <= byte <= 0x7e:
                    replies.extend(self.control(bytes(self.sequence[2:]), chr(byte)))
                    self.sequence.clear()
                elif len(self.sequence) < 64:
                    self.sequence.append(byte)
                else:
                    self.sequence.clear()
            else:
                self.sequence.clear()
        return bytes(replies)

    def control(self, parameters: bytes, command: str) -> bytes:
        if command == 'n' and parameters == b'6':
            return f'\x1b[{self.row};{self.column}R'.encode()
        if command == 'n' and parameters == b'5':
            return b'\x1b[0n'
        if command in ('H', 'f', 'G'):
            try:
                values = [int(value or b'1') or 1 for value in parameters.split(b';')]
            except ValueError:
                return b''
            if command in ('H', 'f'):
                self.row = max(1, min(self.rows, values[0]))
                self.column = max(1, min(self.columns, values[1] if len(values) > 1 else 1))
            else:
                self.column = max(1, min(self.columns, values[0]))
        return b''
