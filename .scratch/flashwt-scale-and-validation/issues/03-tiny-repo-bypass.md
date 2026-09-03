# 03 - Tiny repo bypass

Status: ready-for-agent

## Problem

Repos under 500 files pay the full 1.3 second floor while plain copy finishes in 0.05 seconds, which kills first run trust.

## Solution

Policy above the Store seam: under 500 files and 8 MB, do worktree add plus recursive copy on write clone and skip ingest plus snapshot lookup entirely.

## Acceptance

- Tiny create finishes near copy time.
- No Store writes happen on the bypass path.
- Threshold is a named constant with a test.

## Verification

- `cargo test -p flashwt-cli --lib`
- Benchmark tiny fixture before and after.

## Comments

- Experience first: this is the largest trust win for small repos.
