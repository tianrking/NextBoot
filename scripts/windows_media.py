"""Read-only Windows disk/volume identity checks for the media updater."""
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import re
import subprocess

from media_partitions import ESP_GUID, BASIC_GUID

VOLUME_PATH = re.compile(r'\\\\\?\\Volume\{[0-9a-fA-F-]{36}\}\\')


def inventory():
    if os.name != 'nt':
        raise ValueError('the native Windows updater requires Windows')
    # All executable PowerShell text is constant. User arguments are never
    # interpolated into a command, and this inventory performs no mutations.
    script = r'''
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
@(Get-Disk | ForEach-Object {
    $disk = $_
    $parts = @(Get-Partition -DiskNumber $disk.Number -ErrorAction SilentlyContinue | ForEach-Object {
        [pscustomobject]@{ Number=$_.PartitionNumber; Guid=$_.Guid;
            Type=$_.GptType; Offset=$_.Offset; Size=$_.Size;
            IsBoot=$_.IsBoot; IsSystem=$_.IsSystem; AccessPaths=@($_.AccessPaths) }
    })
    [pscustomobject]@{ Number=$disk.Number; UniqueId=$disk.UniqueId;
        BusType=[string]$disk.BusType; Style=[string]$disk.PartitionStyle;
        Size=$disk.Size; IsBoot=$disk.IsBoot; IsSystem=$disk.IsSystem;
        IsOffline=$disk.IsOffline; IsReadOnly=$disk.IsReadOnly; Partitions=$parts }
}) | ConvertTo-Json -Depth 5 -Compress
'''
    result = subprocess.run(['powershell.exe', '-NoProfile', '-NonInteractive', '-Command', script],
                            capture_output=True, encoding='utf-8', check=True)
    payload = json.loads(result.stdout.lstrip('\ufeff'))
    return payload if isinstance(payload, list) else [payload]


def volume_info(path):
    if os.name != 'nt' or not VOLUME_PATH.fullmatch(path):
        raise ValueError('expected a Windows volume GUID root')
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    query = kernel.GetVolumeInformationW
    query.argtypes = [wintypes.LPCWSTR, wintypes.LPWSTR, wintypes.DWORD,
                      ctypes.POINTER(wintypes.DWORD), ctypes.POINTER(wintypes.DWORD),
                      ctypes.POINTER(wintypes.DWORD), wintypes.LPWSTR, wintypes.DWORD]
    query.restype = wintypes.BOOL
    label, filesystem = ctypes.create_unicode_buffer(261), ctypes.create_unicode_buffer(261)
    serial, limit, flags = wintypes.DWORD(), wintypes.DWORD(), wintypes.DWORD()
    if not query(path, label, len(label), ctypes.byref(serial), ctypes.byref(limit),
                 ctypes.byref(flags), filesystem, len(filesystem)):
        raise ctypes.WinError(ctypes.get_last_error())
    return {'label': label.value, 'filesystem': filesystem.value, 'serial': serial.value}


def select_disk(disks, number, allow_fixed=False, allow_virtual=False, inspect_volume=volume_info):
    matches = [disk for disk in disks if disk['Number'] == number]
    if len(matches) != 1:
        raise ValueError('selected disk is missing or ambiguous')
    disk = matches[0]
    if disk['IsBoot'] or disk['IsSystem'] or any(p['IsBoot'] or p['IsSystem'] for p in disk['Partitions']):
        raise ValueError('the Windows system or boot disk cannot be updated')
    if disk['IsOffline'] or disk['IsReadOnly'] or disk['Style'] != 'GPT' or not disk['UniqueId']:
        raise ValueError('select an online, writable GPT disk with a stable identity')
    virtual = disk['BusType'] == 'File Backed Virtual'
    if (virtual and not allow_virtual) or (not virtual and disk['BusType'] != 'USB' and not allow_fixed):
        raise ValueError('select USB media, or explicitly opt in with --allow-fixed / --allow-virtual')
    esp, data = [], []
    for part in disk['Partitions']:
        kind = part['Type'].strip('{}').lower()
        if kind not in (ESP_GUID, BASIC_GUID):
            continue
        paths = [path for path in part['AccessPaths'] if isinstance(path, str) and VOLUME_PATH.fullmatch(path)]
        if len(paths) != 1 or not part['Guid']:
            raise ValueError('partition has no unambiguous volume GUID identity')
        info = inspect_volume(paths[0])
        record = {**part, 'Root': paths[0], 'Volume': info}
        if kind == ESP_GUID:
            esp.append(record)
        elif info['label'] == 'NEXTDATA':
            data.append(record)
    if len(esp) != 1 or len(data) != 1 or esp[0]['Root'] == data[0]['Root']:
        raise ValueError('expected one FAT ESP and a separate NEXTDATA partition')
    if esp[0]['Volume']['filesystem'].upper() not in ('FAT', 'FAT16', 'FAT32'):
        raise ValueError('ESP filesystem must be FAT')
    if data[0]['Volume']['filesystem'].upper() not in ('FAT32', 'EXFAT'):
        raise ValueError('native updating currently supports FAT32 or exFAT NEXTDATA')
    # Drive-letter access paths are deliberately excluded from the identity.
    # The stable volume GUIDs continue to identify the same volumes if letters change.
    identity = (disk['UniqueId'], disk['Size'], disk['BusType'], *(
        (part['Guid'], part['Type'], part['Offset'], part['Size'], part['Root'],
         part['Volume']['serial'], part['Volume']['filesystem'], part['Volume']['label'])
        for part in (esp[0], data[0])))
    return {'disk': disk, 'esp': esp[0]['Root'], 'data': data[0]['Root'],
            '_partitions': {'esp': esp[0], 'data': data[0]}, 'identity': identity}


def checked_roots(selection):
    # pathlib cannot realpath a valid \\?\Volume{GUID}\ root on all Python/
    # Windows combinations. Requery the volume metadata and retain the GUID
    # root, which is the identity obtained from Get-Partition.
    roots = {}
    for area in ('esp', 'data'):
        source = selection[area]
        root = Path(source)
        if not root.is_dir():
            raise ValueError('volume root changed during inspection')
        observed = volume_info(source)
        partition = selection['_partitions'][area]
        if (observed['serial'], observed['filesystem'], observed['label']) != (
                partition['Volume']['serial'], partition['Volume']['filesystem'], partition['Volume']['label']):
            raise ValueError('volume metadata changed during inspection')
        roots[area] = root
    return roots
