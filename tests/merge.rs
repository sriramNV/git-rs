//! Merge integration tests (tracker 11 verification): byte parity vs real
//! git and cross-tool interop on the same repo.
//!
//! Parity runs real git and git-rs on two identically-seeded repos (fixed
//! dates/identity => identical commit shas), then compares bytes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn scratch_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "git-rs-merge-{}-{name}-{}",
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

    fn tree_sha(&self, rev: &str) -> String {
        let (_, out, _) = self.real(&["rev-parse", &format!("{rev}^{{tree}}")]);
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

    fn file(&self, name: &str) -> Vec<u8> {
        fs::read(self.dir.join(name)).unwrap_or_default()
    }

    fn state_file(&self, name: &str) -> Option<Vec<u8>> {
        fs::read(self.dir.join(".git").join(name)).ok()
    }
}

/// base (a.txt, b.txt) -> feature (a.txt edited, d.txt added); main
/// (b.txt edited, c.txt added). Both branches diverge from base.
fn seed_diverged(f: &Fixture) {
    write_file(&f.dir, "a.txt", "1\n2\n");
    write_file(&f.dir, "b.txt", "b\n");
    assert_eq!(f.real(&["add", "--all"]).0, 0);
    f.commit("base", "1786610000 +0530");
    assert_eq!(f.real(&["checkout", "-q", "-b", "feature"]).0, 0);
    write_file(&f.dir, "a.txt", "1\n2\nfeat\n");
    write_file(&f.dir, "d.txt", "d\n");
    assert_eq!(f.real(&["add", "--all"]).0, 0);
    f.commit("feat", "1786610100 +0530");
    assert_eq!(f.real(&["checkout", "-q", "main"]).0, 0);
    write_file(&f.dir, "b.txt", "b\nmain\n");
    write_file(&f.dir, "c.txt", "c\n");
    assert_eq!(f.real(&["add", "--all"]).0, 0);
    f.commit("main", "1786610200 +0530");
}

/// Identical `seed_diverged` repos in two fixtures, returned.
fn pair() -> (Fixture, Fixture) {
    let (a, b) = (Fixture::new(), Fixture::new());
    seed_diverged(&a);
    seed_diverged(&b);
    (a, b)
}

#[test]
fn clean_merge_is_byte_identical_to_real_git() {
    let (real, ours) = pair();
    let dates = ("1786610300 +0530", "1786610300 +0530");
    let (rc_g, out_g, err_g) = real.run("git", &["merge", "feature"], &commit_env(dates));
    assert_eq!(rc_g, 0);
    let (rc_o, out_o, err_o) = ours.run(
        env!("CARGO_BIN_EXE_git-rs"),
        &["merge", "feature"],
        &commit_env(dates),
    );
    assert_eq!(rc_o, 0, "stderr: {}", String::from_utf8_lossy(&err_o));

    // Success line (stdout, with the diffstat cut — D-018: git prints a
    // stat block after it; assert only the shared line).
    assert!(String::from_utf8_lossy(&out_g).starts_with("Merge made by the 'ort' strategy.\n"));
    assert!(out_o.starts_with(b"Merge made by the 'ort' strategy.\n"));
    assert!(err_g.is_empty());
    assert!(err_o.is_empty());

    // Identical merge commit, tree, and reflog message.
    assert_eq!(ours.head_sha(), real.head_sha());
    assert_eq!(ours.tree_sha("HEAD"), real.tree_sha("HEAD"));
    let (_, lg, _) = real.real(&["reflog", "-1", "--format=%gs"]);
    let (_, lo, _) = ours.real(&["reflog", "-1", "--format=%gs"]);
    assert_eq!(lo, lg);
    assert!(
        String::from_utf8_lossy(&lg)
            .starts_with("merge feature: Merge made by the 'ort' strategy.")
    );

    // Worktree files identical.
    for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        assert_eq!(ours.file(name), real.file(name), "{name}");
    }

    // Two parents, message `Merge branch 'feature'`.
    let (_, p_g, _) = real.real(&["log", "-1", "--format=%P"]);
    let (_, p_o, _) = ours.real(&["log", "-1", "--format=%P"]);
    assert_eq!(p_o, p_g);
    let p_text = String::from_utf8(p_g).unwrap();
    assert_eq!(p_text.split_whitespace().count(), 2);
    let (_, s_g, _) = real.real(&["log", "-1", "--format=%s"]);
    let (_, s_o, _) = ours.real(&["log", "-1", "--format=%s"]);
    assert_eq!(s_o, s_g);
    assert_eq!(String::from_utf8_lossy(&s_g), "Merge branch 'feature'\n");

    // Merge state files are gone; ORIG_HEAD records the pre-merge commit.
    assert!(ours.state_file("MERGE_HEAD").is_none());
    assert!(ours.state_file("MERGE_MSG").is_none());
    assert!(real.state_file("ORIG_HEAD").is_some());
    assert_eq!(ours.state_file("ORIG_HEAD"), real.state_file("ORIG_HEAD"));
}

#[test]
fn content_conflict_state_is_byte_identical_to_real_git() {
    let (real, ours) = pair();
    // Diverged edits of the same file on both sides, no line in common with
    // the base — git's ort then emits a single whole-file conflict hunk,
    // byte-identical to ours. (With a shared prefix git shrinks the hunk;
    // whole-file markers are a locked deviation there, D-018.)
    write_file(&real.dir, "a.txt", "feat-line\nmore\n");
    write_file(&ours.dir, "a.txt", "feat-line\nmore\n");
    assert_eq!(real.real(&["add", "--all"]).0, 0);
    assert_eq!(ours.real(&["add", "--all"]).0, 0);
    real.commit("feat2", "1786610300 +0530");
    ours.commit("feat2", "1786610300 +0530");
    write_file(&real.dir, "a.txt", "main-line\ndiffers\n");
    write_file(&ours.dir, "a.txt", "main-line\ndiffers\n");
    assert_eq!(real.real(&["add", "--all"]).0, 0);
    assert_eq!(ours.real(&["add", "--all"]).0, 0);
    real.commit("main2", "1786610400 +0530");
    ours.commit("main2", "1786610400 +0530");

    let (rc_g, out_g, err_g) = real.real(&["merge", "feature"]);
    assert_eq!(rc_g, 1);
    let (rc_o, out_o, err_o) = ours.our(&["merge", "feature"]);
    assert_eq!(rc_o, 1, "stderr: {}", String::from_utf8_lossy(&err_o));

    assert_eq!(out_o, out_g); // Auto-merging + CONFLICT + failure line
    assert_eq!(err_o, err_g);
    assert_eq!(ours.file("a.txt"), real.file("a.txt")); // marker bytes

    let (_, ls_g, _) = real.real(&["ls-files", "-s"]);
    let (_, ls_o, _) = ours.real(&["ls-files", "-s"]);
    assert_eq!(ls_o, ls_g); // stages 1/2/3

    assert_eq!(ours.state_file("MERGE_HEAD"), real.state_file("MERGE_HEAD"));
    assert_eq!(ours.state_file("MERGE_MSG"), real.state_file("MERGE_MSG"));
    assert_eq!(ours.state_file("ORIG_HEAD"), real.state_file("ORIG_HEAD"));
    assert!(ours.state_file("MERGE_MSG").is_some());
    assert!(ours.state_file("MERGE_HEAD").is_some());
}

#[test]
fn add_add_conflict_is_byte_identical_to_real_git() {
    let (real, ours) = pair();
    write_file(&real.dir, "e.txt", "theirs-add\n");
    write_file(&ours.dir, "e.txt", "theirs-add\n");
    assert_eq!(real.real(&["checkout", "-q", "-b", "aa"]).0, 0);
    assert_eq!(ours.real(&["checkout", "-q", "-b", "aa"]).0, 0);
    assert_eq!(real.real(&["add", "e.txt"]).0, 0);
    assert_eq!(ours.real(&["add", "e.txt"]).0, 0);
    real.commit("theirs", "1786610500 +0530");
    ours.commit("theirs", "1786610500 +0530");
    assert_eq!(real.real(&["checkout", "-q", "main"]).0, 0);
    assert_eq!(ours.real(&["checkout", "-q", "main"]).0, 0);
    write_file(&real.dir, "e.txt", "ours-add\n");
    write_file(&ours.dir, "e.txt", "ours-add\n");
    assert_eq!(real.real(&["add", "e.txt"]).0, 0);
    assert_eq!(ours.real(&["add", "e.txt"]).0, 0);
    real.commit("ours", "1786610600 +0530");
    ours.commit("ours", "1786610600 +0530");

    let (rc_g, out_g, err_g) = real.real(&["merge", "aa"]);
    let (rc_o, out_o, err_o) = ours.our(&["merge", "aa"]);
    assert_eq!((rc_g, rc_o), (1, 1));
    assert_eq!(out_o, out_g);
    assert_eq!(err_o, err_g);
    assert_eq!(ours.file("e.txt"), real.file("e.txt"));
    let (_, ls_g, _) = real.real(&["ls-files", "-s"]);
    let (_, ls_o, _) = ours.real(&["ls-files", "-s"]);
    assert_eq!(ls_o, ls_g); // stages 2/3 only for add/add
    assert_eq!(ours.state_file("MERGE_MSG"), real.state_file("MERGE_MSG"));
}

#[test]
fn modify_delete_conflicts_are_byte_identical() {
    // Case 1: we modified, they deleted.
    let (real, ours) = pair();
    write_file(&real.dir, "a.txt", "1\n2\nmain2\n");
    write_file(&ours.dir, "a.txt", "1\n2\nmain2\n");
    assert_eq!(real.real(&["add", "--all"]).0, 0);
    assert_eq!(ours.real(&["add", "--all"]).0, 0);
    real.commit("ours-mod", "1786610700 +0530");
    ours.commit("ours-mod", "1786610700 +0530");
    assert_eq!(real.real(&["checkout", "-q", "feature"]).0, 0);
    assert_eq!(ours.real(&["checkout", "-q", "feature"]).0, 0);
    assert_eq!(real.real(&["rm", "-q", "a.txt"]).0, 0);
    assert_eq!(ours.real(&["rm", "-q", "a.txt"]).0, 0);
    real.commit("their-del", "1786610800 +0530");
    ours.commit("their-del", "1786610800 +0530");
    assert_eq!(real.real(&["checkout", "-q", "main"]).0, 0);
    assert_eq!(ours.real(&["checkout", "-q", "main"]).0, 0);
    let (rc_g, out_g, _) = real.real(&["merge", "feature"]);
    let (rc_o, out_o, _) = ours.our(&["merge", "feature"]);
    assert_eq!((rc_g, rc_o), (1, 1));
    assert_eq!(out_o, out_g);
    let (_, ls_g, _) = real.real(&["ls-files", "-s"]);
    let (_, ls_o, _) = ours.real(&["ls-files", "-s"]);
    assert_eq!(ls_o, ls_g); // stages 1 + 2
    assert_eq!(ours.file("a.txt"), real.file("a.txt"));

    // Case 2: we deleted, they modified.
    let (real, ours) = pair();
    assert_eq!(real.real(&["checkout", "-q", "feature"]).0, 0);
    assert_eq!(ours.real(&["checkout", "-q", "feature"]).0, 0);
    write_file(&real.dir, "a.txt", "1\n2\nfeat2\n");
    write_file(&ours.dir, "a.txt", "1\n2\nfeat2\n");
    assert_eq!(real.real(&["add", "--all"]).0, 0);
    assert_eq!(ours.real(&["add", "--all"]).0, 0);
    real.commit("their-mod", "1786610900 +0530");
    ours.commit("their-mod", "1786610900 +0530");
    assert_eq!(real.real(&["checkout", "-q", "main"]).0, 0);
    assert_eq!(ours.real(&["checkout", "-q", "main"]).0, 0);
    assert_eq!(real.real(&["rm", "-q", "a.txt"]).0, 0);
    assert_eq!(ours.real(&["rm", "-q", "a.txt"]).0, 0);
    real.commit("our-del", "1786611000 +0530");
    ours.commit("our-del", "1786611000 +0530");
    let (rc_g, out_g, _) = real.real(&["merge", "feature"]);
    let (rc_o, out_o, _) = ours.our(&["merge", "feature"]);
    assert_eq!((rc_g, rc_o), (1, 1));
    assert_eq!(out_o, out_g);
    let (_, ls_g, _) = real.real(&["ls-files", "-s"]);
    let (_, ls_o, _) = ours.real(&["ls-files", "-s"]);
    assert_eq!(ls_o, ls_g); // stages 1 + 3
    // Their version left in the worktree.
    assert_eq!(ours.file("a.txt"), real.file("a.txt"));
}

#[test]
fn real_git_finishes_our_merge() {
    let (_, ours) = pair();
    write_file(&ours.dir, "a.txt", "1\n2\nfeat2\n");
    assert_eq!(ours.real(&["add", "--all"]).0, 0);
    ours.commit("feat2", "1786611100 +0530");
    write_file(&ours.dir, "a.txt", "1\n2\nmain2\n");
    assert_eq!(ours.real(&["add", "--all"]).0, 0);
    ours.commit("main2", "1786611200 +0530");
    let (rc, _, _) = ours.our(&["merge", "feature"]);
    assert_eq!(rc, 1);
    assert!(ours.state_file("MERGE_HEAD").is_some());

    // Real git resolves and commits our conflicted state.
    assert_eq!(ours.real(&["add", "a.txt"]).0, 0);
    let (rc, _, err) = ours.run(
        "git",
        &["commit", "-q", "-m", "resolved"],
        &commit_env(("1786611300 +0530", "1786611300 +0530")),
    );
    assert_eq!(rc, 0, "stderr: {}", String::from_utf8_lossy(&err));
    let (_, p, _) = ours.real(&["log", "-1", "--format=%P"]);
    assert_eq!(String::from_utf8(p).unwrap().split_whitespace().count(), 2);
    assert!(ours.state_file("MERGE_HEAD").is_none());
}

#[test]
fn we_finish_real_git_merge_using_merge_msg() {
    let (real, _) = pair();
    write_file(&real.dir, "a.txt", "1\n2\nfeat2\n");
    assert_eq!(real.real(&["add", "--all"]).0, 0);
    real.commit("feat2", "1786611400 +0530");
    write_file(&real.dir, "a.txt", "1\n2\nmain2\n");
    assert_eq!(real.real(&["add", "--all"]).0, 0);
    real.commit("main2", "1786611500 +0530");
    let (rc, _, _) = real.real(&["merge", "feature"]);
    assert_eq!(rc, 1);
    assert_eq!(real.real(&["add", "a.txt"]).0, 0);

    // No -m: the message comes from MERGE_MSG with comments stripped.
    let (rc, out, err) = real.run(
        env!("CARGO_BIN_EXE_git-rs"),
        &["commit"],
        &commit_env(("1786611600 +0530", "1786611600 +0530")),
    );
    assert_eq!(
        rc,
        0,
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out),
        String::from_utf8_lossy(&err)
    );
    let (_, s, _) = real.real(&["log", "-1", "--format=%s"]);
    assert_eq!(String::from_utf8_lossy(&s), "Merge branch 'feature'\n");
    let (_, p, _) = real.real(&["log", "-1", "--format=%P"]);
    assert_eq!(String::from_utf8(p).unwrap().split_whitespace().count(), 2);
    assert!(real.state_file("MERGE_HEAD").is_none());
    let (_, lg, _) = real.real(&["reflog", "-1", "--format=%gs"]);
    assert!(String::from_utf8_lossy(&lg).starts_with("commit (merge): Merge branch 'feature'"));
}

#[test]
fn abort_matches_real_git_and_missing_abort_errors() {
    let (real, ours) = pair();
    write_file(&real.dir, "a.txt", "1\n2\nfeat2\n");
    write_file(&ours.dir, "a.txt", "1\n2\nfeat2\n");
    assert_eq!(real.real(&["add", "--all"]).0, 0);
    assert_eq!(ours.real(&["add", "--all"]).0, 0);
    real.commit("feat2", "1786611700 +0530");
    ours.commit("feat2", "1786611700 +0530");
    write_file(&real.dir, "a.txt", "1\n2\nmain2\n");
    write_file(&ours.dir, "a.txt", "1\n2\nmain2\n");
    assert_eq!(real.real(&["add", "--all"]).0, 0);
    assert_eq!(ours.real(&["add", "--all"]).0, 0);
    real.commit("main2", "1786611800 +0530");
    ours.commit("main2", "1786611800 +0530");
    assert_eq!(real.real(&["merge", "feature"]).0, 1);
    assert_eq!(ours.our(&["merge", "feature"]).0, 1);

    let (rc_g, out_g, err_g) = real.real(&["merge", "--abort"]);
    let (rc_o, out_o, err_o) = ours.our(&["merge", "--abort"]);
    assert_eq!((rc_g, rc_o), (0, 0));
    assert_eq!(out_o, out_g);
    assert_eq!(err_o, err_g);
    assert_eq!(ours.head_sha(), real.head_sha());
    assert_eq!(ours.file("a.txt"), real.file("a.txt"));
    assert_eq!(
        ours.real(&["status", "--porcelain"]).1,
        real.real(&["status", "--porcelain"]).1
    );

    // Aborting with no merge in progress.
    let (rc_g, out_g, err_g) = real.real(&["merge", "--abort"]);
    let (rc_o, out_o, err_o) = ours.our(&["merge", "--abort"]);
    assert_eq!((rc_g, rc_o), (128, 128));
    assert_eq!(out_o, out_g);
    assert_eq!(err_o, err_g);
    assert_eq!(
        String::from_utf8_lossy(&err_g),
        "fatal: There is no merge to abort (MERGE_HEAD missing).\n"
    );
}

#[test]
fn dirty_index_is_refused_like_real_git() {
    let (real, ours) = pair();
    write_file(&real.dir, "a.txt", "1\n2\nlocal\n");
    write_file(&ours.dir, "a.txt", "1\n2\nlocal\n");
    assert_eq!(real.real(&["add", "a.txt"]).0, 0);
    assert_eq!(ours.real(&["add", "a.txt"]).0, 0);

    let (rc_g, out_g, err_g) = real.real(&["merge", "feature"]);
    let (rc_o, out_o, err_o) = ours.our(&["merge", "feature"]);
    assert_eq!((rc_g, rc_o), (2, 2));
    assert_eq!(err_o, err_g);
    assert_eq!(out_o, out_g);
    assert_eq!(
        String::from_utf8_lossy(&err_g),
        "error: Your local changes to the following files would be overwritten by merge:\n  a.txt\n\
         Merge with strategy ort failed.\n"
    );
    assert!(ours.state_file("ORIG_HEAD").is_none());
}

#[test]
fn unrelated_histories_are_refused_like_real_git() {
    let (real, ours) = pair();
    for f in [&real, &ours] {
        assert_eq!(f.real(&["checkout", "-q", "--orphan", "solo"]).0, 0);
        write_file(&f.dir, "solo.txt", "s\n");
        assert_eq!(f.real(&["add", "solo.txt"]).0, 0);
        f.commit("solo", "1786611900 +0530");
        assert_eq!(f.real(&["checkout", "-q", "main"]).0, 0);
    }
    let (rc_g, out_g, err_g) = real.real(&["merge", "solo"]);
    let (rc_o, out_o, err_o) = ours.our(&["merge", "solo"]);
    assert_eq!((rc_g, rc_o), (128, 128));
    assert_eq!(out_o, out_g);
    assert_eq!(err_o, err_g);
    assert_eq!(
        String::from_utf8_lossy(&err_g),
        "fatal: refusing to merge unrelated histories\n"
    );
}

#[test]
fn commit_with_unmerged_index_matches_real_git() {
    let (real, ours) = pair();
    write_file(&real.dir, "a.txt", "1\n2\nfeat2\n");
    write_file(&ours.dir, "a.txt", "1\n2\nfeat2\n");
    assert_eq!(real.real(&["add", "--all"]).0, 0);
    assert_eq!(ours.real(&["add", "--all"]).0, 0);
    real.commit("feat2", "1786612000 +0530");
    ours.commit("feat2", "1786612000 +0530");
    write_file(&real.dir, "a.txt", "1\n2\nmain2\n");
    write_file(&ours.dir, "a.txt", "1\n2\nmain2\n");
    assert_eq!(real.real(&["add", "--all"]).0, 0);
    assert_eq!(ours.real(&["add", "--all"]).0, 0);
    real.commit("main2", "1786612100 +0530");
    ours.commit("main2", "1786612100 +0530");
    assert_eq!(real.real(&["merge", "feature"]).0, 1);
    assert_eq!(ours.our(&["merge", "feature"]).0, 1);

    let (rc_g, out_g, err_g) = real.real(&["commit", "-m", "x"]);
    let (rc_o, out_o, err_o) = ours.our(&["commit", "-m", "x"]);
    assert_eq!((rc_g, rc_o), (128, 128));
    assert_eq!(out_o, out_g);
    assert_eq!(err_o, err_g);
    assert_eq!(String::from_utf8_lossy(&out_g), "U\ta.txt\n");
    assert!(
        String::from_utf8_lossy(&err_g)
            .starts_with("error: Committing is not possible because you have unmerged files.")
    );
}

#[test]
fn merge_base_parity() {
    let (real, ours) = pair();
    let (rc_g, out_g, err_g) = real.real(&["merge-base", "main", "feature"]);
    let (rc_o, out_o, err_o) = ours.our(&["merge-base", "main", "feature"]);
    assert_eq!((rc_g, rc_o), (0, 0));
    assert_eq!(out_o, out_g);
    assert_eq!(err_o, err_g);

    // Bad rev: git's `Not a valid object name` fatal.
    let (rc_g, out_g, err_g) = real.real(&["merge-base", "main", "nope"]);
    let (rc_o, out_o, err_o) = ours.our(&["merge-base", "main", "nope"]);
    assert_eq!((rc_g, rc_o), (128, 128));
    assert_eq!(out_o, out_g);
    assert_eq!(err_o, err_g);
    assert_eq!(
        String::from_utf8_lossy(&err_g),
        "fatal: Not a valid object name nope\n"
    );
}
