Status: ready-for-agent

# Issue 04: Linux copy_file_range and Btrfs Subvolume Hardening

## Problem
1. `copy_file_range` treats `rc == 0` as normal completion even if `remaining > 0`, silently truncating files on early EOF or extent boundaries. It also lacks an `EINTR` signal retry loop.
2. Device ID probing checks `meta.dev()`, which returns distinct device IDs for Btrfs subvolumes on the same filesystem. This falsely triggers `is_cross_device` and disables zero-copy `FICLONE` reflinks.
3. `copy_tree` opens FIFOs/named pipes in blocking mode, causing unbounded process hangs.

## Requirements
1. In `copy_file_range.rs`, treat `rc == 0` when `remaining > 0` as a failure and fall back to buffered copy. Add an `EINTR` signal retry loop.
2. Support cross-subvolume reflinks on Btrfs by not treating distinct subvolume device IDs as cross-device lockouts for `ioctl(FICLONE)`.
3. Filter out non-regular special files (FIFOs, sockets, device nodes) in directory copy traversal.

## Verification
- Add integration tests verifying large file copies with simulated short reads and signal interruptions.
- Test directory copying on trees containing named pipes to ensure non-blocking execution.
