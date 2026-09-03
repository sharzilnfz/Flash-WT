# Storage deduplication and CoW isolation

`flashwt` achieves near-zero additional disk usage across worktrees by sharing physical storage blocks via Copy-on-Write (`fclonefileat` / `clonefile` on APFS, `FICLONE` on Linux), while maintaining strict break-on-write mutation isolation so changes in one worktree never pollute siblings or the content-addressed store.

## Sub-features

- `dedup-block-sharing` shares identical underlying storage blocks between store blobs and multiple worktrees.
- `dedup-accounting` tracks logical file sizes and reports total disk saved via `flashwt list`.
- `cow-break-on-write` allocates private blocks exclusively when a hydrated worktree file is modified, preserving shared blocks for all other worktrees.
- `store-immutability` guarantees store blobs remain strictly immutable and unaffected by worktree edits.
- `hardlink-alternative` provides optional `FLASHWT_HARDLINK=1` inode sharing with enforced read-only permissions (requires `FLASHWT_SNAPSHOTS=0` on macOS APFS).

## How to get to it (user POV)

- Create multiple worktrees from a shared codebase using `flashwt new <name>`.
- Check deduplication savings across all worktrees with `flashwt list` or `flashwt ls`.
- Mutate, build, or edit files in any worktree without affecting sibling worktrees or the store.

## Driving it with the shell fixture

Preconditions:

- Fixture loaded on macOS APFS volume (`FLASHWT_ORIGIN`, `FLASHWT_STORE` set).
- `FLASHWT_NO_TINY_BYPASS=1` exported so small fixtures populate the content-addressed store.
- Fixture contains heavy files (`heavy/` with 40 files).

- **Create first worktree.** `flashwt --json new worktree-one --dir "$FLASHWT_FIXTURE/worktree-one"`. Note store object count: `find "$FLASHWT_STORE/objects" -type f | wc -l`.
- **Create second worktree.** `flashwt --json new worktree-two --dir "$FLASHWT_FIXTURE/worktree-two"`. Object count in `$FLASHWT_STORE/objects` remains identical; no new blobs are written to the store.
- **Verify disk space accounting.** `flashwt --json list` reports `data.total_disk_saved` representing logical duplicate bytes spared.
- **Verify physical block sharing.** On macOS, inspect inode numbers: `ls -i "$FLASHWT_FIXTURE/worktree-one/heavy/pkg00/nested/file-0.txt"` and `ls -i "$FLASHWT_FIXTURE/worktree-two/heavy/pkg00/nested/file-0.txt"`. Inodes are distinct (private writable inodes), yet physical storage blocks are shared at the filesystem extent level.
- **Test break-on-write mutation isolation.** Mutate a file in `worktree-one`: `echo "MUTATION" > "$FLASHWT_FIXTURE/worktree-one/heavy/pkg00/nested/file-0.txt"`. Verify `worktree-two` file is completely unaffected: `cat "$FLASHWT_FIXTURE/worktree-two/heavy/pkg00/nested/file-0.txt"` matches original `fake-heavy file 0 of 40`. Verify store blob is unaffected: search matching blob in `$FLASHWT_STORE/objects/` and confirm original content remains intact.
- **Verify hardlink mode.** `FLASHWT_HARDLINK=1 FLASHWT_SNAPSHOTS=0 flashwt --json new worktree-hl --dir "$FLASHWT_FIXTURE/worktree-hl"`. Inode numbers match store blobs, and writing in-place fails with Permission Denied.
- **Proof.** Save file checksums, `flashwt list` envelope, and store blob verification to `artifacts/verify-flashwt/<run-id>/`.

## Gotchas

- Copy-on-Write block sharing occurs at the filesystem allocation layer; standard tools like `ls -l` show logical file sizes, not physical disk allocation.
- On macOS APFS, `FLASHWT_HARDLINK=1` requires `FLASHWT_SNAPSHOTS=0`; otherwise, directory snapshot cloning takes precedence over hardlink materialization.
- `FLASHWT_NO_HARDLINK=1` forces plain byte copies, taking precedence over `FLASHWT_HARDLINK`.
- Small fixtures with under 500 files trigger tiny repository bypass unless `FLASHWT_NO_TINY_BYPASS=1` is exported.
