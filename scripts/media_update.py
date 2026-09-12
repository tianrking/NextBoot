"""Journaled file replacement and explicit rollback, without partition operations."""
import json
import os
from pathlib import Path
import re
import shutil
import uuid

from media_update_io import (ADMIN, digest, durable_write, ensure_directory, file_digest, owned_paths,
                             roots_for, safe_path, sync_dir, update_lock, write_json)


def target_path(roots, entry):
    area, relative = entry['area'], entry['path']
    if area not in roots or relative not in owned_paths().get(area, set()):
        raise ValueError(f'unowned update target: {area}:{relative}')
    return safe_path(roots[area], relative)


def transaction_path(roots, area, identifier, relative):
    if not re.fullmatch('[0-9a-f]{32}', identifier):
        raise ValueError('invalid transaction identifier')
    return safe_path(roots[area], f'{ADMIN}/{identifier}/{relative}')


def plan(roots, payload):
    entries, seen = [], set()
    for area, relative, content in payload:
        key = (area, relative.casefold())
        if key in seen:
            raise ValueError('duplicate update target')
        seen.add(key)
        entry = {'area': area, 'path': relative, 'new_sha256': digest(content)}
        path = target_path(roots, entry)
        original = path.read_bytes() if path.is_file() else None
        entry['old_sha256'] = file_digest(path)
        if entry['old_sha256'] != entry['new_sha256']:
            entries.append((entry, content, original))
    return entries


def load_journal(roots, identifier=None):
    path = (safe_path(roots['esp'], ADMIN + '/pending.json') if identifier is None else
            transaction_path(roots, 'esp', identifier, 'manifest.json'))
    journal = json.loads(path.read_text(encoding='utf-8'))
    if journal.get('schema') != 1 or journal.get('status') not in {'prepared', 'committed', 'rolled_back'}:
        raise ValueError('invalid update journal')
    actual = journal['id']
    transaction_path(roots, 'esp', actual, 'manifest.json')
    if identifier is not None and actual != identifier:
        raise ValueError('journal identity mismatch')
    entries = journal.get('entries')
    if not isinstance(entries, list) or not entries:
        raise ValueError('empty or invalid update journal')
    seen = set()
    for entry in entries:
        target_path(roots, entry)
        key = (entry['area'], entry['path'].casefold())
        if key in seen:
            raise ValueError('duplicate journal target')
        seen.add(key)
        for name in ('old_sha256', 'new_sha256'):
            value = entry[name]
            if value is None and name == 'old_sha256':
                continue
            if not isinstance(value, str) or not re.fullmatch('[0-9a-f]{64}', value):
                raise ValueError('invalid journal digest')
    for area in {entry['area'] for entry in entries}:
        marker = transaction_path(roots, area, actual, 'volume.json')
        if json.loads(marker.read_text()) != {'id': actual, 'area': area}:
            raise ValueError('transaction belongs to a different volume')
    return journal


def finish(roots, journal, status):
    journal['status'] = status
    write_json(transaction_path(roots, 'esp', journal['id'], 'manifest.json'), journal)
    pending = safe_path(roots['esp'], ADMIN + '/pending.json')
    if pending.exists():
        if json.loads(pending.read_text())['id'] != journal['id']:
            raise ValueError('another update needs recovery first')
        pending.unlink()
        sync_dir(pending.parent)


def restore(roots, journal):
    """Validate every backup and target before changing any file during rollback."""
    originals = []
    for index, entry in enumerate(journal['entries']):
        path = target_path(roots, entry)
        current = file_digest(path)
        if current not in {None, entry['old_sha256'], entry['new_sha256']}:
            raise ValueError(f'file changed outside this update; preserving it: {entry["path"]}')
        original = None
        if entry['old_sha256'] is not None:
            backup = transaction_path(roots, entry['area'], journal['id'], f'old/{index}')
            original = backup.read_bytes()
            if digest(original) != entry['old_sha256']:
                raise ValueError(f'backup checksum mismatch: {entry["path"]}')
        originals.append(original)
    for entry, original in zip(journal['entries'], originals):
        path = target_path(roots, entry)
        if file_digest(path) == entry['old_sha256']:
            continue
        if original is None:
            path.unlink(missing_ok=True)
            sync_dir(path.parent)
        else:
            durable_write(path, original)
        if file_digest(path) != entry['old_sha256']:
            raise OSError(f'rollback verification failed: {entry["path"]}')
    finish(roots, journal, 'rolled_back')


def rollback(esp: Path, data: Path | None, identifier=None):
    roots = roots_for(esp, data)
    with update_lock(roots['esp']):
        journal = load_journal(roots, identifier)
        pending = safe_path(roots['esp'], ADMIN + '/pending.json')
        if pending.exists() and json.loads(pending.read_text())['id'] != journal['id']:
            raise ValueError('another update needs recovery first')
        restore(roots, journal)
        return journal['id']


def apply(esp: Path, data: Path | None, payload, dry_run=False):
    roots = roots_for(esp, data)
    payload = list(payload)
    # Validate paths and inputs before even creating the administration directory.
    initial = plan(roots, payload)
    if dry_run:
        if safe_path(roots['esp'], ADMIN + '/pending.json').exists():
            raise ValueError('unfinished update found; run rollback before updating')
        return [entry for entry, _, _ in initial]
    with update_lock(roots['esp']):
        pending = safe_path(roots['esp'], ADMIN + '/pending.json')
        if pending.exists():
            raise ValueError('unfinished update found; run rollback before updating')
        changes = plan(roots, payload)
        if not changes:
            return None
        identifier = uuid.uuid4().hex
        areas = {entry['area'] for entry, _, _ in changes}
        for area in areas:
            needed = sum(len(new) + len(old or b'') for entry, new, old in changes if entry['area'] == area)
            if shutil.disk_usage(roots[area]).free < needed + 65536:
                raise ValueError(f'insufficient free space for staged update and backup on {area}')
        for area in areas | {'esp'}:
            marker = transaction_path(roots, area, identifier, 'volume.json')
            write_json(marker, {'id': identifier, 'area': area})
        for index, (entry, content, original) in enumerate(changes):
            stage = transaction_path(roots, entry['area'], identifier, f'new/{index}')
            durable_write(stage, content)
            if file_digest(stage) != entry['new_sha256']:
                raise OSError('staged file checksum mismatch')
            if original is not None:
                backup = transaction_path(roots, entry['area'], identifier, f'old/{index}')
                durable_write(backup, original)
                if file_digest(backup) != entry['old_sha256']:
                    raise OSError('backup checksum mismatch')
        journal = {'schema': 1, 'id': identifier, 'status': 'prepared',
                   'entries': [entry for entry, _, _ in changes]}
        write_json(transaction_path(roots, 'esp', identifier, 'manifest.json'), journal)
        write_json(pending, journal)
        try:
            for index, (entry, _, _) in enumerate(changes):
                path = target_path(roots, entry)
                if file_digest(path) != entry['old_sha256']:
                    raise ValueError(f'file changed while preparing update: {entry["path"]}')
                ensure_directory(path.parent)
                os.replace(transaction_path(roots, entry['area'], identifier, f'new/{index}'), path)
                sync_dir(path.parent)
                if file_digest(path) != entry['new_sha256']:
                    raise OSError(f'installed file checksum mismatch: {entry["path"]}')
            finish(roots, journal, 'committed')
        except Exception as error:
            try:
                restore(roots, load_journal(roots))
            except Exception as recovery_error:
                raise RuntimeError(f'update failed; recovery required: {recovery_error}; transaction {identifier}') from error
            raise RuntimeError(f'update failed and original files were restored: {error}') from error
        return identifier
