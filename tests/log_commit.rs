//! Commit/log/show integration tests: byte-identical behavior vs real git.
//!
//! Tracker 09 verification: same repo, identity, and timestamps
//! (via `GIT_*` env) → our commit sha equals real `git commit`'s; `git log
//! --oneline` output is identical; real git traverses our commits cleanly
//! (`git fsck`); `show` output equals `git show --stat`; unborn/empty/bad-
//! rev diagnostics match git's exit codes and messages.
//!
//! Reflog timestamps are not compared byte-for-byte: real git writes the
//! wall clock, we honor `GIT_COMMITTER_DATE` (decisions.md).

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn scratch_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "git-rs-log-{}-{name}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

/// A repository; real git and git-rs run against the same state.
struct Fixture {
    dir: PathBuf,
}

/// Fixed identity for the commit-parity fixtures.
fn commit_env<'a>(dates: (&'a str, &'a str)) -> Vec<(&'a str, &'a str)> {
    vec![
        ("GIT_AUTHOR_NAME", "A U Thor"),
        ("GIT_AUTHOR_EMAIL", "a@example.com"),
        ("GIT_COMMITTER_NAME", "C O Mitter"),
        ("GIT_COMMITTER_EMAIL", "c@example.com"),
        ("GIT_AUTHOR_DATE", dates.0),
        ("GIT_COMMITTER_DATE", dates.1),
    ]
}

fn write_files(dir: &PathBuf, files: &[(&str, &str)]) {
    for (name, content) in files {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }
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
        Command::new("git")
            .args(["config", "core.autocrlf", "false"])
            .current_dir(&dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        // Stable repo-level identity so bare `git-rs commit` works without
        // env vars (overridden by env in the parity fixtures).
        Command::new("git")
            .args(["config", "user.name", "A U Thor"])
            .current_dir(&dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "a@example.com"])
            .current_dir(&dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        Fixture { dir }
    }

    fn run(&self, bin: &str, args: &[&str], env: &[(&str, &str)]) -> (i32, Vec<u8>, Vec<u8>) {
        let mut cmd = Command::new(bin);
        cmd.args(args)
            .current_dir(&self.dir)
            .env("GIT_CONFIG_NOSYSTEM", "1");
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("failed to run");
        (out.status.code().unwrap_or(-1), out.stdout, out.stderr)
    }

    fn real(&self, args: &[&str]) -> (i32, Vec<u8>, Vec<u8>) {
        self.run("git", args, &[])
    }

    fn our(&self, args: &[&str]) -> (i32, Vec<u8>, Vec<u8>) {
        self.run(env!("CARGO_BIN_EXE_git-rs"), args, &[])
    }

    fn head_sha(&self) -> String {
        let (_, out, _) = self.real(&["rev-parse", "HEAD"]);
        String::from_utf8(out).unwrap().trim().to_string()
    }
}

fn last_line(out: &[u8]) -> String {
    let text = String::from_utf8_lossy(out);
    text.lines().last().unwrap_or("").to_string()
}

/// The same history created by real git and by git-rs must hash to the
/// same commits: same tree, same identity, same dates.
#[test]
fn commit_shas_match_real_git() {
    let files = [
        ("f.txt", "a\nb\nc\n"),
        ("sub/nested/deep.txt", "deep\n"),
        ("sub/leaf.txt", "x\ny\n"),
        ("z.bin", "not-binary-content\n"),
    ];
    let d1 = ("1786610000 +0530", "1786610001 +0530");
    let d2 = ("1786610020 +0530", "1786610021 +0530");
    let d3 = ("1786610040 +0530", "1786610041 +0530");

    // Repo A: everything by real git.
    let a = Fixture::new();
    write_files(&a.dir, &files);
    assert_eq!(a.real(&["add", "--all"]).0, 0);
    let (_, tree_a, _) = a.real(&["write-tree"]);
    assert_eq!(a.run("git", &["commit", "-q", "-m", "init"], &commit_env(d1)).0, 0);
    let sha_a1 = a.head_sha();

    // modify: change f, add one file, delete another
    write_files(&a.dir, &[("f.txt", "a\nb\nc\nd\ne\n"), ("new.txt", "hello\n")]);
    fs::remove_file(a.dir.join("sub/leaf.txt")).unwrap();
    assert_eq!(a.real(&["add", "--all"]).0, 0);
    assert_eq!(a.run("git", &["commit", "-q", "-m", "second"], &commit_env(d2)).0, 0);
    let sha_a2 = a.head_sha();

    // -a commit with a dirty worktree.
    write_files(&a.dir, &[("f.txt", "a\nb\nc\nd\ne\nf\n")]);
    assert_eq!(a.run("git", &["commit", "-aq", "-m", "third"], &commit_env(d3)).0, 0);
    let sha_a3 = a.head_sha();

    // Repo B: identical files, but stages and commits with git-rs.
    let b = Fixture::new();
    write_files(&b.dir, &files);
    assert_eq!(b.our(&["add", "."]).0, 0);
    let (_, tree_b, _) = b.real(&["write-tree"]);
    assert_eq!(b.our(&["commit", "-m", "init"]).0, 0);
    let sha_b1 = b.head_sha();

    write_files(&b.dir, &[("f.txt", "a\nb\nc\nd\ne\n"), ("new.txt", "hello\n")]);
    fs::remove_file(b.dir.join("sub/leaf.txt")).unwrap();
    assert_eq!(b.our(&["add", "."]).0, 0);
    assert_eq!(b.our(&["commit", "-m", "second"]).0, 0);
    let sha_b2 = b.head_sha();

    write_files(&b.dir, &[("f.txt", "a\nb\nc\nd\ne\nf\n")]);
    assert_eq!(b.our(&["commit", "-a", "-m", "third"]).0, 0);
    let sha_b3 = b.head_sha();

    assert_eq!(tree_b, tree_a, "tree built from the index differs from write-tree");
    assert_eq!(sha_b1, sha_a1, "root commit sha differs from real git");
    assert_eq!(sha_b2, sha_a2, "second commit sha differs from real git");
    assert_eq!(sha_b3, sha_a3, "-a commit sha differs from real git");

    // Real git must accept and traverse our objects.
    assert_eq!(b.real(&["fsck", "--no-dangling"]).0, 0);

    // Reflog: same identity and message; ts/tz come from GIT_COMMITTER_DATE
    // for us, the wall clock for real git (documented deviation).
    let reflog_b = std::fs::read_to_string(b.dir.join(".git/logs/HEAD")).unwrap();
    let lines: Vec<&str> = reflog_b.lines().collect();
    assert_eq!(lines.len(), 3, "reflog: {}", reflog_b);
    assert_eq!(lines[0], format!("0000000000000000000000000000000000000000 {sha_a1} C O Mitter <c@example.com> 1786610001 +0530\tcommit (initial): init"));
    assert!(lines[0].contains("\tcommit (initial): init"));
    assert!(lines[1].contains("\tcommit: second"));
    assert!(lines[2].contains("\tcommit: third"));
    assert!(lines[1].starts_with(&format!("{sha_a1} {sha_a2} ")));
    assert!(lines[2].starts_with(&format!("{sha_a2} {sha_a3} ")));

    let git_reflog = std::fs::read_to_string(a.dir.join(".git/logs/HEAD")).unwrap();
    for (ours, theirs) in lines.iter().zip(git_reflog.lines()) {
        assert!(theirs.ends_with(ours.split('\t').last().unwrap()), "reflog msg: {theirs}");
    }

    fs::remove_dir_all(&a.dir).unwrap();
    fs::remove_dir_all(&b.dir).unwrap();
}

/// A history with branches and tags (real git): our log must match
/// `git log --oneline` for the default walk, --all, -n, and --graph.
#[test]
fn log_matches_git_on_real_history() {
    let f = Fixture::new();
    let e = |d| commit_env((d, d));
    write_files(&f.dir, &[("a.txt", "1\n")]);
    assert_eq!(f.run("git", &["add", "--all"], &e("1786610100 +0530")).0, 0);
    assert_eq!(f.run("git", &["commit", "-q", "-m", "c1"], &e("1786610100 +0530")).0, 0);
    write_files(&f.dir, &[("a.txt", "1\n2\n")]);
    assert_eq!(f.run("git", &["add", "--all"], &e("1786610200 +0530")).0, 0);
    assert_eq!(f.run("git", &["commit", "-q", "-m", "c2"], &e("1786610200 +0530")).0, 0);
    f.real(&["tag", "v1"]);
    assert_eq!(f.real(&["branch", "side"]).0, 0);
    f.real(&["checkout", "-q", "side"]);
    write_files(&f.dir, &[("side.txt", "s\n")]);
    assert_eq!(f.real(&["add", "--all"]).0, 0);
    assert_eq!(f.run("git", &["commit", "-q", "-m", "c3"], &e("1786610300 +0530")).0, 0);
    f.real(&["checkout", "-q", "main"]);
    write_files(&f.dir, &[("a.txt", "1\n2\n3\n")]);
    assert_eq!(f.real(&["add", "--all"]).0, 0);
    assert_eq!(f.run("git", &["commit", "-q", "-m", "c4"], &e("1786610400 +0530")).0, 0);
    assert_eq!(f.real(&["tag", "-a", "-m", "annotated", "av1"]).0, 0);

    let cases: Vec<(&[&str], &[&str])> = vec![
        (&["log", "--oneline"], &["log", "--oneline"]),
        (&["log", "--oneline", "--all"], &["log", "--oneline", "--all"]),
        (&["log", "--oneline", "-n", "2"], &["log", "--oneline", "-n", "2"]),
        (&["log", "--oneline", "--graph"], &["log", "--oneline", "--graph"]),
        (
            &["log", "--oneline", "--graph", "--all"],
            &["log", "--oneline", "--graph", "--all"],
        ),
    ];
    for (ours, theirs) in cases {
        let (rc, out, _) = f.real(&theirs);
        let (orc, oout, oerr) = f.our(&ours);
        assert_eq!(orc, rc, "exit for {:?}", ours);
        assert_eq!(oout, out, "stdout for {:?}; ours stderr: {}", ours, String::from_utf8_lossy(&oerr));
    }
    fs::remove_dir_all(&f.dir).unwrap();
}

/// Real git must read back history written entirely by git-rs, and `show`
/// must equal `git show --stat` on it.
#[test]
fn show_matches_git_show_stat() {
    let f = Fixture::new();
    let d1 = ("1786610500 +0530", "1786610501 +0530");
    let d2 = ("1786610520 +0530", "1786610521 +0530");
    let e1 = commit_env(d1);
    let e2 = commit_env(d2);
    write_files(&f.dir, &[("f.txt", "a\nb\nc\n"), ("lonely.txt", "x\n")]);
    assert_eq!(f.our(&["add", "."]).0, 0);
    assert_eq!(f.run(env!("CARGO_BIN_EXE_git-rs"), &["commit", "-m", "add files"], &e1).0, 0);
    let root = f.head_sha();
    assert_eq!(f.our(&["commit", "-m", "again"]).0, 1, "second commit must be empty");

    write_files(&f.dir, &[("f.txt", "a\nb\n\nz\n")]);
    fs::remove_file(f.dir.join("lonely.txt")).unwrap();
    assert_eq!(f.our(&["add", "."]).0, 0);
    assert_eq!(f.run(env!("CARGO_BIN_EXE_git-rs"), &["commit", "-m", "second: modify, delete"], &e2).0, 0);
    let second = f.head_sha();
    write_files(&f.dir, &[("new.bin", "abc")]);
    assert_eq!(f.our(&["add", "."]).0, 0);
    assert_eq!(f.our(&["commit", "-m", "third"]).0, 0);
    let third = f.head_sha();

    assert_eq!(f.real(&["fsck", "--no-dangling"]).0, 0);

    for rev in [&root, &second, &third] {
        let (rc, out, _) = f.real(&["show", "--stat", rev]);
        let (orc, oout, oerr) = f.our(&["show", rev]);
        assert_eq!(orc, rc, "exit for {rev}");
        assert_eq!(oout, out, "stdout for {rev}; ours stderr: {}", String::from_utf8_lossy(&oerr));
    }
    fs::remove_dir_all(&f.dir).unwrap();
}

/// Unborn/empty-commit messages: final line and exit code match real git
/// (git prints a status block first; v1 prints only the final line).
#[test]
fn empty_and_unborn_commit_messages_match_git() {
    let f = Fixture::new();
    let d1 = ("1786610600 +0530", "1786610601 +0530");
    let env = commit_env(d1);

    // Unborn, empty index, no untracked files.
    let (rc, out, _) = f.real(&["commit", "-m", "x"]);
    let (orc, oout, oerr) = f.our(&["commit", "-m", "x"]);
    assert_eq!(orc, rc, "unborn empty: ours stderr {}", String::from_utf8_lossy(&oerr));
    assert_eq!(last_line(&oout), last_line(&out));

    // Unborn with an untracked file.
    write_files(&f.dir, &[("u.txt", "u\n")]);
    let (rc, out, _) = f.real(&["commit", "-m", "x"]);
    let (orc, oout, oerr) = f.our(&["commit", "-m", "x"]);
    assert_eq!(orc, rc, "unborn untracked: ours stderr {}", String::from_utf8_lossy(&oerr));
    assert_eq!(last_line(&oout), last_line(&out));

    // A real commit first; then nothing staged + clean worktree.
    assert_eq!(f.real(&["add", "u.txt"]).0, 0);
    assert_eq!(f.run("git", &["commit", "-q", "-m", "first"], &env).0, 0);
    let (rc, out, _) = f.real(&["commit", "-m", "x"]);
    let (orc, oout, oerr) = f.our(&["commit", "-m", "x"]);
    assert_eq!(orc, rc, "clean: ours stderr {}", String::from_utf8_lossy(&oerr));
    assert_eq!(last_line(&oout), last_line(&out));

    // Dirty worktree, nothing staged.
    write_files(&f.dir, &[("u.txt", "u2\n")]);
    let (rc, out, _) = f.real(&["commit", "-m", "x"]);
    let (orc, oout, oerr) = f.our(&["commit", "-m", "x"]);
    assert_eq!(orc, rc, "dirty: ours stderr {}", String::from_utf8_lossy(&oerr));
    assert_eq!(last_line(&oout), last_line(&out));
    fs::remove_dir_all(&f.dir).unwrap();
}

/// `commit -m ""` aborts with git's exact message, exit 1.
#[test]
fn empty_message_aborts_like_git() {
    let f = Fixture::new();
    let (rc, _, err) = f.real(&["commit", "-m", ""]);
    assert_eq!(rc, 1);
    let (orc, _, oerr) = f.our(&["commit", "-m", ""]);
    assert_eq!(orc, 1);
    assert_eq!(oerr, err);
    fs::remove_dir_all(&f.dir).unwrap();
}

/// Missing identity reproduces git's hint block byte for byte (same
/// machine => same auto-detect guess). Exit 128.
#[test]
fn missing_identity_matches_git() {
    // A bare repo with no user config at all (the Fixture sets one).
    let dir = scratch_dir("noid");
    fs::create_dir_all(&dir).unwrap();
    let init = Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(init.status.success());
    let nohome = scratch_dir("nohome");
    fs::create_dir_all(&nohome).unwrap();
    let run_bare = |bin: &str| {
        let out = Command::new(bin)
            .args(["commit", "--allow-empty", "-m", "x"])
            .current_dir(&dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &nohome)
            .env_remove("GIT_AUTHOR_NAME")
            .env_remove("GIT_AUTHOR_EMAIL")
            .env_remove("GIT_COMMITTER_NAME")
            .env_remove("GIT_COMMITTER_EMAIL")
            .env_remove("EMAIL")
            .output()
            .unwrap();
        (out.status.code().unwrap_or(-1), out.stdout, out.stderr)
    };
    let (rc, _, err) = run_bare("git");
    let (orc, _, oerr) = run_bare(env!("CARGO_BIN_EXE_git-rs"));
    assert_eq!(orc, rc, "exit must be 128");
    assert_eq!(oerr, err);
    fs::remove_dir_all(&dir).unwrap();
    fs::remove_dir_all(&nohome).unwrap();
}

/// Unborn `log`/`show` and bad revisions fail with git's exact fatal.
#[test]
fn unborn_log_and_bad_rev_match_git() {
    let f = Fixture::new();
    let (rc, _, err) = f.real(&["log", "--oneline"]);
    let (orc, _, oerr) = f.our(&["log", "--oneline"]);
    assert_eq!(orc, rc);
    assert_eq!(oerr, err);

    let (rc, _, err) = f.real(&["show"]);
    let (orc, _, oerr) = f.our(&["show"]);
    assert_eq!(orc, rc);
    assert_eq!(oerr, err);

    let (rc, _, err) = f.real(&["show", "nope"]);
    let (orc, _, oerr) = f.our(&["show", "nope"]);
    assert_eq!(orc, rc, "bad rev exit; ours stderr {}", String::from_utf8_lossy(&oerr));
    assert_eq!(oerr, err, "bad rev stderr");

    // --all on an unborn repo is silent, exit 0.
    let (rc, out, _) = f.real(&["log", "--all", "--oneline"]);
    let (orc, oout, _) = f.our(&["log", "--all", "--oneline"]);
    assert_eq!(orc, rc);
    assert_eq!(oout, out);
    fs::remove_dir_all(&f.dir).unwrap();
}

/// `commit -a` staged nothing and a second `commit` on the same index is a
/// clean-state no-op with git's message.
#[test]
fn commit_after_clean_index_is_noop() {
    let f = Fixture::new();
    write_files(&f.dir, &[("a.txt", "1\n")]);
    assert_eq!(f.real(&["add", "a.txt"]).0, 0);
    assert_eq!(f.our(&["commit", "-m", "one"]).0, 0);
    let (rc, out, _) = f.real(&["commit", "-m", "two"]);
    let (orc, oout, _) = f.our(&["commit", "-m", "two"]);
    assert_eq!(orc, rc);
    assert_eq!(last_line(&oout), last_line(&out));
    fs::remove_dir_all(&f.dir).unwrap();
}