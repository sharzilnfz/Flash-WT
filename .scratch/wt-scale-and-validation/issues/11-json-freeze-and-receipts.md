# 11 - JSON contract freeze and agent receipts

Status: ready-for-agent

## Problem

Envelope v1 works but payloads drift with ad hoc optional fields. No schema file exists. Agents cannot resume crashed runs from a receipt.

## Solution

Freeze v1 fields with additive only changes. Publish schema plus changelog. Write a receipt file per mutating command. Add lease show in JSON.

## Acceptance

- Schema file exists and CI checks it.
- Crashed create resumes from the receipt.
- Every verb supports machine readable output with stable error codes.

## Verification

- JSON golden tests plus a crash resume integration test.

## Comments

- Type discipline on the wire, not just in Rust types.
