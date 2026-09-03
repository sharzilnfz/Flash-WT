# 09 - Linux parity diagnostic

Status: ready-for-agent

## Problem

Off macOS the fast path disables silently. Linux users see slow runs with no reason.

## Solution

Print mechanism plus reason in human output on fallback. Publish per OS speedup numbers with releases.

## Acceptance

- Fallback run names the backend and why it refused acceleration.
- Docs carry a Linux versus macOS table.

## Verification

- CLI output tests covering fallback messaging.

## Comments

- Most of the target millions run Linux. Silent slowness reads as broken.
