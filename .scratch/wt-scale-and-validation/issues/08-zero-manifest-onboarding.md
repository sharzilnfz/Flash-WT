# 08 - Zero manifest onboarding

Status: ready-for-agent

## Problem

First run expects a manifest file. Missing config plus slower than copy tiny runs destroy first run trust.

## Solution

Ship working defaults, keep demo as the trial path, and warn when hydration saves nothing.

## Acceptance

- Fresh repo with no config creates a working worktree.
- Zero savings run prints a plain reason.

## Verification

- CLI onboarding tests plus manual first run on an empty repo.

## Comments

- Experience first over implementation convenience.
