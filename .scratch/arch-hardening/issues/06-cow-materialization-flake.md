# 06: cow_materialization free-disk-space test is flaky under parallel load

Status: ready-for-agent

`cow_materialization::before_first_write_hydrated_files_share_physical_blocks_with_the_blob`
measures free-disk-space delta on the whole volume. Cargo runs test suites in
parallel against the same disk, so concurrent writes pollute the measurement.
Reproduced twice during wave 2 on the untouched base branch, so it is
pre-existing, not caused by the refactor.

Fix options, cheapest first:
1. Measure space used inside the test's own temp dir subtree instead of the
   whole volume (`statfs` on the temp dir before/after, files confined there).
2. Or gate the assertion behind sequential execution (`--test-threads=1`
   cannot be detected portably; prefer option 1).

Done when: the suite passes 10 consecutive parallel full-workspace runs.

## Comments

Completed on `arch/hardening-and-simplify` via wave 2 of the orchestrated build. See git log for the implementing commits.

## Comments

Filed by orchestrator after wave 2: pre-existing flake, root-caused to whole-volume free-disk measurement under parallel cargo load. Not caused by this branch.
