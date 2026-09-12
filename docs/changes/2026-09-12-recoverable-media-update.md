# Recoverable loader and runtime updating

- Date: 2026-09-12
- Modules: host updater, runtime packaging, host validation
- API: `media_update.apply(esp, data, payload, dry_run=False)` and
  `media_update.rollback(esp, data, identifier=None)` operate only on verified,
  mounted roots. Device discovery and mount ownership remain the frontend's job.
- CLI: `update-media.sh` now migrates pinned runtime resources by default;
  `--loaders-only` retains the deliberate ESP-only operation. `--rollback ID`
  restores a saved transaction; `--rollback pending` recovers an interrupted one.
- Files: per-volume `.nextboot-update/<id>` retains staged files and originals;
  ESP manifests bind allowed targets and old/new digests. A pending transaction
  prevents another update. Rollback validates all backups and targets first.
- Validation: local preservation, interruption, failure recovery, conflict and
  real-artifact tests pass; Linux frontend FAT/exFAT verification is included in
  CI. Physical devices and native Windows volume discovery remain separate gates.

See [the user and recovery guide](../update-and-rollback.md) for exact commands,
backup retention and power-loss limitations.
