# System diagnostics

`flashwt doctor` performs environment validation, probes filesystem Copy-on-Write capabilities, inspects store path permissions, and measures categorized disk consumption across store subsystems.

## Sub-features

- `doctor-store-path` resolves and reports the active content-addressed store root directory.
- `doctor-env-vars` displays the presence and value of all twelve configuration environment variables.
- `doctor-fs-capabilities` probes and reports support for APFS clonefile, FICLONE, and copy_file_range.
- `doctor-disk-usage` inspects and categorizes storage consumption across objects, snapshots, mirrors, refs, and caches.
- `doctor-json-envelope` outputs structured diagnostic data under `status: "ok"` and `command: "doctor"`.

## How to get to it (user POV)

- Run `flashwt doctor` to view a formatted diagnostic summary in the terminal.
- Run `flashwt --json doctor` to obtain machine-readable JSON telemetry.

## Driving it with the shell fixture

Preconditions:

- Fixture loaded (`FLASHWT_BIN`, `FLASHWT_ORIGIN`, `FLASHWT_STORE` set), cwd `$FLASHWT_ORIGIN`.

- **Inspect diagnostic output.** `flashwt doctor`. Output contains `Resolved Store:`, `Environment Variables:`, `Filesystem Capabilities:`, and `Store Disk Usage:`.
- **Inspect JSON envelope.** `flashwt --json doctor`. Envelope `status` is `ok`; `command` is `doctor`; `data.store_path` matches `$FLASHWT_STORE`; `data.fs_capabilities.apfs_clonefile` is boolean; `data.store_disk_usage.total_bytes` is numeric.
- **Proof.** Save the text report and JSON envelope to `artifacts/verify-flashwt/<run-id>/`.

## Gotchas

- `doctor` reports capabilities of the filesystem hosting the store, not necessarily the current working directory.
- Store disk usage reflects uncompressed logical bytes stored across subsystems.
