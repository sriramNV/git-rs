//! Smoke tests exercising the built binary end to end.

use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_git-rs"))
        .args(args)
        .output()
        .expect("failed to run git-rs")
}

#[test]
fn no_args_prints_usage_and_succeeds() {
    let out = run(&[]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("usage: git-rs <command>"));
    assert!(stdout.contains("help"));
}

#[test]
fn help_exits_zero() {
    let out = run(&["--help"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("usage: git-rs <command>"));
}

#[test]
fn help_for_unknown_command_exits_one() {
    let out = run(&["help", "nonexistent-command"]);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn version_prints_name_and_version() {
    let out = run(&["--version"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("git-rs "));
}

#[test]
fn unknown_command_exits_one() {
    let out = run(&["nonexistent-command"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown command: nonexistent-command"));
}
