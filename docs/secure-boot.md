# Secure Boot Workflow

NextBoot can be signed for Secure Boot with a local owner-controlled key. This
is useful for personal machines, lab devices, and firmware where you can enroll
your own certificate into the Secure Boot database or a shim MOK list.

This is not a production distribution workflow. The project does not yet ship a
Microsoft UEFI CA signed shim, a public revocation/SBAT policy, or a release key
management process.

## Tooling

`scripts/secure-boot.sh` provides the supported local workflow:

```bash
# Show required tools, paths, and the EFI binary that will be used by default.
./scripts/secure-boot.sh status

# Create a local self-signed test key and certificate pair.
./scripts/secure-boot.sh generate-test-cert

# Build NextBoot, then sign the EFI binary with the local key.
./scripts/build.sh release
./scripts/secure-boot.sh sign

# Verify the signature on hosts that have sbverify.
./scripts/secure-boot.sh verify
```

The default files are written under `target/secure-boot/`:

- `nextboot-db.key`: private signing key. Keep this private.
- `nextboot-db.crt`: PEM certificate used by `sbsign` and `sbverify`.
- `nextboot-db.cer`: DER certificate suitable for many firmware db or MOK
  enrollment interfaces.
- `nextboot-boot-signed.efi`: signed output produced by `sign`.

The script intentionally fails with an installation hint when a required tool is
missing. `openssl` is required for certificate generation; `sbsign` and
`sbverify` come from sbsigntools.

## Enrollment Model

After signing, the firmware must trust the certificate before it will load the
binary with Secure Boot enabled. Common local paths are:

- Enroll `target/secure-boot/nextboot-db.cer` into firmware `db` if your UEFI
  setup exposes key management.
- Enroll the same certificate through shim's MOK manager when booting through a
  trusted shim.
- Keep Secure Boot disabled on machines where you cannot enroll your own key.

Once the certificate is enrolled, install the signed EFI binary as
`EFI/BOOT/BOOTX64.EFI` for `x86_64-unknown-uefi` or `EFI/BOOT/BOOTAA64.EFI`
for `aarch64-unknown-uefi` on the ESP instead of the unsigned build output.

## Current Limits

- No bundled shim or Microsoft UEFI CA signed first-stage binary.
- No SBAT/revocation metadata policy for public releases.
- No automated `.auth` variable update generation. Some firmware requires
  platform-owner tooling such as `efitools` for authenticated db updates.
- QEMU smoke tests in this repository validate boot behavior, not firmware
  Secure Boot enforcement. Signature verification is delegated to `sbverify`
  where available.
