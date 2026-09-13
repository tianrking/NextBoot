# Windows exFAT allocation-bitmap chain repair

The rc.1 image advertised an Allocation Bitmap `DataLength` sized for a future
expanded volume. rc.2 changed that field to the current volume's exact length,
but left the longer preallocated FAT chain in place. Windows rejects both forms:
the Allocation Bitmap byte length, the number of FAT-chain clusters needed to
store it, and the current `ClusterCount` must agree.

rc.3 writes an exact initial bitmap chain and limits first-boot expansion to
128 GiB, which fits inside its single 128 KiB bitmap cluster. The UEFI
first-boot growth path and the offline growth tool update the Allocation Bitmap
entry before they publish the larger exFAT boot geometry. The generated-image
verifier now rejects a bitmap whose chain length does not match `DataLength`.

Validation includes all three UEFI builds, generated-image metadata checks,
offline growth, and a QEMU boot that expands 256 MiB media to 1 GiB through the
firmware path followed by independent post-growth verification. Physical
Windows removable-media mounting remains the final release-candidate gate.

Reference: [Microsoft exFAT specification, sections 3.1.9, 3.1.10, 4.1 and 7.1](https://learn.microsoft.com/en-us/windows/win32/fileio/exfat-specification).
