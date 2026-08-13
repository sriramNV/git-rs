//! Diff integration tests: byte-identical output vs real git.
//!
//! Tracker 08 verification: `git-rs diff` / `git-rs diff --cached` produce
//! output byte-identical to real git 2.55 on the same repository, across
//! modified lines, insertions, deletions, hunks split across context,
//! new/deleted files, binary files, and trailing-newline differences.
//! Multi-change fixtures use distinct lines on purpose: placement within
//! all-identical runs is a known deviation (decisions.md D-014).

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn scratch_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "git-rs-diff-{}-{name}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

/// A single repository; real git and git-rs run against the same state.
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
        // Keep content and index byte-identical on both sides.
        Command::new("git")
            .args(["config", "core.autocrlf", "false"])
            .current_dir(&dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        Fixture { dir }
    }

    fn seed_commit(&self, files: &[(&str, &str)]) {
        for (name, content) in files {
            let path = self.dir.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, content).unwrap();
        }
        self.real(&["add", "--all"]);
        let commit = Command::new("git")
            .args([
                "-c",
                "user.name=A U Thor",
                "-c",
                "user.email=a@example.com",
                "commit",
                "-q",
                "-m",
                "seed",
            ])
            .current_dir(&self.dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(commit.status.success(), "seed commit failed");
    }

    fn real(&self, args: &[&str]) -> (i32, Vec<u8>, Vec<u8>) {
        self.run_in("git", &self.dir, args)
    }

    fn our(&self, args: &[&str]) -> (i32, Vec<u8>, Vec<u8>) {
        self.run_in(env!("CARGO_BIN_EXE_git-rs"), &self.dir, args)
    }

    fn run_in(&self, bin: &str, cwd: &PathBuf, args: &[&str]) -> (i32, Vec<u8>, Vec<u8>) {
        let out = Command::new(bin)
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("failed to run");
        (out.status.code().unwrap_or(-1), out.stdout, out.stderr)
    }

    /// Assert identical stdout and exit code for real git and git-rs.
    fn assert_same(&self, args: &[&str]) {
        let (rc, out, _) = self.real(args);
        let (orc, oout, oerr) = self.our(args);
        assert_eq!(
            orc,
            rc,
            "exit for {args:?}\nreal: {:?}\nours: {:?}\nours stderr: {}",
            String::from_utf8_lossy(&out),
            String::from_utf8_lossy(&oout),
            String::from_utf8_lossy(&oerr)
        );
        assert_eq!(
            oout,
            out,
            "stdout for {args:?} differs\nreal: {:?}\nours: {:?}",
            String::from_utf8_lossy(&out),
            String::from_utf8_lossy(&oout)
        );
    }
}

fn distinct_lines(n: usize) -> String {
    (1..=n).map(|i| format!("line{i}\n")).collect()
}

fn write(f: &Fixture, name: &str, content: &str) {
    fs::write(f.dir.join(name), content).unwrap();
}

#[test]
fn staged_and_worktree_diffs_byte_identical() {
    let f = Fixture::new();
    f.seed_commit(&[
        ("f_mod.txt", &distinct_lines(12)),
        ("f_split.txt", &distinct_lines(13)),
        ("f_noeol.txt", "a\nb\n"),
        ("b.bin", "A\0B\0"),
        ("d.txt", "gone\n"),
        ("sub/deep.txt", "deep\n"),
    ]);

    // Clean tree: both silent.
    f.assert_same(&["diff"]);
    f.assert_same(&["diff", "--cached"]);

    // Stage a spread of changes: modified lines, insertions, deletions,
    // a 7-line gap splitting the diff into two hunks, dropped final
    // newline, binary change, new file, deleted file.
    let lines12: Vec<String> = distinct_lines(12)
        .lines()
        .map(|s| format!("{s}\n"))
        .collect();
    let mut mod12 = lines12.clone();
    mod12[1] = "MOD2\n".into(); // modified middle line
    mod12[4] = "MOD5\n".into(); // modified line
    mod12.insert(7, "INS8\n".into()); // insertion
    mod12.remove(9); // deletion
    write(&f, "f_mod.txt", &mod12.concat());

    let lines13: Vec<String> = distinct_lines(13)
        .lines()
        .map(|s| format!("{s}\n"))
        .collect();
    let mut split = lines13.clone();
    split[2] = "SPLIT3\n".into(); // 7 unchanged lines to the second edit
    split[10] = "SPLIT11\n".into();
    write(&f, "f_split.txt", &split.concat());

    write(&f, "f_noeol.txt", "a\nb"); // final newline removed
    write(&f, "b.bin", "C\0D\0E\0");
    fs::remove_file(f.dir.join("d.txt")).unwrap();
    write(&f, "n.txt", "new\n");

    // Worktree diff covers it all; single-path filtering on both sides.
    f.assert_same(&["diff"]);
    f.assert_same(&["diff", "--", "f_mod.txt"]);
    f.assert_same(&["diff", "--", "sub/deep.txt"]); // unchanged -> silent

    // Stage everything: --cached now shows the full change set.
    f.real(&["add", "--all"]);
    f.assert_same(&["diff"]);
    f.assert_same(&["diff", "--cached"]);
    f.assert_same(&["diff", "--cached", "--", "f_split.txt"]);

    // Unstaged edit on top of staged state: worktree diff narrows to it,
    // --cached keeps showing the staged set.
    let mut staged2 = mod12.clone();
    staged2[9] = "MOD10\n".into();
    write(&f, "f_mod.txt", &staged2.concat());
    f.assert_same(&["diff"]);
    f.assert_same(&["diff", "--cached"]);
}

#[test]
fn funcname_suffix_matches_git() {
    // Hunk with a letter-leading line above it gets the suffix; a second
    // hunk with only digit lines above it carries it over (sticky).
    let f = Fixture::new();
    let mut lines: Vec<String> = vec!["lineA\n".into()];
    lines.extend((1..=20).map(|i| format!("{i}\n")));
    f.seed_commit(&[("f.txt", &lines.concat())]);
    let mut new = lines.clone();
    new[1] = "y\n".into();
    new[13] = "z\n".into();
    write(&f, "f.txt", &new.concat());
    f.assert_same(&["diff"]);
}

#[test]
fn funcname_truncates_long_lines_to_80() {
    // The nearest letter-leading line is 200 bytes: git's def_ff caps the
    // suffix at 80 bytes (probed against git 2.55).
    let f = Fixture::new();
    let mut lines: Vec<String> = vec![format!("L{}\n", "a".repeat(200))];
    lines.extend((1..=10).map(|i| format!("{i}\n")));
    f.seed_commit(&[("f.txt", &lines.concat())]);
    let mut new = lines.clone();
    new[10] = "11\n".into();
    write(&f, "f.txt", &new.concat());
    f.assert_same(&["diff"]);
}

#[test]
fn quoting_and_binary_cases_match_git() {
    // Spaced and non-ASCII names exercise quote_two (CQUOTE_NODQ: spaces
    // unquoted, non-ASCII octal-escaped), the ---/+++ trailing-tab rule,
    // and the /dev/null labels in the Binary files line.
    let f = Fixture::new();
    f.seed_commit(&[
        ("sp ace.txt", "one\ntwo\n"),
        ("caf\u{e9}.txt", "one\n"),
        ("b.bin", "A\0B\0"),
        ("gone.bin", "G\0O\0"),
    ]);

    write(&f, "sp ace.txt", "one\nTWO\n");
    write(&f, "caf\u{e9}.txt", "ONE\n");
    write(&f, "b.bin", "C\0D\0");
    fs::remove_file(f.dir.join("gone.bin")).unwrap();
    fs::write(f.dir.join("n.bin"), [9u8, 0, 10]).unwrap();
    f.assert_same(&["diff"]);
    f.assert_same(&["diff", "--", "sp ace.txt"]);

    f.real(&["add", "--all"]);
    f.assert_same(&["diff", "--cached"]);
}
