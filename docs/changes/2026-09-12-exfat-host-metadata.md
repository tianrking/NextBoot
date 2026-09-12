# exFAT host metadata compatibility

The native Linux volume update check exposed an interoperability problem with
the 131072-byte uncompressed upcase table: exfatprogs 1.2.2 passes its size to
an unsigned-short checksum length, resulting in a computed checksum of zero.
The generator now uses standard identity-run compression, retaining all 65536
mappings while keeping the encoded table below that limit.

File name hashes now use the same per-UTF-16-code-unit mappings as the volume
table. Full Unicode uppercasing could previously expand a name such as sharp s
or alter supplementary characters, producing a hash that the filesystem could
not use to find the file. Stream extensions now set AllocationPossible and
clear NoFatChain for empty files; timestamps contain valid dates.

The image verifier independently decompresses and checks the actual table,
mandatory ASCII mappings, file entry checksums, stream flags and name hashes.
Regression checks include damaged metadata, invalid compression runs, empty
files, accented characters, sharp s and supplementary-plane names.

References: [Microsoft exFAT specification, sections 7.2.5 and 7.6](https://learn.microsoft.com/en-us/windows/win32/fileio/exfat-specification),
[exfatprogs 1.2.2 upcase reader](https://github.com/exfatprogs/exfatprogs/blob/1.2.2/fsck/fsck.c),
[checksum implementation](https://github.com/exfatprogs/exfatprogs/blob/1.2.2/lib/libexfat.c).

Validation: focused local metadata checks; actual filesystem checks and host
update/rollback remain gated by the Release Runtime Integrity CI job.
