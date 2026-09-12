# Updating existing media and restoring a previous version

Development implementation, updated 2026-09-12. The Linux/macOS frontend now
updates loaders and the pinned compatibility runtime without formatting the
disk. It preserves `/ISO`, user configuration and files outside the explicit
update allowlist. The native Windows device frontend is still being developed.

## Update an existing disk

Build the architecture you need, inspect the actual disk, then run the update:

```sh
TARGET=x86_64-unknown-uefi ./scripts/build.sh release
./scripts/update-media.sh list
./scripts/update-media.sh --target x86_64-unknown-uefi --dry-run /dev/sdX
./scripts/update-media.sh --target x86_64-unknown-uefi /dev/sdX
```

Replace `/dev/sdX` with the whole NextBoot disk. macOS uses `/dev/diskN`.
Do not select an individual partition. The default target is `all`; all three
architecture artifacts must exist when that target is used. The helper prefers
the release artifact and falls back to a built debug artifact if necessary.
Use release artifacts for media intended for normal use.

Partition detection uses GPT type, label and filesystem metadata, supporting
both earlier and current partition order. The ESP must be FAT and DATA must be
the separate `NEXTDATA` basic-data partition. `--force` cannot bypass ESP
validation or supply a missing DATA volume for runtime migration.
Whole Linux loop disks are rejected by default. The developer-only `--allow-loop`
option permits explicit image testing while retaining all partition validation.

The first update prepares the pinned runtime cache under `target/runtime-assets`.
`--runtime-dir DIR` chooses a different cache. Downloads and cached resources
are checked against the repository's fixed manifest before installation.
The updater includes the same patched runtime, licenses and provenance files
as the release builder. This also repairs older media that omitted those files.

`--loaders-only` explicitly skips runtime migration and DATA mounting. This is
useful for a deliberate loader-only operation, but does not repair a medium
whose runtime is missing. `--yes` skips the interactive confirmation. Dry-run
inspects partition metadata and prints the operations; it does not mount, download
or write. File validation and the precise changed-file list are available from
the mounted-volume backend's `--dry-run` option.

## Backups and rollback

Every changed file is staged and checksum-verified. Existing bytes are copied
and verified before replacements begin. Runtime files are replaced before EFI
loaders. Each volume retains its own backup
under `.nextboot-update/<transaction-id>/`; the ESP holds the transaction
manifest and a pending-operation record. The updater prints the transaction ID.
Identical files are skipped, and a repeated update does not create a new backup
transaction if nothing needs replacement.

To restore a completed transaction:

```sh
./scripts/update-media.sh --rollback TRANSACTION_ID /dev/sdX
```

To recover after process interruption or an update that could not finish its
automatic rollback:

```sh
./scripts/update-media.sh --rollback pending /dev/sdX
```

Use `--loaders-only` during recovery only for a transaction that did not involve
DATA. Recovery requires the matching volumes and intact backups. It does not
need a newly built loader or downloaded runtime. A pending transaction blocks
further updates until recovery succeeds.

Rollback checks all target and backup hashes before writing. It restores
replaced bytes and removes only files newly introduced by that transaction.
It refuses to overwrite subsequent external modifications or use a mismatched
DATA volume. Keep the error and transaction ID if recovery is refused; deleting
the pending record or backups does not repair the interrupted operation.

Backups are retained after success and rollback. Automatic backup pruning is
not implemented yet; their disk space must remain available for recovery.
File writes are flushed and individual replacements use same-volume rename.
This provides journaled recovery, not an atomic transaction spanning both
filesystems or a guarantee against filesystem/device corruption during power
loss. Physical interruption and remount behavior still require device testing.

## Mounted-volume backend and validation scope

`scripts/update-mounted-media.py` is the common file-update backend. It expects
already verified mounted roots; it does not identify or mount a disk itself.
For example, on disposable verified test volumes:

```sh
python3 scripts/update-mounted-media.py --esp /mnt/esp --data /mnt/data \
  --loader BOOTX64.EFI=target/x86_64-unknown-uefi/release/nextboot-boot.efi --dry-run
```

Current local evidence includes fault-injected write failure, process interruption,
retry after recovery, failed rollback with a retained journal, corrupt backups,
external file changes, wrong-volume refusal, concurrent updates, insufficient
space, path restrictions and actual EFI/runtime migration with ISO/config hashes
preserved. Windows runs these filesystem tests; this does not yet establish a
Windows device-update workflow. Linux CI additionally exercises the shell
frontend on disposable loopback FAT/exFAT volumes. Physical Linux/macOS device
checks remain required before claiming broad host compatibility.
