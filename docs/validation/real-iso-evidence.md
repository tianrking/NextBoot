# Real ISO validation evidence

Updated 2026-09-12. All results below are x86_64 QEMU/OVMF boot checks using the
actual release-media builder with the pinned compatibility runtime. They do not
establish completed installation or physical-device compatibility.

## Verified checkpoints

| Image | SHA256 | Result | Evidence |
| --- | --- | --- | --- |
| Alpine Standard 3.24.1 x86_64 | `f4dd613206676c62949144c8ad75fc64582099f444dd1485bae104a60f51dd26` | Login prompt | [fa310d0 CI job](https://github.com/tianrking/NextBoot/actions/runs/34677266205/job/103509270161) |
| Debian 13.6.0 netinst amd64 | `65273beed27b2df543b68b65630ba525cfbad8df2b12035732b2dff87d6664e7` | Installer language selection | [922a10f CI job](https://github.com/tianrking/NextBoot/actions/runs/34680482171/job/103518103228) |
| Fedora Workstation 44 x86_64 | `1620295f6a00c27c3208f0c00b8ece4eab1ec69b9002152d97488bf26a426ddf` | GNOME Display Manager starts | [6eaab56 CI job](https://github.com/tianrking/NextBoot/actions/runs/34682234508/job/103522866275) |
| Kali 2026.2 netinst amd64 | `d32f929dacc48134a31461a09f2160d13ad1d26b820cee920446813ca979b39b` | Installer language selection | [fa310d0 CI job](https://github.com/tianrking/NextBoot/actions/runs/34677266205/job/103509270103) |
| Ubuntu 26.04 Server amd64 | `dec49008a71f6098d0bcfc822021f4d042d5f2db279e4d75bdd981304f1ca5d9` | Installer serial-mode selection | [fa310d0 CI job](https://github.com/tianrking/NextBoot/actions/runs/34677266205/job/103509270144) |

The earlier Windows-hosted Ubuntu 2 GiB, single-CPU serial run did not meet the installer UI markers.
A control boot using the same official ISO's kernel/initrd directly also failed
under that configuration. With 4 GiB, two CPUs, QEMU `max` CPU and virtio RNG,
the control reached both the Ubuntu ttyS0 banner and `Continue in basic mode`.
The Linux CI run at fa310d0 passed with 2 GiB and one CPU; the larger test VM
configuration is intended to reduce host-dependent timeout sensitivity.
The local NextBoot path also passed with that configuration on Windows, using
QEMU 11.1.0, OVMF 2024.02 and the fa310d0 loader. This comparison does
not isolate RAM, CPU count, CPU model or entropy as the individual cause, and
does not establish a minimum hardware requirement.

## Reproduce and retain evidence

Run one case at a time, for example:

```sh
python3 scripts/check-real-iso-qemu.py --case alpine-standard
python3 scripts/check-real-iso-qemu.py --case debian-13.6-netinst
python3 scripts/check-real-iso-qemu.py --case ubuntu-26.04-server
python3 scripts/check-real-iso-qemu.py --case kali-2026.2-netinst
```

The test downloads and verifies the pinned ISO, builds the loader, generates
release media, verifies its runtime, and waits for the declared boot markers.
`--skip-build` uses an existing release loader; `--skip-download` requires the
correct ISO and runtime to be cached already. QEMU and OVMF must be installed.
Use `QEMU_BINARY` and `NEXTBOOT_OVMF_CODE` for explicit local tool paths.

Each boot attempt saves `target/real-iso/<case>.serial.log` and
`<case>.evidence.json`. The JSON records source revision and working-tree state,
actual ISO/loader/firmware hashes, QEMU version, VM configuration, expected
markers, elapsed time, result and log digest. A dirty working tree is explicitly
marked; a source commit alone must not be treated as the complete build input.
The result starts as `running`, becomes `pass` only after all markers are seen,
and becomes `fail` on launcher, timeout or marker-check failure. Cancellation
may leave `running`; that is not passing evidence. Download/build/media failures
before the boot attempt do not produce a boot result.

Pull-request and scheduled/main-branch workflows run each ISO independently and
upload available logs and evidence even on failure. The JSON describes boot
verification only, never completed installation or hardware certification.
