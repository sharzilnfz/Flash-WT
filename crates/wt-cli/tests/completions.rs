// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! End-to-end coverage for `wt completions <shell>` (market-launch
//! ticket 01): every supported shell must produce a nonempty,
//! shell-appropriate script on stdout.

use std::process::Command;

fn wt(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(args)
        .output()
        .expect("run wt binary")
}

fn completions_stdout(shell: &str) -> String {
    let out = wt(&["completions", shell]);
    assert!(
        out.status.success(),
        "wt completions {shell} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        !text.trim().is_empty(),
        "wt completions {shell} produced no output"
    );
    text
}

#[test]
fn bash_completions_define_the_wt_function() {
    let text = completions_stdout("bash");
    assert!(text.contains("_wt()"), "missing bash function: {text}");
    assert!(text.contains("--json"));
}

#[test]
fn zsh_completions_carry_the_compdef_header() {
    let text = completions_stdout("zsh");
    assert!(text.contains("#compdef wt"), "missing zsh header: {text}");
    assert!(text.contains("--json"));
}

#[test]
fn fish_completions_register_wt_completions() {
    let text = completions_stdout("fish");
    assert!(
        text.contains("complete") && text.contains("wt"),
        "missing fish completion calls: {text}"
    );
    // Fish spells long options as `-l json` rather than `--json`.
    assert!(text.contains("-l json"));
}

#[test]
fn powershell_completions_register_an_argument_completer() {
    let text = completions_stdout("powershell");
    assert!(
        text.contains("Register-ArgumentCompleter"),
        "missing PowerShell registration: {text}"
    );
    assert!(text.contains("--json"));
}

#[test]
fn elvish_completions_bind_completion_calls() {
    let text = completions_stdout("elvish");
    assert!(
        text.contains("edit:completion"),
        "missing elvish completion hooks: {text}"
    );
    assert!(text.contains("--json"));
}

#[test]
fn unknown_shell_is_rejected() {
    let out = wt(&["completions", "csh"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("csh"), "stderr should name the bad value");
}

#[test]
fn completions_requires_a_shell_argument() {
    let out = wt(&["completions"]);
    assert!(!out.status.success());
}

#[test]
fn help_lists_the_completions_subcommand() {
    let out = wt(&["--help"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("completions"));
}
