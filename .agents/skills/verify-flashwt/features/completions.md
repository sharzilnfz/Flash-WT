# Shell completions generation

`flashwt completions` generates native tab-completion scripts for major shells (Bash, Zsh, Fish, Elvish, and PowerShell), enabling instant command, subcommand, flag, and option completion in the user's terminal.

## Sub-features

- `completions-bash` outputs Bourne-Again Shell completion definitions.
- `completions-zsh` outputs Z shell completion functions.
- `completions-fish` outputs Fish shell completion directives.
- `completions-elvish` outputs Elvish completion code.
- `completions-powershell` outputs PowerShell argument completer scripts.

## How to get to it (user POV)

- Run `flashwt completions bash` to generate Bash completion code.
- Run `flashwt completions zsh` to generate Zsh completion code.
- Run `flashwt completions fish` to generate Fish completion code.
- Run `flashwt completions elvish` or `flashwt completions powershell` for respective shells.

## Driving it with the shell fixture

Preconditions:

- `$FLASHWT_BIN` executable and available.

- **Generate bash completions.** `"$FLASHWT_BIN" completions bash`. Output contains `_flashwt()` and lists top-level subcommands `new`, `clean`, `hydrate`, `list`, `sweep`, `doctor`.
- **Generate zsh completions.** `"$FLASHWT_BIN" completions zsh`. Output contains `#compdef flashwt` and completion function definitions for subcommands.
- **Generate fish completions.** `"$FLASHWT_BIN" completions fish`. Output contains directives starting with `complete -c flashwt`.
- **Generate powershell completions.** `"$FLASHWT_BIN" completions powershell`. Output contains `Register-ArgumentCompleter -Native -CommandName 'flashwt'`.
- **Generate elvish completions.** `"$FLASHWT_BIN" completions elvish`. Output contains `edit:completion`.
- **Verify subcommands represented.** Grep generated completions for `hydrate`, `clean`, `scratch`, `demo`, `store`, `list`, `init`, `doctor`, `lease`.
- **Proof.** Save the head of each generated completion script to `artifacts/verify-flashwt/<run-id>/`.

## Gotchas

- `flashwt completions` outputs raw shell script text to stdout, not a JSON envelope (even if `--json` is supplied).
- Passing an unsupported shell name (e.g. `flashwt completions csh`) exits with clap error code 2.
- Subcommand `migrate` lives under `store` (`flashwt store migrate`), not at the top level.
