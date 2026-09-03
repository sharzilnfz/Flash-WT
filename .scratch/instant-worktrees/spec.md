# Spec: Instant worktree hydration

Status: ready-for-agent

## Problem Statement

Developers on macOS lose whole minutes to filesystem operations that Linux
handles in seconds. Installing dependencies, deleting `node_modules`, and
creating git worktrees run 5 to 10 times slower on APFS than on ext4 or XFS,
even on faster hardware. The cause is metadata: every small-file create,
delete, and link costs more. Coding agents make this worse because they
create parallel workspaces constantly, and each one pays the full price again.

Existing fixes each cover one slice. pnpm deduplicates npm installs. mantle
clones build artifacts into worktrees but is macOS-only and single-purpose.
AgentFS gives agents a safe sandbox but only mounts on Linux and does not
chase speed. Nobody offers one cross-platform tool that makes agent-era
workspace creation effectively free.

## Solution

A single Rust binary, installed with brew or curl, that wraps git worktree
creation. Running `flashwt create feature-x` produces a full working copy in
seconds, with heavy untracked directories such as `node_modules`, build
outputs, and caches already present. The directories are listed in a manifest
file using gitignore syntax. Hydration copies links instead of bytes, using
APFS `clonefile` on macOS and reflink or hardlink fallbacks elsewhere.

Underneath sits a content-addressed store: the source of truth holding every
unique file content once per machine. The visible project tree is a disposable
projection kept coherent with it (ADR-0001). The store is fed by any tool, so
it stays package-manager-agnostic rather than competing with pnpm (ADR-0002).
Version 1 ships as an explicit command; an opt-in watcher daemon comes later
(ADR-0003).

## User Stories

1. As a developer on macOS, I want `flashwt create feature-x` to give me a working
   copy with dependencies in seconds, so that switching branches stops costing
   me minutes.
2. As a developer running several coding agents, I want to create many
   worktrees in parallel cheaply, so that multi-agent sessions stay practical.
3. As a developer with limited disk space, I want identical file contents
   stored once per machine, so that ten checkouts do not mean ten copies of
   the same packages.
4. As a developer who deletes old branches, I want unreferenced store content
   garbage-collected automatically, so that the store never grows without
   bound.
5. As a developer on a filesystem without clone support, I want hardlink
   fallback with protection against in-place rewrites, so that my projects
   never get corrupted through shared links.
6. As a developer using pip, cargo, or any non-JS toolchain, I want my heavy
   directories hydrate just as fast as node_modules, so that I am not locked
   into one ecosystem.
7. As a cautious user, I want the explicit command to show what it linked and
   from where, so that I can trust what touched my disk.
8. As a developer adopting the tool across a team, I want the manifest file
   checked into git, so that everyone's hydration behaves identically.
9. As a developer whose project has no manifest yet, I want sensible defaults
   plus a suggested starter manifest, so that first use works with zero setup.
10. As a benchmark-curious user, I want a public suite reproducing the
    macOS-vs-Linux numbers before and after, so that claims are verifiable.
11. As a developer restoring a botched workspace, I want to recreate any
    hydrated directory from the store exactly, so that recovery is trivial.
12. As a CI author, I want the same hydration logic available headlessly, so
    that pipelines can reuse it later without new machinery.
13. As a contributor, I want platform-specific copy strategies isolated behind
    one interface, so that adding Windows support later touches one module.
14. As a security-minded reviewer, I want store content addressed by hash, so
    that silent corruption is detectable.

## Implementation Decisions

- Single static Rust binary; no runtime dependencies; brew plus curl-script
  distribution (ADR-0003).
- CLI wraps `git worktree add`, then hydrates. Explicit command ships first;
  watcher daemon deferred (ADR-0003).
- Content-addressed store is the source of truth; on-disk trees are
  projections (ADR-0001). In v1 the store is populated by scanning existing
  checkouts' heavy directories; agents do not talk to it yet.
- Hydrate copies by whole-directory `clonefile` where available (macOS),
  reflink where available (Linux btrfs/XFS), hardlink otherwise. Copy strategy
  selection hides behind one backend trait.
- Manifest uses gitignore-syntax patterns for heavy directories,
  `.worktreeinclude`-compatible so Claude Code users recognize it.
- Hardlink mode must neutralize in-place rewrite hazards (the pnpm lessons:
  prefer clone semantics, or copy-on-shared-write) before shipping.
- Garbage collection ships in v1: reference counting plus age-based sweep of
  unreferenced store entries.
- Store integrity via content hashes; corruption detectable on read.

## Testing Decisions

- One seam: the CLI boundary. Tests run the binary against real temporary git
  repositories containing generated fake-heavy directories (thousands of
  small files), then assert externally observable results: worktree exists,
  contents byte-identical to source, store dedupes across two worktrees,
  GC removes entries after their references die, and wall-clock hydration
  stays within generous bounds sized to avoid flakes.
- Good tests assert behavior, not implementation: nothing inspects internal
  data structures; everything is checked through commands and files on disk.
- No prior art exists in this greenfield repo; these end-to-end tests become
  the founding suite. Platform-specific backends are exercised through this
  seam on each supported OS in CI.

## Out of Scope

- Watcher daemon / automatic hydration (later phase).
- Agent-facing protocol over the store; snapshot/rollback surfaces.
- Windows support (ReFS block cloning noted as future).
- Cross-machine sync of the store.
- Replacing or wrapping package managers themselves; pnpm, pip, cargo keep
  doing installs. We hydrate from whatever they produced.

## Further Notes

### Future plans

1. Watcher daemon: hydrates any newly created worktree automatically,
   opt-in after the explicit command earns trust.
2. Agent workspace layer: combine our speed with AgentFS-style safety.
   Snapshot, restore, and audit agent sessions against the store. This is the
   long-term differentiator: AgentFS has safety without speed, mantle has
   speed without agent-awareness.
3. Cross-machine sync: laptop, desktop, and CI sharing one store, sending
   only missing content. Git did this for source files; we extend it to
   everything git ignores.
4. Windows support via ReFS block cloning.
5. Editor integration: surface worktrees and store usage in VS Code / JetBrains.

### Open opportunities and known gaps

- Cache poisoning through shared hardlinks is the top correctness risk;
  solve inside v1 or do not ship hardlink mode at all.
- GC policy tuning (what counts as unreferenced, how aggressive the sweep)
  will need real-world feedback.
- A credible public benchmark suite reproducing Theo's numbers doubles as
  launch marketing.
- Security/sandboxing of agents writing into shared stores remains unsolved
  industry-wide; whoever solves speed-plus-safety owns this space.
