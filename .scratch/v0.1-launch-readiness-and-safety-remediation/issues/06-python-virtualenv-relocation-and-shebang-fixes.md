Status: ready-for-agent

# Issue 06: Python Virtualenv Relocation & Shebang Fixes

## Problem
1. `relocate_venv` only inspects `pyvenv.cfg` and `bin/`, ignoring `.pth` and `_editable.*.pth` files in `site-packages/`. Worktrees with editable package installations continue importing code from the source repository.
2. Shebang rewriting fails silently on non-UTF-8 launcher binaries starting with `#!`.
3. `find_venvs` follows directory symlinks without cycle detection, panicking with stack overflow on recursive symlinks.

## Requirements
1. Discover and rewrite `.pth` files in `lib/python*/site-packages/` during virtualenv relocation.
2. Search and replace shebang lines in `bin/` executables directly in byte buffers without requiring whole-file UTF-8 conversion.
3. Add cycle detection (visited inode set) in `find_venvs` to prevent unbounded recursion.

## Verification
- Add integration test hydrating a virtual environment with `.pth` files and asserting that import paths point to the destination worktree.
- Add test asserting that binary launcher executables with shebangs have their headers rewritten.
