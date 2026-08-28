# 05: Upfront filesystem capability probing and Linux copy acceleration

**What to build:** Probe filesystem parameters via `statfs(2)` once during store initialization and cache device identifiers, filesystem types, and reflink capabilities. On Linux ext4, bypass failing `FICLONE` ioctls and dispatch parallel `copy_file_range(2)` across worker threads for in-kernel zero-copy page splicing. On btrfs and XFS, execute `ioctl(FICLONE)`. Surface `CROSS_DEVICE_COPY_DEGRADATION` diagnostic warnings in `--json` and human output when cross-volume boundaries force fallback byte copies.

**Blocked by:** 01: Versioned JSON output envelope across CLI commands.

**Status:** ready-for-agent

- [x] Store initialization probes filesystem parameters via `statfs(2)` and caches `(device_id, fs_type, reflink_capable)`.
- [x] Linux ext4 hydration skips `FICLONE` ioctls and runs parallel `copy_file_range(2)` across worker threads.
- [x] Linux btrfs and XFS execute `ioctl(FICLONE)` for copy-on-write extent sharing.
- [x] Cross-device copies and unsupported mounts fall back to parallel buffered copy with `posix_fadvise(POSIX_FADV_SEQUENTIAL)`.
- [x] Storage boundary fallback copies emit `CROSS_DEVICE_COPY_DEGRADATION` diagnostic warnings in `--json` output.
- [x] Copy engine unit tests and benchmarks verify correct strategy selection and zero-copy performance on supported filesystems.
