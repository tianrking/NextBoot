# Boot attempt lifetimes and recovery

- Date: 2026-09-12
- Modules: nextboot-boot, nextboot-virtio registration
- API: RegisteredVirtualBlockIo gains consuming uninstall; virtual boot devices
  own their registrations until a loader returns. Uninstall removes interfaces
  before releasing storage; failed removals retain their backing memory.
- Behavior: boot errors and returning applications offer a return to the menu.
  The second menu has no automatic timeout, so a broken default cannot cause a
  retry loop. Global menu authentication remains valid for that session; image
  authentication still applies to every selection.
- Runtime: delete attempt-owned OS parameters before releasing referenced runtime
  pools. Refused cleanup stops retries, preserving still-referenced allocations.
  The user is asked to press a key to cold-restart; NextBoot must not return and
  unload its code while firmware still retains its callback pointers.
- Firmware integration: register BlockIO, BlockIO2 and DevicePath; allow firmware
  to install its DiskIo adapter. Preinstalling our own DiskIo while connecting
  the firmware adapter reproduced a page fault in OVMF's Generic Disk I/O Driver
  during DisconnectController. The driver failed Start on duplicate DiskIo,
  retained a BlockIo2 BY_DRIVER open, then interpreted our interface as its own
  private context in Stop. Removing competing DiskIo registrations permits
  normal driver ownership and disconnection.
- Regression: check-boot-retry-qemu.py builds actual release media containing an
  intentionally invalid EFI ISO and a returning Linux fixture, uses QMP keyboard
  events to switch between them, and requires both attempts' device and runtime
  cleanup markers. The Release Reliability workflow runs this test.

Reference: [EDK2 DiskIoDriverBindingStart/Stop](https://github.com/tianocore/edk2/blob/master/MdeModulePkg/Universal/Disk/DiskIoDxe/DiskIo.c).
