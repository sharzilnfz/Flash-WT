# 10 - Doctor plus store observability

Status: ready-for-agent

## Problem

Policy hides in env vars. Store size is invisible. Sweep has no dry run. Disks fill with no warning.

## Solution

Add doctor showing resolved config plus probe results. Add store du. Add sweep dry run. Read only. No lifecycle change.

## Acceptance

- One command prints resolved store path, flags, probe, and store size.
- Sweep dry run reports what would delete without deleting.

## Verification

- CLI golden tests for doctor and dry run output.

## Comments

- Boundary discipline: validate once at startup, make the result inspectable.
- Builds on the ticket 05 two phase sweep protocol doc in `.scratch/deep-hydration-architecture`. Doctor surfaces the policy. That doc defines the mechanism. The dry run output must match the documented phase order.
