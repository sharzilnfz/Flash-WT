# 03: Ephemeral scratch and isolate worktrees with lease persistence

**What to build:** Add `flashwt scratch` and `flashwt isolate` commands for autonomous agents running throwaway test and evaluation workloads. Generate uniquely named temporary worktrees without requiring manual branch management. When run with `--run "<command>"`, execute the child command, forward standard input/output/exit codes, and clean up the sandbox immediately via an in-process RAII guard. Persist a lease file in `<store>/worktrees/scratch-<id>.lease` with worktree path, git directory, process ID, process start time, and expiration time to enable robust background garbage collection.

**Blocked by:** 01: Versioned JSON output envelope across CLI commands.

**Status:** ready-for-agent

- [x] `flashwt scratch` and `flashwt isolate` subcommands are registered in the CLI command dispatcher.
- [x] Bare `flashwt scratch` generates uniquely named temporary worktrees marked with ephemeral lease tags.
- [x] `flashwt scratch --run "<command>"` creates a temporary sandbox, executes the command, returns the command exit code, and removes the worktree on clean process exit.
- [x] A lease record is written to `<store>/worktrees/scratch-<id>.lease` containing worktree path, git directory, process ID, process start time fingerprint, and expiration timestamp.
- [x] `--json` mode returns structured worktree creation and command execution status.
- [x] CLI tests verify ephemeral creation, command execution, and exit cleanup.
