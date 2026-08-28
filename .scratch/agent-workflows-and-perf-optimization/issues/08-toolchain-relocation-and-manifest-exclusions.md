# 08: Post-hydration toolchain relocation and cache manifest exclusions

**What to build:** Run an automated post-hydration sanitization pass after heavy directory materialization. When hydrating `.venv/` directories, rewrite `pyvenv.cfg` base home paths, `bin/activate*` shell scripts, and shebang lines in all executable scripts under `bin/` to match the target worktree path. Leave `.pyc` files for bytecode self-healing. Exclude volatile host compiler caches (`target/debug/incremental/`, `.next/cache`, `node_modules/.vite`) from starter manifests to prevent cache corruption across worktrees.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] Post-hydration pass inspects hydrated `.venv/` directories and updates absolute host paths in `pyvenv.cfg`.
- [ ] Virtual environment `bin/activate*` shell scripts are updated with target worktree directory paths.
- [ ] Shebang lines in `bin/*` script executables are patched to point to the new worktree Python binary.
- [ ] Python `.pyc` cache files are preserved without rewriting.
- [ ] Starter manifests omit volatile compiler incremental caches (`target/debug/incremental/`, `.next/cache`, `node_modules/.vite`).
- [ ] Integration tests verify that hydrated virtual environments and cargo workspaces build and run without path errors.
