# 06: Eval CLI subcommand, regression gating, and report cards

**What to build:** A top-level CLI subcommand (`flashwt eval`) and regression gate that parses evaluation parameters, enforces configurable performance and storage budgets, exits non-zero on regressions or fidelity errors, and outputs formatted GitHub pull request report cards.

**Blocked by:** 02: Automated baseline-versus-candidate runner, 03: Volume-level APFS and Linux storage footprint evaluator, 04: Multi-ecosystem fixture matrix and fan-out generator, 05: Automated chaos and fault-injection runner

**Status:** ready-for-agent

- [ ] CLI command interface `flashwt eval run` with arguments `--base <ref>`, `--candidate <ref>`, `--scenarios <list>`, `--runs <n>`, `--threshold <percent>`, and `--output <path>`.
- [ ] Automated regression gating logic asserting that median wall-clock time and key stages (`materialize`, `ingest`) do not exceed the allowed regression budget.
- [ ] Strict fidelity gate asserting zero tolerance for byte, permission, or symlink discrepancies.
- [ ] GitHub PR markdown generator formatting before-and-after tables, stage breakdowns, speedup multipliers, and pass/fail badges.
- [ ] End-to-end integration test exercising `flashwt eval` in a synthetic CI scenario with intentional regression triggers.
