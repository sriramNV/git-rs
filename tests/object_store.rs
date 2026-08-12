//! Object store integration tests: our commands vs real git on the same repo.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn scratch_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "git-rs-int-{}-{name}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

fn run_git(dir: &PathBuf, args: &[&str]) -> Output {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run real git")
}

fn run_git_rs(dir: &PathBuf, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_git-rs"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run git-rs")
}

/// Create a real git repo with one file, return (dir, file path).
fn repo_with_file(content: &str) -> (PathBuf, PathBuf) {
    let dir = scratch_dir("repo");
    fs::create_dir_all(&dir).unwrap();
    let init = run_git(&dir, &["init", "-q"]);
    assert!(init.status.success(), "git init failed");
    let file = dir.join("hello.txt");
    fs::write(&file, content).unwrap();
    (dir, file)
}

#[test]
fn hash_object_matches_real_git() {
    let (dir, _file) = repo_with_file("hello world\n");
    let real = run_git(&dir, &["hash-object", "hello.txt"]);
    assert!(real.status.success());
    let real_id = String::from_utf8_lossy(&real.stdout).trim().to_string();

    let ours = run_git_rs(&dir, &["hash-object", "hello.txt"]);
    assert!(ours.status.success());
    let our_id = String::from_utf8_lossy(&ours.stdout).trim().to_string();

    assert_eq!(our_id, real_id);
    assert_eq!(our_id, "3b18e512dba79e4c8300dd08aeb37f8e728b8dad");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn hash_object_writes_object_real_git_can_read() {
    let (dir, _file) = repo_with_file("written content");
    let ours = run_git_rs(&dir, &["hash-object", "-w", "hello.txt"]);
    assert!(ours.status.success());
    let our_id = String::from_utf8_lossy(&ours.stdout).trim().to_string();

    // Real git can read the object we wrote.
    let t = run_git(&dir, &["cat-file", "-t", &our_id]);
    assert!(t.status.success());
    assert_eq!(String::from_utf8_lossy(&t.stdout).trim(), "blob");
    let s = run_git(&dir, &["cat-file", "-s", &our_id]);
    assert_eq!(String::from_utf8_lossy(&s.stdout).trim(), "15");
    let p = run_git(&dir, &["cat-file", "-p", &our_id]);
    assert_eq!(String::from_utf8_lossy(&p.stdout), "written content");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cat_file_reads_objects_real_git_wrote() {
    let (dir, _file) = repo_with_file("cross direction");
    let real = run_git(&dir, &["hash-object", "-w", "hello.txt"]);
    assert!(real.status.success());
    let real_id = String::from_utf8_lossy(&real.stdout).trim().to_string();

    let t = run_git_rs(&dir, &["cat-file", "-t", &real_id]);
    assert!(t.status.success());
    assert_eq!(String::from_utf8_lossy(&t.stdout).trim(), "blob");
    let s = run_git_rs(&dir, &["cat-file", "-s", &real_id]);
    assert_eq!(String::from_utf8_lossy(&s.stdout).trim(), "15");
    let p = run_git_rs(&dir, &["cat-file", "-p", &real_id]);
    assert_eq!(String::from_utf8_lossy(&p.stdout), "cross direction");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn fsck_stays_clean_after_our_writes() {
    let (dir, _file) = repo_with_file("fsck me");
    let ours = run_git_rs(&dir, &["hash-object", "-w", "hello.txt"]);
    assert!(ours.status.success());
    let fsck = run_git(&dir, &["fsck", "--no-dangling"]);
    assert!(
        fsck.status.success(),
        "real git fsck failed: {}",
        String::from_utf8_lossy(&fsck.stderr)
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cat_file_missing_object_fails_like_git() {
    let (dir, _file) = repo_with_file("missing");
    let ours = run_git_rs(&dir, &["cat-file", "-t", "deadbeef".repeat(5).as_str()]);
    assert_eq!(ours.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&ours.stderr);
    assert!(
        stderr.contains("Not a valid object name"),
        "stderr: {stderr}"
    );
    // Real git 2.55 exits 128 too, but with "could not get object info"
    // (see decisions.md D-003 — message parity is a step 17 concern).
    let real = run_git(&dir, &["cat-file", "-t", "deadbeef".repeat(5).as_str()]);
    assert_eq!(real.status.code(), Some(128));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn stdin_hashing_matches_git() {
    let dir = scratch_dir("stdin");
    fs::create_dir_all(&dir).unwrap();
    let init = run_git(&dir, &["init", "-q"]);
    assert!(init.status.success());

    let ours = Command::new(env!("CARGO_BIN_EXE_git-rs"))
        .args(["hash-object", "--stdin"])
        .current_dir(&dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"stdin content")
                .unwrap();
            child.wait_with_output()
        })
        .unwrap();
    assert!(ours.status.success());

    let real = Command::new("git")
        .args(["hash-object", "--stdin"])
        .current_dir(&dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"stdin content")
                .unwrap();
            child.wait_with_output()
        })
        .unwrap();
    assert!(real.status.success());

    assert_eq!(
        String::from_utf8_lossy(&ours.stdout).trim(),
        String::from_utf8_lossy(&real.stdout).trim()
    );
    let _ = fs::remove_dir_all(&dir);
}
