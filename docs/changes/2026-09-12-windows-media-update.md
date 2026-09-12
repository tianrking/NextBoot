# Native Windows media updating

The Windows entry point identifies an explicit disk number using its stable
disk ID, GPT partition GUIDs, volume GUID roots, filesystem labels and serials.
It always refuses system/boot disks and requires explicit opt-in for fixed
non-USB disks or test VHDs. It rechecks identity before applying a change.
Existing volume access paths are preserved; no drive letters are assigned and
the product updater has no partition/format operations.

Loader machine validation, pinned runtime migration, exact dry-run previews,
backups, no-op updates and rollback share the existing file-update core.
The default target is x64; other release architectures can be selected.

Validation includes eight disk-selection regressions and an administrator CI
test restricted to a newly created VHD under target. That test covers native
FAT32/exFAT access, 50 runtime resources, ISO/config preservation, rollback and
detach/remount. It uses a non-booted PE fixture, not a claim of Windows OS boot
or physical-device validation. See the current CI result for execution status.
