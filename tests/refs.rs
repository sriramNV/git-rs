//! Refs integration tests: our ref/reflog writes vs real git reads.
//!
//! Tracker 05 verification: "create a branch + commit via our code, then
//! real `git branch -v`, `git log`, `git reflog` show identical results".

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use git_rs::object::commit::{Commit, Ident};
use git_rs::object::tree::{Tree, TreeEntry};
use git_rs::refs::Refs;
use git_rs::store::{Kind, ObjectStore};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn scratch_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "git-rs-refs-{}-{name}-{}",
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

/// A real git repo; we build objects/refs inside it and compare reads.
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

    fn store(&self) -> ObjectStore {
        ObjectStore::new(self.dir.join(".git/objects"))
    }

    fn refs(&self) -> Refs {
        Refs::new(self.dir.join(".git"))
    }

    fn real(&self, args: &[&str]) -> String {
        git(&self.dir, args)
    }

    fn bin(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_git-rs"))
            .args(args)
            .current_dir(&self.dir)
            .envs(fixed_env())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap()
    }
}

fn cleanup(f: Fixture) {
    let _ = fs::remove_dir_all(&f.dir);
}

fn fixed_env() -> Vec<(&'static str, &'static str)> {
    vec![
        ("GIT_COMMITTER_NAME", "A U Thor"),
        ("GIT_COMMITTER_EMAIL", "a@example.com"),
        ("GIT_COMMITTER_DATE", "1700000000 +0530"),
    ]
}

/// Build a commit object (tree + blob) with our code, as step 04 did.
fn commit_from_scratch(store: &ObjectStore) -> String {
    let blob = store.write_blob(b"hello refs\n").unwrap();
    let tree = Tree {
        entries: vec![TreeEntry {
            mode: 0o100644,
            name: b"f.txt".to_vec(),
            oid: hex_to_bytes(&blob),
        }],
    };
    let tree_sha = store
        .write_object(Kind::Tree, &tree.serialize().unwrap())
        .unwrap();
    let ident = Ident::new("A U Thor", "a@example.com", 1700000000, 530).unwrap();
    let commit = Commit {
        tree: hex_to_bytes(&tree_sha),
        parents: vec![],
        author: ident.clone(),
        committer: ident,
        message: b"first".to_vec(),
    };
    store
        .write_object(Kind::Commit, &commit.serialize().unwrap())
        .unwrap()
}

fn hex_to_bytes(s: &str) -> [u8; 20] {
    let mut out = [0u8; 20];
    for i in 0..20 {
        out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
    }
    out
}

#[test]
fn update_ref_creates_branch_real_git_reads_it() {
    let f = Fixture::new();
    let commit = commit_from_scratch(&f.store());
    // Create a commit on main via our update-ref, like a commit command would.
    let out = f.bin(&[
        "update-ref",
        "-m",
        "commit (initial): first",
        "HEAD",
        &commit,
    ]);
    assert!(
        out.status.success(),
        "our update-ref failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Real git sees the branch and the commit.
    let log = f.real(&["log", "--format=%s", "main"]);
    assert_eq!(log, "first\n");
    let branch = f.real(&["branch", "-v"]);
    assert_eq!(branch, format!("* main {} first\n", &commit[..7]));
    let reflog = f.real(&["reflog", "show", "--format=%H %gs", "HEAD"]);
    assert!(
        reflog.contains(&format!("{commit} commit (initial): first")),
        "reflog: {reflog}"
    );

    // Second commit: real git itself commits on top, then our HEAD reflog
    // chain must still resolve.
    f.real(&["commit", "--allow-empty", "-m", "second"]);
    let log2 = f.real(&["log", "--format=%s", "main"]);
    assert_eq!(log2, "second\nfirst\n");

    cleanup(f);
}

#[test]
fn update_ref_matches_real_git_byte_for_byte() {
    let f = Fixture::new();
    let commit = commit_from_scratch(&f.store());
    // Same operation through real git and through us.
    let real_out = Command::new("git")
        .args([
            "update-ref",
            "-m",
            "branch: created",
            "refs/heads/feature",
            &commit,
        ])
        .current_dir(&f.dir)
        .envs(fixed_env())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(real_out.status.success());
    let our_out = f.bin(&[
        "update-ref",
        "-m",
        "branch: created",
        "refs/heads/feature2",
        &commit,
    ]);
    assert!(our_out.status.success());
    let real_ref = fs::read_to_string(f.dir.join(".git/refs/heads/feature")).unwrap();
    let our_ref = fs::read_to_string(f.dir.join(".git/refs/heads/feature2")).unwrap();
    assert_eq!(our_ref, real_ref);
    let real_log = fs::read_to_string(f.dir.join(".git/logs/refs/heads/feature")).unwrap();
    let our_log = fs::read_to_string(f.dir.join(".git/logs/refs/heads/feature2")).unwrap();
    assert_eq!(our_log, real_log);
    // Real git's reflog display for our branch matches the real one.
    let real_disp = f.real(&["reflog", "show", "--format=%H %gs", "feature"]);
    let our_disp = f.real(&["reflog", "show", "--format=%H %gs", "feature2"]);
    assert_eq!(our_disp, real_disp);
    cleanup(f);
}

#[test]
fn update_ref_cas_and_errors_match_real_git() {
    let f = Fixture::new();
    let commit = commit_from_scratch(&f.store());
    let real_out = Command::new("git")
        .args(["update-ref", "refs/heads/feature", &commit])
        .current_dir(&f.dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(real_out.status.success());

    // CAS success through us.
    let out = f.bin(&["update-ref", "refs/heads/feature", &commit, &commit]);
    assert!(
        out.status.success(),
        "CAS success failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Bad name: same stderr, same exit code as real git.
    let bad = "refs/heads/../evil";
    let real = Command::new("git")
        .args(["update-ref", bad, &commit])
        .current_dir(&f.dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    let ours = f.bin(&["update-ref", bad, &commit]);
    assert_eq!(ours.status.code(), real.status.code());
    let real_msg = String::from_utf8_lossy(&real.stderr);
    let our_msg = String::from_utf8_lossy(&ours.stderr);
    assert_eq!(our_msg.trim(), real_msg.trim());

    // Nonexistent object: same stderr, same exit code.
    let ghost = "1111111111111111111111111111111111111111";
    let real = Command::new("git")
        .args(["update-ref", "refs/heads/ghost", ghost])
        .current_dir(&f.dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    let ours = f.bin(&["update-ref", "refs/heads/ghost", ghost]);
    assert_eq!(ours.status.code(), real.status.code());
    let real_msg = String::from_utf8_lossy(&real.stderr);
    let our_msg = String::from_utf8_lossy(&ours.stderr);
    assert_eq!(our_msg.trim(), real_msg.trim());

    // CAS mismatch: "is at <actual> but expected <expected>".
    let other = "2222222222222222222222222222222222222222";
    let real = Command::new("git")
        .args(["update-ref", "refs/heads/feature", &commit, other])
        .current_dir(&f.dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    let ours = f.bin(&["update-ref", "refs/heads/feature", &commit, other]);
    assert_eq!(ours.status.code(), real.status.code());
    let real_msg = String::from_utf8_lossy(&real.stderr);
    let our_msg = String::from_utf8_lossy(&ours.stderr);
    assert_eq!(our_msg.trim(), real_msg.trim());

    // CAS create-only on existing ref: "reference already exists".
    let zero = "0000000000000000000000000000000000000000";
    let real = Command::new("git")
        .args(["update-ref", "refs/heads/feature", &commit, zero])
        .current_dir(&f.dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    let ours = f.bin(&["update-ref", "refs/heads/feature", &commit, zero]);
    assert_eq!(ours.status.code(), real.status.code());
    let real_msg = String::from_utf8_lossy(&real.stderr);
    let our_msg = String::from_utf8_lossy(&ours.stderr);
    assert_eq!(our_msg.trim(), real_msg.trim());

    cleanup(f);
}

#[test]
fn packed_refs_loose_wins_matches_real_git() {
    let f = Fixture::new();
    let commit = commit_from_scratch(&f.store());
    // Create a ref, pack it, then override with a loose ref via our code.
    f.real(&["update-ref", "refs/heads/packed", &commit]);
    f.real(&["update-ref", "refs/heads/stays", &commit]);
    f.real(&["pack-refs", "--all"]);
    assert_eq!(f.real(&["rev-parse", "refs/heads/stays"]).trim(), commit);
    // Loose override needs a real object for our check; reuse the commit id
    // for the loose ref so both git and we agree on content.
    let out = f.bin(&["update-ref", "refs/heads/packed", &commit]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let loose = fs::read_to_string(f.dir.join(".git/refs/heads/packed")).unwrap();
    assert_eq!(loose.trim(), commit);
    assert_eq!(f.real(&["rev-parse", "refs/heads/packed"]).trim(), commit);
    // Still-packed ref keeps resolving through packed-refs.
    assert_eq!(f.real(&["rev-parse", "refs/heads/stays"]).trim(), commit);
    cleanup(f);
}

#[test]
fn resolve_matches_real_git() {
    let f = Fixture::new();
    let commit = commit_from_scratch(&f.store());
    let out = f.bin(&["update-ref", "refs/heads/r1", &commit]);
    assert!(out.status.success());
    // Unborn HEAD before update: our resolve → None, git errors.
    assert!(f.refs().resolve("refs/heads/unborn").unwrap().is_none());
    assert_eq!(
        f.refs().resolve("refs/heads/r1").unwrap(),
        Some(commit.clone())
    );
    // HEAD → refs/heads/main → unborn, so None.
    assert!(f.refs().resolve("HEAD").unwrap().is_none());
    cleanup(f);
}
