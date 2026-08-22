# Lead with tool-agnostic worktree hydration, not a package manager

Fast dependency installs were our first milestone, but pnpm's global virtual
store already delivers that for npm, mature for years, and Nix proves the same
pattern for system packages. Early tools (mantle, cow-cli) clone build
artifacts into worktrees via APFS clonefile but are niche, macOS-only, and
single-purpose. We redirect the wedge to what nobody owns coherently: one
content-addressed store fed by any language's tooling, with instant whole-
directory hydration of worktrees as the first-class primitive, benchmarked
against pnpm and mantle rather than npm.

## Considered options

- Compete on dependency installs: rejected, entering a market pnpm already
  won for the ecosystem that dominates the small-file pain.
- Full agent workspace first: deferred, earned later by winning on speed.
