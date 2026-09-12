"""Filesystem primitives for recoverable updates of already verified volumes."""
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import tempfile
import re

from runtime_assets import PROJECT_DIR, manifest

ADMIN = '.nextboot-update'
EFI_MACHINES = {'BOOTX64.EFI': 0x8664, 'BOOTIA32.EFI': 0x14c, 'BOOTAA64.EFI': 0xaa64}
WINDOWS_VOLUME_ROOT = re.compile(r'\\\\\?\\Volume\{[0-9a-fA-F-]{36}\}\\')


def owned_paths():
    data = {'ventoy/' + item['path'] for item in manifest()['files']}
    data.update('ventoy/License/' + p.name for p in (PROJECT_DIR / 'docs/third-party/ventoy').glob('*.txt'))
    data.update({'ventoy/NEXTBOOT-NOTICE.txt', 'ventoy/nextboot-runtime.json'})
    return {'esp': {'EFI/BOOT/' + name for name in EFI_MACHINES}, 'data': data}


def safe_path(root: Path, relative: str) -> Path:
    parts = PurePosixPath(relative).parts
    if not parts or PurePosixPath(relative).is_absolute() or any(p in ('.', '..') or ':' in p or '\\' in p for p in parts):
        raise ValueError(f'unsafe update path: {relative}')
    if '/'.join(parts) != relative:
        raise ValueError(f'non-canonical update path: {relative}')
    current = root
    for index, part in enumerate(('', *parts)):
        if part:
            current = current / part
        if current.is_symlink() or (hasattr(current, 'is_junction') and current.is_junction()):
            raise ValueError(f'linked update path refused: {current}')
        if current.exists() and not (current.is_file() or current.is_dir()):
            raise ValueError(f'special file refused: {current}')
        if index < len(parts) and current.exists() and not current.is_dir():
            raise ValueError(f'update parent is not a directory: {current}')
    return current


def roots_for(esp: Path, data: Path | None):
    roots = {'esp': Path(esp).absolute()}
    if data is not None:
        roots['data'] = Path(data).absolute()
    for area, root in roots.items():
        safe_path(root, ADMIN)
        if not root.is_dir():
            raise ValueError(f'volume must be an existing directory without linked parents: {root}')
        # pathlib cannot realpath a valid \\?\Volume{GUID}\ root on some
        # Windows hosts. It is already bound to the selected partition and
        # rechecked by the native frontend, so preserve that stable root.
        if os.name == 'nt' and WINDOWS_VOLUME_ROOT.fullmatch(str(root)):
            continue
        for parent in root.parents:
            if parent.is_symlink() or (hasattr(parent, 'is_junction') and parent.is_junction()):
                raise ValueError(f'volume has a linked parent: {parent}')
        # Windows commonly supplies an 8.3 TEMP path (RUNNER~1). Its long-name
        # expansion is not a link. Check actual links before canonicalizing it.
        roots[area] = root.resolve(strict=True)
    if data is not None and (roots['esp'].is_relative_to(roots['data']) or roots['data'].is_relative_to(roots['esp'])):
        raise ValueError('ESP and DATA roots must be separate, non-overlapping volumes')
    return roots


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def file_digest(path: Path) -> str | None:
    if path.exists() and not path.is_file():
        raise ValueError(f'expected a regular file: {path}')
    return digest(path.read_bytes()) if path.exists() else None


def sync_dir(path: Path):
    if os.name != 'nt':
        descriptor = os.open(path, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)


def durable_write(path: Path, content: bytes):
    ensure_directory(path.parent)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as handle:
            temporary = Path(handle.name)
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
        sync_dir(path.parent)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def ensure_directory(path: Path):
    missing = []
    current = path
    while not current.exists():
        missing.append(current)
        current = current.parent
    for directory in reversed(missing):
        directory.mkdir()
        sync_dir(directory.parent)


def write_json(path: Path, payload):
    durable_write(path, (json.dumps(payload, indent=2) + '\n').encode())


@contextmanager
def update_lock(root: Path):
    directory = safe_path(root, ADMIN)
    directory.mkdir(exist_ok=True)
    path = safe_path(root, ADMIN + '/update.lock')
    with path.open('a+b') as handle:
        if path.stat().st_size == 0:
            handle.write(b'0')
            handle.flush()
        handle.seek(0)
        try:
            if os.name == 'nt':
                import msvcrt
                msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            raise ValueError('another media update is already running') from error
        try:
            yield
        finally:
            handle.seek(0)
            if os.name == 'nt':
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


def validate_efi(name: str, data: bytes):
    if name not in EFI_MACHINES or len(data) < 64 or data[:2] != b'MZ':
        raise ValueError(f'invalid EFI loader: {name}')
    pe = int.from_bytes(data[60:64], 'little')
    if pe < 64 or pe + 96 > len(data) or data[pe:pe+4] != b'PE\0\0':
        raise ValueError(f'invalid PE header: {name}')
    machine = int.from_bytes(data[pe+4:pe+6], 'little')
    opt_size = int.from_bytes(data[pe+20:pe+22], 'little')
    magic = int.from_bytes(data[pe+24:pe+26], 'little')
    subsystem = int.from_bytes(data[pe+92:pe+94], 'little')
    if (machine != EFI_MACHINES[name] or opt_size < 72 or pe + 24 + opt_size > len(data)
            or magic != (0x10b if name == 'BOOTIA32.EFI' else 0x20b) or subsystem != 10):
        raise ValueError(f'loader architecture or EFI application header mismatch: {name}')
