# 12 - Fix docs promise gap

Status: ready-for-human

## Problem

Domain docs promise a watcher that keeps the Tree coherent with the Store. Only explicit hydrate exists. Readers expect sync and get stale trees.

## Solution

Fix the docs now. Optionally ship a dry run watch log showing what would sync. No daemon in this wave.

## Acceptance

- Docs name explicit hydration as the only mechanism.
- No promise of background sync remains.

## Verification

- Docs review. No code test required.

## Comments

- Subtract the false promise before adding any daemon.
