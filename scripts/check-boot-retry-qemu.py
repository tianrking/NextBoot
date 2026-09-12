#!/usr/bin/env python3
"""Select a broken ISO, return to the menu, then boot a different Linux fixture."""
import argparse
import os
from pathlib import Path
import runpy
import socket
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
QA = ROOT / 'target' / 'boot-retry'
PYTHON = sys.executable


def run(*command):
    if os.name == 'nt' and str(command[0]).endswith('.sh'):
        command = (os.environ.get('NEXTBOOT_BASH', 'C:/Program Files/Git/bin/bash.exe'), *command)
    subprocess.run(command, cwd=ROOT, check=True, stdout=subprocess.DEVNULL)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--skip-build', action='store_true')
    args = parser.parse_args()
    QA.mkdir(parents=True, exist_ok=True)
    if not args.skip_build:
        run(str(ROOT / 'scripts/build.sh'), 'release')
    fixture = ROOT / 'target/x86_64-unknown-uefi/release/nextboot-smoke-efi.efi'
    if not fixture.is_file():
        raise RuntimeError('release MiniOS fixture is missing')
    broken = QA / 'broken.efi'
    broken.write_bytes(b'This intentionally is not a PE executable.\n')
    for profile, efi, name in [('generic', broken, '00-broken.iso'), ('linux-grub', fixture, '01-linux.iso')]:
        run(PYTHON, str(ROOT / 'scripts/create-smoke-iso.py'), '--profile', profile,
            '--efi', str(efi), str(QA / name))
    disk = QA / 'retry.img'
    run(str(ROOT / 'scripts/create-release-media.sh'), '--skip-build', '--target', 'x86_64-unknown-uefi',
        '--size', '256', '--growable-max-size', '512', '--output', str(disk),
        '--image', str(QA / '00-broken.iso'), '--image', str(QA / '01-linux.iso'))
    firmware = next((Path(p) for p in [os.environ.get('NEXTBOOT_OVMF_CODE', ''),
                                     '/usr/share/OVMF/OVMF_CODE_4M.fd', '/usr/share/OVMF/OVMF_CODE.fd']
                     if p and Path(p).is_file()), None)
    if firmware is None:
        raise RuntimeError('set NEXTBOOT_OVMF_CODE to x86_64 OVMF firmware')
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    runner = runpy.run_path(str(ROOT / 'scripts/qemu-boot-smoke.py'))
    log = QA / 'serial.log'
    log.write_bytes(b'')
    # The second selection and payload markers cannot be produced by the broken first ISO.
    command = [PYTHON, str(ROOT / 'scripts/qemu-boot-smoke.py'), '--timeout', '90', '--log', str(log),
               '--send-after', 'Phase 3: Displaying boot menu', '--send-key', 'enter', '--qmp-port', str(port)]
    for marker in ['Selected: /ISO/00-broken.iso', 'Boot failed:',
                   'Returned to boot menu; automatic boot timeout disabled',
                   'Selected: /ISO/01-linux.iso', 'NEXTBOOT_SMOKE_EFI_STARTED',
                   'Boot attempt 2 resources released',
                   'Released Linux EFI initrd LoadFile2 provider after loader returned']:
        command += ['--expect', marker]
    command += ['--', os.environ.get('QEMU_BINARY', 'qemu-system-x86_64'), '-machine', 'q35,accel=tcg',
                '-m', '512M', '-net', 'none', '-nographic', '-serial', 'mon:stdio',
                '-qmp', f'tcp:127.0.0.1:{port},server=on,wait=off',
                '-drive', f'if=pflash,format=raw,readonly=on,file={firmware}',
                '-drive', f'if=none,id=nextboot_disk,format=raw,file={disk}',
                '-device', 'nvme,drive=nextboot_disk,serial=NEXTBOOT0,bootindex=1']
    with (QA / 'runner.log').open('w') as output:
        process = subprocess.Popen(command, cwd=ROOT, stdout=output, stderr=subprocess.STDOUT)
        try:
            acknowledged = selected = False
            deadline = time.monotonic() + 100
            while process.poll() is None and time.monotonic() < deadline:
                text = log.read_text(errors='replace')
                if not acknowledged and 'Boot attempt ended; return-to-menu is available' in text:
                    time.sleep(.4)
                    runner['send_qmp_key'](port, 'enter')
                    acknowledged = True
                elif acknowledged and not selected and 'Returned to boot menu;' in text:
                    time.sleep(.4)
                    runner['send_qmp_key'](port, 'down')
                    time.sleep(.2)
                    runner['send_qmp_key'](port, 'enter')
                    selected = True
                time.sleep(.1)
            process.wait(timeout=5)
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
    text = log.read_text(errors='replace')
    report = (QA / 'runner.log').read_text(errors='replace')
    if process.returncode or text.count('Released virtual boot device after loader returned') < 2:
        raise RuntimeError(report + '\nBoth virtual devices must be released.\n' + text[-3000:])
    if text.count('Released boot attempt runtime metadata') < 2 or '[PANIC]' in text:
        raise RuntimeError('runtime metadata cleanup failed or a panic occurred')
    print('passed: broken ISO -> menu -> different Linux ISO; both attempts released their resources')
    return 0


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f'boot retry check failed: {error}', file=sys.stderr)
        raise SystemExit(1)
