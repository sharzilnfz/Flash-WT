# Spec: Market Launch Readiness, Presentation Consolidation, and Deep Architecture

Status: ready-for-agent

## Problem Statement

As `wt` prepares for public product launch, several friction points in user experience, presentation consistency, and module depth prevent it from delivering a flawless day-one developer experience:

- **Missing Shell Autocompletions**: Developers using modern shells (Bash, Zsh, Fish) expect tab-completions for subcommands, flags, and branch names out of the box. Currently, typing `wt <TAB>` offers no completion candidates, increasing command-line friction.
- **Fragmented Presentation and Formatting Logic**: Terminal output, byte scaling, duration formatting, number grouping, and aligned receipt tables are independently re-implemented across four distinct command handlers with subtle mathematical differences and duplicated formatting code.
- **Shallow Worktree Discovery and Git Lifecycle Seams**: Worktree enumeration, gitdir resolution, porcelain parsing, and merge-base ancestor checks are scattered across command handlers rather than encapsulated behind a cohesive workspace module.
- **Leaked Ingestion Invariants**: Ingesting directory trees, executing bulk walker syscalls, and maintaining validation cache metadata reside in the CLI crate rather than within the content-addressed storage engine where storage invariants belong.
- **Outdated Launch Documentation**: The primary README documents legacy manual snapshot flags (`WT_SNAPSHOTS=1`) as prerequisites rather than highlighting automated macOS APFS defaults and the modern 3-verb workflow (`wt new`, `wt clean`, `wt list`, `wt demo`).

## Solution

Deliver the final launch polish and architectural deepening across four unified pillars:

1. **Shell Autocompletion Engine (`wt completions <shell>`)**: Integrate shell completion generation via command-line argument parsing definitions, enabling instant tab-completion for subcommands and flags across Bash, Zsh, and Fish, with auto-installation support in the install script.
2. **Deep Presentation and Formatting Module**: Consolidate human-friendly unit conversion (bytes, durations, numbers), terminal receipt banners, and aligned tables behind a single presentation module, guaranteeing uniform visual typography across all commands.
3. **Deep Workspace Module**: Consolidate worktree lifecycle management, porcelain metadata parsing, active checkout detection, gitdir resolution, and ancestor merge verification behind a single workspace interface.
4. **Deep Store Ingestion Module**: Relocate directory walking, macOS bulk walker dispatch, and validation cache synchronization into the store package behind an ingest tree interface.
5. **Documentation and Release Realignment**: Update the primary documentation, quickstart guides, Homebrew formulas, and install scripts to reflect automated platform defaults and modern workflow verbs.

## User Stories

1. As a developer installing `wt` on launch day, I want `wt completions zsh` to generate shell completion scripts, so that I can tab-complete commands and flags in my terminal.
2. As a developer running the install script (`install.sh`), I want my shell completions to be detected and installed automatically, so that autocompletion works without manual configuration.
3. As a developer creating a worktree with `wt new`, I want byte counts and duration numbers formatted with clean, standard unit scaling, so that receipts are immediately readable.
4. As a developer inspecting active worktrees with `wt list`, I want the output table to use the same unit conversion precision and alignment rules as other commands, so that disk savings and hydration metrics are consistent.
5. As a maintainer, I want all byte and duration formatting logic to reside in one presentation module, so that unit conversion bugs and rounding discrepancies are eliminated.
6. As a maintainer, I want git worktree porcelain parsing and path resolution encapsulated in a deep workspace module, so that command handlers do not duplicate raw git command invocations.
7. As a maintainer, I want worktree merge-base status checks centralized in the workspace module, so that cleanup commands can reliably detect merged branches across repositories.
8. As a non-CLI consumer or background worker, I want to ingest directory trees directly through the store interface, so that content ingestion does not depend on CLI modules.
9. As a developer evaluating `wt` from the README, I want the quickstart to feature modern verbs (`wt new`, `wt clean`, `wt list`, `wt demo`), so that I learn the recommended workflow first.
10. As a macOS developer reading the README, I want the documentation to state that directory snapshots and diff rebuilds are active by default on APFS, so that I do not needlessly configure redundant environment variables.
11. As an automated test harness, I want to verify shell completion generation across all supported shells, so that syntax errors in completion scripts are caught before release.
12. As a package maintainer, I want the release workflow to package shell completions alongside binaries, so that Homebrew and archive distributions include completions automatically.

## Implementation Decisions

### Decision 1: Shell Autocompletions Engine
- Add a `completions` subcommand taking a shell argument (Bash, Elvish, Fish, PowerShell, Zsh).
- Implement completion generation by passing the CLI parser definition into the completion generation machinery.
- Update the shell installer script to detect active shell directories and place completion scripts in standard locations (`~/.zsh/completion`, `/usr/local/share/zsh/site-functions`, `~/.bash_completion.d/`, or fish completion directories).

### Decision 2: Deep Presentation and Terminal Receipt Module
- Create a dedicated presentation module inside the CLI crate that exports:
  - Formatted byte representation with unified unit scaling (B, KB, MB, GB).
  - Formatted duration representation (seconds, minutes, hours, days).
  - Formatted number grouping with standard digit grouping separators.
  - Formatted table alignment and scorecard layout helpers.
- Replace duplicate formatting helpers across creation, cleanup, listing, and test-drive command handlers with calls to the presentation module.

### Decision 3: Deep Workspace Lifecycle Module
- Deepen the workspace module to encapsulate:
  - Repository root detection and gitdir resolution.
  - Worktree creation, checkout, and default destination path derivation.
  - Porcelain worktree record parsing and metadata mapping.
  - Merge-base ancestor inspection for identifying merged branches.
  - Worktree and branch removal execution.
- Command handlers for listing, cleanup, creation, and scratch isolation interact exclusively with the workspace module interface.

### Decision 4: Relocate Tree Ingestion into the Store Package
- Move directory tree scanning, bulk walker dispatch, and validation cache synchronization from the CLI hydration module into the store package.
- Provide a unified ingestion method on the store interface that accepts an ingestion path, exclusion patterns, and configuration options, returning an ingested tree record.
- Update the hydration engine to invoke the store's ingestion method, reducing hydration module complexity by several hundred lines.

### Decision 5: Documentation and Release Realignment
- Realign the primary README to highlight the modern 3-verb workflow (`wt new`, `wt clean`, `wt list`) and zero-setup test drive (`wt demo`).
- Correct the snapshot configuration table to reflect automatic APFS defaults on macOS.
- Update Homebrew formula templates and release packaging workflows to bundle shell completions.

## Testing Decisions

- **Testing High Seams**: Test CLI completions and command outputs end-to-end through CLI binary integration tests, asserting stdout contents and return codes.
- **Presentation Unit Verification**: Test byte, duration, and number formatting against comprehensive edge cases (zero values, boundary transitions, large numbers) in a dedicated presentation test suite.
- **Workspace Lifecycle Verification**: Test workspace enumeration, porcelain parsing, and merge-ancestor detection against synthetic multi-worktree fixtures.
- **Store Ingestion Verification**: Test tree ingestion and cache validation directly against the store interface.
- **Prior Art**: Follows existing integration test patterns in `crates/wt-cli/tests/cli.rs`, `crates/wt-cli/tests/list.rs`, and `crates/wt-store/tests/store.rs`.

## Out of Scope

- Graphical User Interface (GUI) or desktop menu bar apps.
- Background daemon processes or systemd / launchd services.
- Windows file system clone optimizations.

## Further Notes

- Completely preserves backward compatibility for all existing flags, subcommands (`create`, `remove`, `sweep`, `scratch`), and versioned JSON envelope formats.
- Maintains compliance with all project ADRs (ADR-0001 through ADR-0006).
