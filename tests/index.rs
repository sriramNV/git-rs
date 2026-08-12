//! Index integration tests: our index reads/writes vs real git.
//!
//! Tracker 06 verification: after real `git add`, we read the index and see
//! the same staged entries (`git ls-files --stage` agrees); after we write an
//! index, real `git status` / `git diff --cached` agree.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use git_rs::index::Index;

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn scratch_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "git-rs-index-{}-{name}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

fn git(dir: &PathBuf, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("failed to run real git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// A real git repo; real git stages/commits, we read and rewrite the index.
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = scratch_dir("repo");
        fs::create_dir_all(&dir).unwrap();
        let init = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(init.status.success(), "git init failed");
        Fixture { dir }
    }

    fn real(&self, args: &[&str]) -> String {
        git(&self.dir, args)
    }

    fn our_bin(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_git-rs"))
            .args(args)
            .current_dir(&self.dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap()
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join(".git/index")
    }
}

fn cleanup(f: Fixture) {
    let _ = fs::remove_dir_all(&f.dir);
}

#[test]
fn read_real_git_add_index_matches_ls_files() {
    let f = Fixture::new();
    fs::write(f.dir.join("a.txt"), "alpha\n").unwrap();
    fs::write(f.dir.join("b.txt"), "beta\n").unwrap();
    fs::create_dir_all(f.dir.join("sub")).unwrap();
    fs::write(f.dir.join("sub/c.txt"), "gamma\n").unwrap();
    f.real(&["add", "a.txt", "b.txt", "sub/c.txt"]);
    let index = Index::read(&f.index_path()).unwrap();
    let ls = f.real(&["ls-files", "--stage"]);
    let expected: Vec<&str> = ls.lines().collect();
    assert_eq!(index.entries().len(), expected.len());
    for (entry, line) in index.entries().iter().zip(&expected) {
        // line: "<mode> <oid> <stage>\t<path>"
        let (meta, path) = line.split_once('\t').unwrap();
        let mut parts = meta.split(' ');
        let mode = parts.next().unwrap();
        let oid = parts.next().unwrap();
        let stage = parts.next().unwrap();
        assert_eq!(format!("{:06o}", entry.mode), mode);
        assert_eq!(hex_oid(entry.oid), oid);
        assert_eq!(entry.stage().to_string(), stage);
        assert_eq!(String::from_utf8_lossy(&entry.path), path);
    }
    cleanup(f);
}

#[test]
fn byte_round_trip_keeps_git_status_clean() {
    let f = Fixture::new();
    fs::write(f.dir.join("a.txt"), "alpha\n").unwrap();
    fs::write(f.dir.join("b.txt"), "beta\n").unwrap();
    f.real(&["add", "a.txt", "b.txt"]);
    f.real(&[
        "-c",
        "user.name=A U Thor",
        "-c",
        "user.email=a@example.com",
        "commit",
        "-q",
        "-m",
        "seed",
    ]);
    // A committed index carries git's TREE extension; our read skips it.
    let index = Index::read(&f.index_path()).unwrap();
    assert_eq!(index.entries().len(), 2);
    index.write(&f.index_path()).unwrap();
    // Stat fields preserved verbatim -> real git sees a clean tree.
    assert_eq!(f.real(&["status", "--porcelain"]), "");
    assert_eq!(f.real(&["diff", "--cached"]), "");
    cleanup(f);
}

#[test]
fn our_staged_write_real_git_agrees() {
    let f = Fixture::new();
    fs::write(f.dir.join("a.txt"), "old\n").unwrap();
    fs::write(f.dir.join("b.txt"), "beta\n").unwrap();
    f.real(&["add", "a.txt", "b.txt"]);
    f.real(&[
        "-c",
        "user.name=A U Thor",
        "-c",
        "user.email=a@example.com",
        "commit",
        "-q",
        "-m",
        "seed",
    ]);
    // Re-stage a.txt with new content via our own hash-object.
    fs::write(f.dir.join("a.txt"), "new\n").unwrap();
    let out = f.our_bin(&["hash-object", "-w", "a.txt"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let oid = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let mut index = Index::read(&f.index_path()).unwrap();
    for e in index.entries_mut() {
        if e.path == b"a.txt" {
            e.oid = hex_to_oid(&oid);
        }
    }
    index.write(&f.index_path()).unwrap();
    let porcelain = f.real(&["status", "--porcelain"]);
    assert_eq!(porcelain, "M  a.txt\n");
    let diff = f.real(&["diff", "--cached", "--", "a.txt"]);
    assert!(diff.contains("+new\n"), "diff: {diff}");
    assert!(diff.contains("-old\n"), "diff: {diff}");
    cleanup(f);
}

#[test]
fn stage_slots_round_trip() {
    let f = Fixture::new();
    fs::write(f.dir.join("f.txt"), "conflict\n").unwrap();
    f.real(&["add", "f.txt"]);
    let sha = f.real(&["rev-parse", ":f.txt"]).trim().to_string();
    // Craft stages 1,2,3 with update-index --index-info (mode prefix digit).
    let input = format!(
        "100644 blob {sha} 0\tf.txt\n110644 blob {sha} 1\tf.txt\n120644 blob {sha} 2\tf.txt\n130644 blob {sha} 3\tf.txt\n"
    );
    let out = Command::new("git")
        .args(["update-index", "--index-info"])
        .current_dir(&f.dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            use std::io::Write;
            c.stdin.as_mut().unwrap().write_all(input.as_bytes())?;
            c.wait_with_output()
        })
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let before = f.real(&["ls-files", "--stage"]);
    let index = Index::read(&f.index_path()).unwrap();
    let stages: Vec<u8> = index.entries().iter().map(|e| e.stage()).collect();
    assert_eq!(stages, vec![0, 1, 2, 3]);
    index.write(&f.index_path()).unwrap();
    let after = f.real(&["ls-files", "--stage"]);
    assert_eq!(after, before);
    cleanup(f);
}

fn hex_oid(oid: [u8; 20]) -> String {
    oid.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_to_oid(s: &str) -> [u8; 20] {
    let mut out = [0u8; 20];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(chunk).unwrap(), 16).unwrap();
    }
    out
}
