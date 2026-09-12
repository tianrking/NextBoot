"""Pinned compatibility resources shared by release builds and real ISO QA."""
from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import tempfile
import urllib.request

PROJECT_DIR = Path(__file__).resolve().parents[1]
MANIFEST_PATH = PROJECT_DIR / 'docs/runtime-assets.json'
DEFAULT_DIRECTORY = PROJECT_DIR / 'target/runtime-assets'


def manifest() -> dict:
    return json.loads(MANIFEST_PATH.read_text(encoding='utf-8'))


def checked_bytes(path: Path, entry: dict) -> bytes:
    data = path.read_bytes()
    if len(data) != entry['size'] or hashlib.sha256(data).hexdigest() != entry['sha256']:
        raise ValueError(f'runtime asset checksum mismatch: {path.name}')
    return data


def verify(directory: Path) -> None:
    for entry in manifest()['files']:
        checked_bytes(directory / entry['path'], entry)


def prepare(directory: Path = DEFAULT_DIRECTORY, source: Path | None = None, offline: bool = False) -> Path:
    """Only publish each cache file after checking its immutable upstream digest."""
    config = manifest()
    directory.mkdir(parents=True, exist_ok=True)

    def fetch(entry):
        target = directory / entry['path']
        if target.is_file():
            try:
                checked_bytes(target, entry)
                return
            except ValueError:
                if offline and source is None:
                    raise
        if source is not None:
            data = checked_bytes(source / entry['path'], entry)
        elif offline:
            raise ValueError(f'missing pinned runtime asset: {target}')
        else:
            url = f"https://raw.githubusercontent.com/ventoy/Ventoy/{config['commit']}/{entry['source_path']}"
            with urllib.request.urlopen(url, timeout=60) as response:
                data = response.read(entry['size'] + 1)
            if len(data) != entry['size'] or hashlib.sha256(data).hexdigest() != entry['sha256']:
                raise ValueError(f'download checksum mismatch: {entry["path"]}')
        with tempfile.NamedTemporaryFile(dir=directory, delete=False) as temporary:
            temporary.write(data)
            temporary_path = Path(temporary.name)
        try:
            os.replace(temporary_path, target)
        finally:
            temporary_path.unlink(missing_ok=True)

    with ThreadPoolExecutor(max_workers=4) as pool:
        list(pool.map(fetch, config['files']))
    verify(directory)
    return directory


def release_files(directory: Path) -> list[tuple[str, bytes]]:
    """All runtime files, including original notices and resulting patched hashes."""
    from qemu.disk_image.ventoy_assets import patch_ventoy_cpio

    config = manifest()
    files = []
    inventory = []
    for entry in config['files']:
        data = checked_bytes(directory / entry['path'], entry)
        if entry['path'] == 'ventoy.cpio':
            data = patch_ventoy_cpio(data)
        path = '/ventoy/' + entry['path']
        files.append((path, data))
        inventory.append({'path': path, 'sha256': hashlib.sha256(data).hexdigest(), 'size': len(data),
                          'upstream_sha256': entry['sha256']})
    for notice in sorted((PROJECT_DIR / 'docs/third-party/ventoy').glob('*.txt')):
        files.append(('/ventoy/License/' + notice.name, notice.read_bytes()))
    files.append(('/ventoy/NEXTBOOT-NOTICE.txt', (PROJECT_DIR / 'docs/third-party/VENTOY-NOTICE.txt').read_bytes()))
    provenance = {'version': config['version'], 'commit': config['commit'], 'source_archive': config['source_archive'],
                  'modification': 'NextBoot dm-table partition-offset patch; see scripts/qemu/disk_image/ventoy_assets.py',
                  'files': inventory}
    files.append(('/ventoy/nextboot-runtime.json', (json.dumps(provenance, indent=2) + '\n').encode()))
    return files
