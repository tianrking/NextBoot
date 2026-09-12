# Returning EFI loader ownership

- Date: 2026-09-12
- Modules: nextboot-boot chain_load, Linux initrd LoadFile2 provider
- Reproduction: generated Linux/GRUB ISO whose EFI payload returns SUCCESS;
  after the payload marker, the parent panicked in allocator.free_pool with
  INVALID_PARAMETER. Earlier smoke tests terminated at the payload marker and
  therefore missed this failure.
- Cause: patch_loaded_image replaced firmware-owned LoadedImage.FilePath with
  a pointer into a Rust Vec. EDK2 frees FilePath when unloading the child; Rust
  subsequently attempted to free the same allocation.
- Fix: retain the path copied by LoadImage. Uninstall the Linux initrd provider
  after a returning/failed loader; only free interfaces firmware released.
  If protocol rollback fails, keep its allocation rather than free a live pointer.
- Validation: release build passes. The same generated ISO now reaches the
  payload marker, the provider-release marker, and the parent's successful return.
  Linux smoke expectations require provider cleanup after the payload returns.
- Boundary: this is preparation for safe menu retries; virtual-device cleanup
  and the complete retry flow are still tracked in the release acceptance ledger.

Firmware ownership reference: [EDK2 CoreUnloadAndCloseImage and CoreLoadImageCommon](https://github.com/tianocore/edk2/blob/master/MdeModulePkg/Core/Dxe/Image/Image.c).
