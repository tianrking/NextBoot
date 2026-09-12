# FAT directory structure and host-write validation

- Date: 2026-09-12
- Modules: release/QEMU FAT16 and FAT32 image generators and verifiers
- Found during: Linux-mounted ESP update validation. The kernel returned I/O
  errors when opening the generated loader for replacement. Generator inspection
  found missing `.` and `..` entries in all FAT subdirectories.
- Change: emit dot entries with the correct self/parent clusters, including zero
  for a parent root; write the root volume-label entry and valid DOS dates.
  FAT32 free-cluster accounting also excludes the two reserved cluster numbers.
- Verification: image readers now reject missing/mismatched self dot entries.
  CI runs read-only dosfstools/exfatprogs checks before mounting, then performs
  update and rollback on both exFAT and FAT32 DATA volumes. Kernel diagnostics
  are retained on failure. Host-write results must be checked on the final commit.

Directory requirements are specified in the original
[Microsoft FAT specification](https://www.scs.stanford.edu/~zyedidia/docs/_other/fat.pdf).
Earlier boot-only or custom-reader checks did not establish host write validity.
