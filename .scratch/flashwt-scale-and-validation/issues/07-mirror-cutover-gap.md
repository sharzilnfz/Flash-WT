# 07 - Mirror cutover dual write gap

Status: ready-for-agent

## Problem

Snapshot hydrations record only a snapshot hash in the Mirror. Legacy sweepers that count refs can collect member blobs the new path treats as live.

## Solution

Keep writing refs for snapshot members until no legacy sweeper remains, or gate the cutover on audit parity for snapshot children.

## Acceptance

- Mixed version run never collects live snapshot members.
- Cutover steps are documented in the GC module.

## Verification

- GC mirror tests plus a mixed mode integration test.

## Comments

- Respects ADR-0004 mark and sweep design. Safety over disk savings during transition.
