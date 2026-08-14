//! Checkout/branch/tag/reset integration tests: byte parity vs real git and
//! cross-tool interop on the same repo (tracker 10 verification).
//!
//! Reset parity runs real git and git-rs on two identically-seeded repos
//! (fixed dates/identity => identical shas), then compares bytes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn scratch_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "git-rs-cbr-{}-{name}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

struct Fixture {
    dir: PathBuf,
}

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

fn write_file(dir: &Path, name: &str, content: &str) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
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
        for kv in [
            ("core.autocrlf", "false"),
            ("user.name", "A U Thor"),
            ("user.email", "a@example.com"),
        ] {
            Command::new("git")
                .args(["config", kv.0, kv.1])
                .current_dir(&dir)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
        }
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

    fn commit(&self, msg: &str, date: &str) -> String {
        assert_eq!(
            self.run(
                "git",
                &["commit", "-q", "-m", msg],
                &commit_env((date, date))
            )
            .0,
            0
        );
        self.head_sha()
    }
}

/// c1 (a.txt) -> c2 (a.txt modified, b.txt added); tags v1.0, v0.5, v1.9,
/// v1.10 at c2; branches feature (c2), side (c2).
fn seed(f: &Fixture) {
    write_file(&f.dir, "a.txt", "1\n");
    assert_eq!(f.real(&["add", "--all"]).0, 0);
    f.commit("c1", "1786610100 +0530");
    write_file(&f.dir, "a.txt", "1\n2\n");
    write_file(&f.dir, "b.txt", "b\n");
    assert_eq!(f.real(&["add", "--all"]).0, 0);
    f.commit("c2", "1786610200 +0530");
    for t in ["v1.0", "v0.5", "v1.9", "v1.10"] {
        assert_eq!(f.real(&["tag", t]).0, 0);
    }
    assert_eq!(f.real(&["branch", "feature"]).0, 0);
    assert_eq!(f.real(&["branch", "side"]).0, 0);
}

fn last_stderr_lines(out: &[u8], n: usize) -> String {
    let text = String::from_utf8_lossy(out);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// branch create/list/delete: silence, bytes, exit codes match git.
#[test]
fn branch_lifecycle_matches_git() {
    let f = Fixture::new();
    seed(&f);

    // Create is silent.
    let (rc, out, err) = f.our(&["branch", "mine"]);
    assert_eq!((rc, out, err), (0, b"".to_vec(), b"".to_vec()));

    // List byte-equal (piped output is unpadded).
    let (rc, out, _) = f.real(&["branch", "-a"]);
    let (orc, oout, oerr) = f.our(&["branch", "-a"]);
    assert_eq!(orc, rc);
    assert_eq!(
        oout,
        out,
        "branch -a; ours stderr: {}",
        String::from_utf8_lossy(&oerr)
    );

    // Detached list: pseudo row first, byte-equal.
    assert_eq!(f.real(&["checkout", "-q", "HEAD~1"]).0, 0);
    let (rc, out, _) = f.real(&["branch", "-a"]);
    let (orc, oout, oerr) = f.our(&["branch", "-a"]);
    assert_eq!(orc, rc);
    assert_eq!(
        oout,
        out,
        "detached branch -a; ours stderr: {}",
        String::from_utf8_lossy(&oerr)
    );
    assert_eq!(f.real(&["checkout", "-q", "main"]).0, 0);

    // Merged delete: `Deleted branch mine2 (was <short>).` byte-equal. Real
    // runs first, so re-create mine2 before ours deletes it.
    assert_eq!(f.real(&["branch", "mine2"]).0, 0);
    let (rc, out, _) = f.real(&["branch", "-d", "mine2"]);
    assert_eq!(f.real(&["branch", "mine2"]).0, 0);
    let (orc, oout, oerr) = f.our(&["branch", "-d", "mine2"]);
    assert_eq!(orc, rc);
    assert_eq!(
        oout,
        out,
        "merged delete; ours stderr: {}",
        String::from_utf8_lossy(&oerr)
    );

    // Unmerged delete: git's error + 2 hints on stderr, exit 1.
    assert_eq!(f.real(&["checkout", "-q", "-b", "unmerged"]).0, 0);
    write_file(&f.dir, "u.txt", "u\n");
    assert_eq!(f.real(&["add", "u.txt"]).0, 0);
    f.commit("c3", "1786610300 +0530");
    let c3 = f.head_sha();
    assert_eq!(f.real(&["checkout", "-q", "main"]).0, 0);
    let (rc, _, err) = f.real(&["branch", "-d", "unmerged"]);
    let (orc, _, oerr) = f.our(&["branch", "-d", "unmerged"]);
    assert_eq!(orc, rc, "unmerged delete exit");
    assert_eq!(oerr, err, "unmerged delete stderr (error + hints)");

    // Force delete prints the same success line. Real deletes first, so
    // recreate at the same commit before ours runs.
    let (rc, out, _) = f.real(&["branch", "-D", "unmerged"]);
    assert_eq!(f.real(&["branch", "unmerged", &c3]).0, 0);
    let (orc, oout, _) = f.our(&["branch", "-D", "unmerged"]);
    assert_eq!((orc, oout), (rc, out));

    // Current branch: `error: cannot delete branch ... used by worktree
    // at '<path>'`, exit 1.
    let (rc, _, err) = f.real(&["branch", "-d", "main"]);
    let (orc, _, oerr) = f.our(&["branch", "-d", "main"]);
    assert_eq!(orc, rc, "delete current exit");
    assert_eq!(oerr, err, "delete current stderr");

    // Missing branch: `error: branch 'nope' not found`, exit 1.
    let (rc, _, err) = f.real(&["branch", "-d", "nope"]);
    let (orc, _, oerr) = f.our(&["branch", "-d", "nope"]);
    assert_eq!(orc, rc, "delete missing exit");
    assert_eq!(oerr, err, "delete missing stderr");

    // Duplicate create: `fatal: a branch named 'feature' already exists`.
    let (rc, _, err) = f.real(&["branch", "feature"]);
    let (orc, _, oerr) = f.our(&["branch", "feature"]);
    assert_eq!(orc, rc, "duplicate create exit");
    assert_eq!(oerr, err, "duplicate create stderr");

    fs::remove_dir_all(&f.dir).unwrap();
}

/// checkout messages, Already-on, unknown target, dirty gate, -f, detached.
/// State-changing commands are compared by running real git, restoring the
/// baseline via `checkout -q main`, then running ours on the same state.
#[test]
fn checkout_messages_match_git() {
    let f = Fixture::new();
    seed(&f);

    // Switch to existing branch.
    let (rc, _, err) = f.real(&["checkout", "side"]);
    assert_eq!(f.real(&["checkout", "-q", "main"]).0, 0);
    let (orc, _, oerr) = f.our(&["checkout", "side"]);
    assert_eq!(orc, rc);
    assert_eq!(oerr, err);
    assert_eq!(f.real(&["checkout", "-q", "main"]).0, 0);

    // Already on the current branch (clean).
    let (rc, _, err) = f.real(&["checkout", "main"]);
    let (orc, _, oerr) = f.our(&["checkout", "main"]);
    assert_eq!(orc, rc);
    assert_eq!(oerr, err);

    // Create and switch: identical message modulo the branch name.
    let (rc, _, err) = f.real(&["checkout", "-b", "newb"]);
    assert_eq!(f.real(&["checkout", "-q", "main"]).0, 0);
    let (orc, _, oerr) = f.our(&["checkout", "-b", "newb2"]);
    assert_eq!(orc, rc);
    assert_eq!(
        String::from_utf8_lossy(&oerr),
        String::from_utf8_lossy(&err).replace("newb", "newb2")
    );
    assert_eq!(f.real(&["checkout", "-q", "main"]).0, 0);

    // Unknown target, exit 1.
    let (rc, _, err) = f.real(&["checkout", "nope"]);
    let (orc, _, oerr) = f.our(&["checkout", "nope"]);
    assert_eq!(orc, rc, "unknown target exit");
    assert_eq!(oerr, err, "unknown target stderr");

    // Dirty gate: staged change + different target tree refuses, byte-equal.
    // (Switching to a same-tree branch is allowed by git even when dirty, so
    // the target must differ: HEAD~1's tree has only a.txt at "1\n".)
    write_file(&f.dir, "a.txt", "1\n2\n3\n");
    assert_eq!(f.real(&["add", "a.txt"]).0, 0);
    let (rc, _, err) = f.real(&["checkout", "HEAD~1"]);
    let (orc, _, oerr) = f.our(&["checkout", "HEAD~1"]);
    assert_eq!(orc, rc, "dirty gate exit");
    assert_eq!(oerr, err, "dirty gate stderr");

    // -f discards the staged change, switches, and materializes the tree.
    let (orc, _, oerr) = f.our(&["checkout", "-f", "side"]);
    assert_eq!(orc, 0, "ours stderr: {}", String::from_utf8_lossy(&oerr));
    assert_eq!(fs::read_to_string(f.dir.join("a.txt")).unwrap(), "1\n2\n");
    assert_eq!(
        String::from_utf8_lossy(&f.real(&["status", "--porcelain"]).1),
        "",
        "worktree+index must be clean after -f"
    );

    // Detached: git prints a full advice block (v1 deviation); the last
    // stderr line (`HEAD is now at ...`) must match byte for byte.
    assert_eq!(f.real(&["checkout", "-q", "main"]).0, 0);
    let (rc, _, err) = f.real(&["checkout", "HEAD~1"]);
    assert_eq!(f.real(&["checkout", "-q", "main"]).0, 0);
    let (orc, _, oerr) = f.our(&["checkout", "HEAD~1"]);
    assert_eq!(orc, rc);
    assert_eq!(last_stderr_lines(&oerr, 1), last_stderr_lines(&err, 1));

    fs::remove_dir_all(&f.dir).unwrap();
}

/// Real git must accept what our checkout/branch/tag wrote, and vice versa.
#[test]
fn checkout_interop_with_real_git() {
    let f = Fixture::new();
    seed(&f);

    // Our checkout of a branch: real git sees a clean status.
    assert_eq!(f.our(&["checkout", "side"]).0, 0);
    assert_eq!(f.real(&["status", "--porcelain"]).0, 0);
    assert_eq!(
        String::from_utf8_lossy(&f.real(&["status", "--porcelain"]).1),
        ""
    );

    // Untracked files survive our branch switches.
    write_file(&f.dir, "scratch.txt", "untracked\n");
    assert_eq!(f.our(&["checkout", "main"]).0, 0);
    assert!(f.dir.join("scratch.txt").exists());
    assert_eq!(
        String::from_utf8_lossy(&f.real(&["status", "--porcelain"]).1),
        "?? scratch.txt\n"
    );

    // Real git can switch between branches we created and edited.
    assert_eq!(f.real(&["checkout", "-q", "side"]).0, 0);
    write_file(&f.dir, "sidefile.txt", "s\n");
    assert_eq!(f.real(&["add", "sidefile.txt"]).0, 0);
    f.commit("c3 side", "1786610400 +0530");
    assert_eq!(f.our(&["checkout", "main"]).0, 0);
    assert!(!f.dir.join("sidefile.txt").exists());
    assert_eq!(f.our(&["checkout", "side"]).0, 0);
    assert_eq!(
        f.real(&["log", "--oneline"]).1,
        f.real(&["log", "--oneline", "side"]).1
    );
    assert_eq!(f.real(&["fsck", "--no-dangling"]).0, 0);

    // Tags: real git reads our annotated tag object; ours reads git's.
    assert_eq!(f.our(&["tag", "-a", "-m", "our annotated", "ours"]).0, 0);
    assert_eq!(f.real(&["fsck", "--no-dangling"]).0, 0);
    let (rc, _, _) = f.real(&["cat-file", "tag", "ours"]);
    assert_eq!(rc, 0, "real git must read our annotated tag");

    assert_eq!(
        f.real(&["tag", "-a", "-m", "their annotated", "theirs"]).0,
        0
    );
    let (rc, out, _) = f.real(&["cat-file", "-p", "theirs"]);
    assert_eq!(rc, 0);
    let (orc, oout, oerr) = f.our(&["cat-file", "-p", "theirs"]);
    assert_eq!(orc, 0);
    assert_eq!(
        oout,
        out,
        "our cat-file of git's tag; ours stderr: {}",
        String::from_utf8_lossy(&oerr)
    );

    // Checkout by tag peels to the commit (detached).
    let target = f.real(&["rev-parse", "v1.0"]).1;
    assert_eq!(f.our(&["checkout", "v1.0"]).0, 0);
    assert_eq!(
        f.real(&["rev-parse", "HEAD"]).1,
        target,
        "checkout by tag must land on the tagged commit"
    );
    fs::remove_dir_all(&f.dir).unwrap();
}

/// tag create/delete/list: bytes and exit codes match git.
#[test]
fn tag_lifecycle_matches_git() {
    let f = Fixture::new();
    seed(&f);

    // Create silent; duplicate fatal.
    let (rc, out, err) = f.our(&["tag", "mine"]);
    assert_eq!((rc, out, err), (0, b"".to_vec(), b"".to_vec()));
    let (rc, _, err) = f.real(&["tag", "mine"]);
    let (orc, _, oerr) = f.our(&["tag", "mine"]);
    assert_eq!(orc, rc, "duplicate tag exit");
    assert_eq!(oerr, err, "duplicate tag stderr");

    // List byte-equal (lexicographic: foo, v0.5, v1.0, v1.10, v1.9).
    let (rc, out, _) = f.real(&["tag", "-l"]);
    let (orc, oout, oerr) = f.our(&["tag", "-l"]);
    assert_eq!(orc, rc);
    assert_eq!(
        oout,
        out,
        "tag -l; ours stderr: {}",
        String::from_utf8_lossy(&oerr)
    );

    // Delete: success line and missing error match git. Real runs first on
    // the shared repo, so re-create v0.5 before ours runs.
    let (rc, out, _) = f.real(&["tag", "-d", "v0.5"]);
    assert_eq!(f.real(&["tag", "v0.5"]).0, 0);
    let (orc, oout, oerr) = f.our(&["tag", "-d", "v0.5"]);
    assert_eq!(orc, rc);
    assert_eq!(
        oout,
        out,
        "tag delete; ours stderr: {}",
        String::from_utf8_lossy(&oerr)
    );
    let (rc, _, err) = f.real(&["tag", "-d", "v0.5"]);
    let (orc, _, oerr) = f.our(&["tag", "-d", "v0.5"]);
    assert_eq!(orc, rc, "tag delete missing exit");
    assert_eq!(oerr, err, "tag delete missing stderr");

    // Annotated: real git cat-file output equals ours byte-for-byte. Both
    // get the same committer date env so tagger lines match; the tag name
    // line differs (v2.0 vs v2.1), so normalize it before comparing.
    let env = commit_env(("1786691742 +0530", "1786691742 +0530"));
    let (rc, _, _) = f.run("git", &["tag", "-a", "-m", "release two", "v2.0"], &env);
    assert_eq!(rc, 0);
    assert_eq!(
        f.run(
            env!("CARGO_BIN_EXE_git-rs"),
            &["tag", "-a", "-m", "release two", "v2.1"],
            &env
        )
        .0,
        0
    );
    let tag_obj_a = f.real(&["rev-parse", "v2.0"]).1;
    let tag_obj_b = f.real(&["rev-parse", "v2.1"]).1;
    let (_, cat_a, _) = f.real(&[
        "cat-file",
        "tag",
        String::from_utf8_lossy(&tag_obj_a).trim(),
    ]);
    let (_, cat_b, _) = f.real(&[
        "cat-file",
        "tag",
        String::from_utf8_lossy(&tag_obj_b).trim(),
    ]);
    let cat_b = String::from_utf8_lossy(&cat_b).replace("v2.1", "v2.0");
    assert_eq!(
        cat_a.as_slice(),
        cat_b.as_bytes(),
        "our annotated tag object must be byte-identical to git's"
    );

    fs::remove_dir_all(&f.dir).unwrap();
}

/// reset --soft/--mixed/--hard: byte parity on identically-seeded repos.
#[test]
fn reset_matches_git() {
    let make = |seed_dir: bool| {
        let f = Fixture::new();
        if seed_dir {
            seed(&f);
            // Move main ahead of the side branch so resets have history.
            write_file(&f.dir, "a.txt", "1\n2\n3\n");
            assert_eq!(f.real(&["add", "--all"]).0, 0);
            f.commit("c3", "1786610500 +0530");
        }
        f
    };
    let (g, o) = (make(true), make(true));
    assert_eq!(g.head_sha(), o.head_sha(), "seeds must produce same shas");

    // --hard to a parent: stdout `HEAD is now at <short> <subject>`.
    let (rc, out, _) = g.real(&["reset", "--hard", "HEAD~1"]);
    let (orc, oout, oerr) = o.our(&["reset", "--hard", "HEAD~1"]);
    assert_eq!(orc, rc, "reset hard exit");
    assert_eq!(
        oout,
        out,
        "reset --hard stdout; ours stderr: {}",
        String::from_utf8_lossy(&oerr)
    );
    // Same commit: real git re-prints `HEAD is now at`.
    let (rc, out, _) = g.real(&["reset", "--hard", "HEAD"]);
    let (orc, oout, _) = o.our(&["reset", "--hard", "HEAD"]);
    assert_eq!(orc, rc);
    assert_eq!(oout, out);

    // A dirty worktree, then --soft: silent, index keeps the staged change.
    write_file(&o.dir, "a.txt", "dirty\n");
    assert_eq!(o.real(&["add", "a.txt"]).0, 0);
    write_file(&g.dir, "a.txt", "dirty\n");
    assert_eq!(g.real(&["add", "a.txt"]).0, 0);
    let (rc, out, _) = g.real(&["reset", "--soft", "HEAD"]);
    let (orc, oout, _) = o.our(&["reset", "--soft", "HEAD"]);
    assert_eq!((orc, oout), (rc, out));
    assert_eq!(
        g.real(&["status", "--porcelain"]).1,
        o.real(&["status", "--porcelain"]).1
    );

    // --mixed: `Unstaged changes after reset:` block byte-equal; index
    // matches the target tree on both.
    let (rc, out, _) = g.real(&["reset", "--mixed", "HEAD"]);
    let (orc, oout, oerr) = o.our(&["reset", "--mixed", "HEAD"]);
    assert_eq!(orc, rc, "reset mixed exit");
    assert_eq!(
        oout,
        out,
        "reset --mixed stdout; ours stderr: {}",
        String::from_utf8_lossy(&oerr)
    );
    assert_eq!(
        g.real(&["diff", "--cached"]).1,
        o.real(&["diff", "--cached"]).1
    );

    // --hard discards the worktree change; repos agree on state.
    let (rc, _, _) = g.real(&["reset", "--hard", "HEAD"]);
    let (orc, _, _) = o.our(&["reset", "--hard", "HEAD"]);
    assert_eq!(orc, rc);
    assert_eq!(
        g.real(&["status", "--porcelain"]).1,
        o.real(&["status", "--porcelain"]).1
    );
    assert_eq!(
        g.real(&["log", "--oneline"]).1,
        o.real(&["log", "--oneline"]).1
    );

    // Bare `reset` (mixed to HEAD): byte-equal.
    write_file(&o.dir, "a.txt", "d2\n");
    write_file(&g.dir, "a.txt", "d2\n");
    let (rc, out, _) = g.real(&["reset"]);
    let (orc, oout, oerr) = o.our(&["reset"]);
    assert_eq!(orc, rc);
    assert_eq!(
        oout,
        out,
        "bare reset; ours stderr: {}",
        String::from_utf8_lossy(&oerr)
    );

    // Unknown target: `fatal: ambiguous argument ...`, exit 128.
    let (rc, _, err) = g.real(&["reset", "--hard", "nope"]);
    let (orc, _, oerr) = o.our(&["reset", "--hard", "nope"]);
    assert_eq!(orc, rc, "reset unknown exit");
    assert_eq!(oerr, err, "reset unknown stderr");

    fs::remove_dir_all(&g.dir).unwrap();
    fs::remove_dir_all(&o.dir).unwrap();
}
