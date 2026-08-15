//! Rebase integration tests (tracker 12 verification): byte parity vs real
//! git and cross-tool interop on the same repo.
//!
//! Parity runs real git and git-rs on two identically-seeded repos (fixed
//! dates/identity => identical commit shas, and git rebase honors
//! GIT_COMMITTER_DATE for the replayed commits â€” probed), then compares
//! bytes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn scratch_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "git-rs-rebase-{}-{name}-{}",
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
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_EDITOR", "true");
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("failed to run");
        (out.status.code().unwrap_or(-1), out.stdout, out.stderr)
    }

    fn real(&self, args: &[&str]) -> (i32, Vec<u8>, Vec<u8>) {
        self.run("git", args, &[])
    }

    fn rebase_real(&self, args: &[&str], dates: (&str, &str)) -> (i32, Vec<u8>, Vec<u8>) {
        self.run("git", args, &commit_env(dates))
    }

    fn rebase_our(&self, args: &[&str], dates: (&str, &str)) -> (i32, Vec<u8>, Vec<u8>) {
        self.run(env!("CARGO_BIN_EXE_git-rs"), args, &commit_env(dates))
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

    fn file(&self, name: &str) -> Vec<u8> {
        fs::read(self.dir.join(name)).unwrap_or_default()
    }

    fn state_file(&self, name: &str) -> Option<Vec<u8>> {
        fs::read(self.dir.join(".git").join("rebase-merge").join(name)).ok()
    }
}

/// base (a.txt, b.txt) -> feature (a.txt edited twice, d.txt added); main
/// (b.txt edited, c.txt added). Both branches diverge from base; feature is
/// checked out for the rebase.
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
    write_file(&f.dir, "a.txt", "1\n2\nfeat2\n");
    assert_eq!(f.real(&["add", "a.txt"]).0, 0);
    f.commit("feat2", "1786610200 +0530");
    assert_eq!(f.real(&["checkout", "-q", "main"]).0, 0);
    write_file(&f.dir, "b.txt", "b\nmain\n");
    write_file(&f.dir, "c.txt", "c\n");
    assert_eq!(f.real(&["add", "--all"]).0, 0);
    f.commit("main", "1786610300 +0530");
    assert_eq!(f.real(&["checkout", "-q", "feature"]).0, 0);
}

/// Identical `seed_diverged` repos in two fixtures, returned.
fn pair() -> (Fixture, Fixture) {
    let (a, b) = (Fixture::new(), Fixture::new());
    seed_diverged(&a);
    seed_diverged(&b);
    (a, b)
}

/// Feature edits a.txt, main edits a.txt to disjoint content â€” the range is
/// a single pick that conflicts with a whole-file marker (step-11 locked:
/// ort emits whole-file hunks when no line is shared). Feature checked out.
fn seed_edit_conflict(f: &Fixture) {
    write_file(&f.dir, "a.txt", "base\n");
    assert_eq!(f.real(&["add", "a.txt"]).0, 0);
    f.commit("base", "1786610000 +0530");
    assert_eq!(f.real(&["checkout", "-q", "-b", "feature"]).0, 0);
    write_file(&f.dir, "a.txt", "feat-line\nmore\n");
    assert_eq!(f.real(&["add", "a.txt"]).0, 0);
    f.commit("feat", "1786610100 +0530");
    assert_eq!(f.real(&["checkout", "-q", "main"]).0, 0);
    write_file(&f.dir, "a.txt", "main-line\ndiffers\n");
    assert_eq!(f.real(&["add", "a.txt"]).0, 0);
    f.commit("main", "1786610200 +0530");
    assert_eq!(f.real(&["checkout", "-q", "feature"]).0, 0);
}

#[test]
fn replay_is_sha_identical_to_real_git() {
    let (real, ours) = pair();
    let dates = ("1786610400 +0530", "1786610500 +0530");
    let (rc_g, out_g, err_g) = real.rebase_real(&["rebase", "main"], dates);
    assert_eq!(rc_g, 0, "stderr: {}", String::from_utf8_lossy(&err_g));
    let (rc_o, out_o, err_o) = ours.rebase_our(&["rebase", "main"], dates);
    assert_eq!(rc_o, 0, "stderr: {}", String::from_utf8_lossy(&err_o));

    // Progress + success line (stderr; git's progress uses `\r`, no
    // newline); replay stdout is silent for clean picks.
    assert_eq!(
        String::from_utf8_lossy(&err_g),
        "Rebasing (1/2)\rRebasing (2/2)\rSuccessfully rebased and updated refs/heads/feature.\n"
    );
    assert_eq!(err_o, err_g);
    assert!(out_g.is_empty());
    assert_eq!(out_o, out_g);

    // Identical final commit, tree, and history.
    assert_eq!(ours.head_sha(), real.head_sha());
    let (_, lg, _) = real.real(&["log", "--format=%h %s"]);
    let (_, lo, _) = ours.real(&["log", "--format=%h %s"]);
    assert_eq!(lo, lg);
    let (_, pg, _) = real.real(&["log", "-1", "--format=%P"]);
    assert_eq!(String::from_utf8(pg).unwrap().split_whitespace().count(), 1);

    // Author preserved (name/email/date verbatim), fresh committer.
    let (_, ag, _) = real.real(&["log", "-1", "--format=%an <%ae> %ai"]);
    let (_, ao, _) = ours.real(&["log", "-1", "--format=%an <%ae> %ai"]);
    assert_eq!(ao, ag);
    let (_, cg, _) = real.real(&["log", "-1", "--format=%cn <%ce> %ci"]);
    assert_eq!(
        String::from_utf8_lossy(&cg),
        "C O Mitter <c@example.com> 2026-08-13 14:11:40 +0530\n"
    );

    // Worktree files identical; state gone; reflogs identical.
    for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        assert_eq!(ours.file(name), real.file(name), "{name}");
    }
    assert!(ours.state_file("onto").is_none());
    let (_, rg, _) = real.real(&["reflog", "--format=%gs"]);
    let (_, ro, _) = ours.real(&["reflog", "--format=%gs"]);
    assert_eq!(ro, rg);
    assert!(
        String::from_utf8_lossy(&rg).contains("rebase (start): checkout main")
            && String::from_utf8_lossy(&rg).contains("rebase (pick): feat")
            && String::from_utf8_lossy(&rg)
                .contains("rebase (finish): returning to refs/heads/feature")
    );
    let (_, bg, _) = real.real(&["reflog", "feature", "--format=%gs"]);
    assert!(String::from_utf8_lossy(&bg).starts_with("rebase (finish): refs/heads/feature onto "));
}

#[test]
fn up_to_date_matches_real_git() {
    let dates = ("1786610400 +0530", "1786610400 +0530");
    // Case A: linear upstream âŠ† HEAD (feature forked from main) -> git's
    // up-to-date message, no state, rc 0.
    let (real, ours) = (Fixture::new(), Fixture::new());
    for f in [&real, &ours] {
        write_file(&f.dir, "b.txt", "b\n");
        assert_eq!(f.real(&["add", "b.txt"]).0, 0);
        f.commit("main", "1786610000 +0530");
        assert_eq!(f.real(&["checkout", "-q", "-b", "feature"]).0, 0);
        write_file(&f.dir, "a.txt", "feat\n");
        assert_eq!(f.real(&["add", "a.txt"]).0, 0);
        f.commit("feat", "1786610100 +0530");
    }
    let (rc_g, out_g, err_g) = real.rebase_real(&["rebase", "main"], dates);
    let (rc_o, out_o, err_o) = ours.rebase_our(&["rebase", "main"], dates);
    assert_eq!((rc_g, rc_o), (0, 0));
    assert_eq!(out_o, out_g);
    assert_eq!(err_o, err_g);
    assert_eq!(
        String::from_utf8_lossy(&out_g),
        "Current branch feature is up to date.\n"
    );
    assert!(ours.state_file("onto").is_none());
    assert_eq!(ours.head_sha(), real.head_sha()); // nothing replayed

    // Case B: upstream âŠ† HEAD but a merge commit sits on the first-parent
    // chain (main merged into feature) -> the preemptive fast-forward is
    // blocked (git's can_fast_forward + is_linear_history): both replay.
    let (real, ours) = pair();
    for f in [&real, &ours] {
        let (rc, _, err) = f.run(
            "git",
            &["merge", "-q", "--no-ff", "main", "-m", "join"],
            &commit_env(("1786610200 +0530", "1786610200 +0530")),
        );
        assert_eq!(rc, 0, "stderr: {}", String::from_utf8_lossy(&err));
    }
    let (rc_g, out_g, err_g) = real.rebase_real(&["rebase", "main"], dates);
    let (rc_o, out_o, err_o) = ours.rebase_our(&["rebase", "main"], dates);
    assert_eq!((rc_g, rc_o), (0, 0));
    assert_eq!(out_o, out_g);
    assert_eq!(err_o, err_g);
    assert!(!err_g.is_empty()); // it replays: progress + success line
    assert_eq!(ours.head_sha(), real.head_sha()); // replayed identically
    let (_, lg, _) = real.real(&["log", "-1", "--format=%P"]);
    assert_eq!(String::from_utf8(lg).unwrap().split_whitespace().count(), 1);

    // Case C: HEAD is an ancestor of upstream and the range is empty
    // (feature merged into main) -> git fast-forwards the branch to the
    // upstream tip with only the success line on stderr.
    let (real, ours) = pair();
    for f in [&real, &ours] {
        assert_eq!(f.real(&["checkout", "-q", "main"]).0, 0);
        let (rc, _, err) = f.run(
            "git",
            &["merge", "-q", "--no-ff", "feature", "-m", "join"],
            &commit_env(("1786610200 +0530", "1786610200 +0530")),
        );
        assert_eq!(rc, 0, "stderr: {}", String::from_utf8_lossy(&err));
        assert_eq!(f.real(&["checkout", "-q", "feature"]).0, 0);
    }
    let (rc_g, out_g, err_g) = real.rebase_real(&["rebase", "main"], dates);
    let (rc_o, out_o, err_o) = ours.rebase_our(&["rebase", "main"], dates);
    assert_eq!((rc_g, rc_o), (0, 0));
    assert_eq!(out_o, out_g);
    assert_eq!(err_o, err_g);
    assert_eq!(
        String::from_utf8_lossy(&err_g),
        "Successfully rebased and updated refs/heads/feature.\n"
    );
    assert!(out_g.is_empty());
    // The branch ref fast-forwarded to the upstream tip; worktree synced.
    assert_eq!(ours.head_sha(), real.head_sha()); // feature == main tip
    let (_, rg, _) = real.real(&["reflog", "--format=%gs"]);
    let (_, ro, _) = ours.real(&["reflog", "--format=%gs"]);
    assert_eq!(ro, rg);
    assert_eq!(ours.state_file("onto"), real.state_file("onto")); // both gone
    assert!(ours.state_file("onto").is_none());
    assert_eq!(
        ours.real(&["rev-parse", "feature"]).1,
        real.real(&["rev-parse", "feature"]).1
    );
    assert_eq!(
        ours.real(&["rev-parse", "main"]).1,
        ours.real(&["rev-parse", "feature"]).1
    );
    // ORIG_HEAD records the pre-rebase feature tip, like git.
    let read_orig = |f: &Fixture| fs::read(f.dir.join(".git").join("ORIG_HEAD")).unwrap();
    assert_eq!(read_orig(&ours), read_orig(&real));
}

#[test]
fn conflict_stop_is_byte_identical_to_real_git() {
    let (real, ours) = (Fixture::new(), Fixture::new());
    seed_edit_conflict(&real);
    seed_edit_conflict(&ours);

    let dates = ("1786610300 +0530", "1786610300 +0530");
    let (rc_g, out_g, err_g) = real.rebase_real(&["rebase", "main"], dates);
    let (rc_o, out_o, err_o) = ours.rebase_our(&["rebase", "main"], dates);
    assert_eq!((rc_g, rc_o), (1, 1));
    assert_eq!(out_o, out_g); // Auto-merging + CONFLICT lines
    assert_eq!(err_o, err_g); // error: could not apply + hints + Could not apply
    assert_eq!(ours.file("a.txt"), real.file("a.txt")); // markers
    let err_text = String::from_utf8_lossy(&err_g);
    assert!(err_text.starts_with("Rebasing (1/1)\r"));
    assert!(err_text.contains("error: could not apply"));
    assert!(err_text.contains("Could not apply"));
    assert_eq!(
        String::from_utf8_lossy(&out_g),
        "Auto-merging a.txt\nCONFLICT (content): Merge conflict in a.txt\n"
    );

    let (_, ls_g, _) = real.real(&["ls-files", "-s"]);
    let (_, ls_o, _) = ours.real(&["ls-files", "-s"]);
    assert_eq!(ls_o, ls_g); // stages 1/2/3

    // State files we write (git writes more: done/patch/backup â€” ours is
    // an own format, D-019): identical content for the shared set.
    for name in [
        "head-name",
        "onto",
        "orig-head",
        "msgnum",
        "end",
        "message",
        "author-script",
    ] {
        assert_eq!(ours.state_file(name), real.state_file(name), "{name}");
    }
    // Branch ref untouched by the pause; HEAD parked on the onto commit.
    assert_eq!(ours.head_sha(), real.head_sha());
    assert_eq!(
        ours.real(&["rev-parse", "feature"]).1,
        real.real(&["rev-parse", "feature"]).1
    );
}

#[test]
fn continue_matches_real_git_byte_for_byte() {
    let (real, ours) = (Fixture::new(), Fixture::new());
    seed_edit_conflict(&real);
    seed_edit_conflict(&ours);
    let dates = ("1786610300 +0530", "1786610400 +0530");
    assert_eq!(real.rebase_real(&["rebase", "main"], dates).0, 1);
    assert_eq!(ours.rebase_our(&["rebase", "main"], dates).0, 1);

    // Unmerged continue: git's `path: needs merge` block on stdout, rc 1.
    let (rc_g, out_g, err_g) = real.rebase_real(&["rebase", "--continue"], dates);
    let (rc_o, out_o, err_o) = ours.rebase_our(&["rebase", "--continue"], dates);
    assert_eq!((rc_g, rc_o), (1, 1));
    assert_eq!(out_o, out_g);
    assert_eq!(err_o, err_g);
    assert_eq!(
        String::from_utf8_lossy(&out_g),
        "a.txt: needs merge\nYou must edit all merge conflicts and then\nmark them as resolved using git add\n"
    );

    // Resolve identically in both (one line in, one line out), then
    // continue.
    write_file(&real.dir, "a.txt", "main-line\nmore\n");
    write_file(&ours.dir, "a.txt", "main-line\nmore\n");
    assert_eq!(real.real(&["add", "a.txt"]).0, 0);
    assert_eq!(ours.real(&["add", "a.txt"]).0, 0);
    let (rc_g, out_g, err_g) = real.rebase_real(&["rebase", "--continue"], dates);
    let (rc_o, out_o, err_o) = ours.rebase_our(&["rebase", "--continue"], dates);
    assert_eq!((rc_g, rc_o), (0, 0));

    // Byte-identical summary block (git commit 2.55 prints only the
    // summary, no per-file stat lines), identical final commit, state
    // removed, author preserved.
    assert_eq!(out_o, out_g);
    assert_eq!(err_o, err_g);
    let expected = format!(
        "[detached HEAD {}] feat\n Author: A U Thor <a@example.com>\n 1 file changed, 1 insertion(+), 1 deletion(-)\n",
        &ours.head_sha()[..7]
    );
    assert_eq!(String::from_utf8_lossy(&out_g), expected);
    assert_eq!(ours.head_sha(), real.head_sha());
    let (_, ag, _) = real.real(&["log", "-1", "--format=%an <%ae> %ai"]);
    let (_, ao, _) = ours.real(&["log", "-1", "--format=%an <%ae> %ai"]);
    assert_eq!(ao, ag);
    assert!(ours.state_file("onto").is_none());
    let (_, rg, _) = real.real(&["reflog", "--format=%gs"]);
    let (_, ro, _) = ours.real(&["reflog", "--format=%gs"]);
    assert_eq!(ro, rg);
    assert!(String::from_utf8_lossy(&rg).contains("rebase (continue): feat"));
}

#[test]
fn abort_restores_and_missing_state_is_fatal() {
    let (real, ours) = (Fixture::new(), Fixture::new());
    seed_edit_conflict(&real);
    seed_edit_conflict(&ours);
    let dates = ("1786610300 +0530", "1786610300 +0530");
    assert_eq!(real.rebase_real(&["rebase", "main"], dates).0, 1);
    assert_eq!(ours.rebase_our(&["rebase", "main"], dates).0, 1);

    let (rc_g, out_g, err_g) = real.rebase_real(&["rebase", "--abort"], dates);
    let (rc_o, out_o, err_o) = ours.rebase_our(&["rebase", "--abort"], dates);
    assert_eq!((rc_g, rc_o), (0, 0));
    // Silent: nothing on either stream.
    assert!(out_g.is_empty() && err_g.is_empty());
    assert_eq!(out_o, out_g);
    assert_eq!(err_o, err_g);

    // Exact pre-rebase state: HEAD sha, worktree, index, no leftovers.
    assert_eq!(ours.head_sha(), real.head_sha());
    for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        assert_eq!(ours.file(name), real.file(name), "{name}");
    }
    assert_eq!(
        ours.real(&["status", "--porcelain"]).1,
        real.real(&["status", "--porcelain"]).1
    );
    assert!(ours.state_file("onto").is_none());

    // Abort/continue/skip with no rebase in progress.
    for flag in ["--abort", "--continue", "--skip"] {
        let (rc_g, out_g, err_g) = real.rebase_real(&["rebase", flag], dates);
        let (rc_o, out_o, err_o) = ours.rebase_our(&["rebase", flag], dates);
        assert_eq!((rc_g, rc_o), (128, 128), "{flag}");
        assert_eq!(out_o, out_g);
        assert_eq!(err_o, err_g);
        assert_eq!(
            String::from_utf8_lossy(&err_g),
            "fatal: no rebase in progress\n",
            "{flag}"
        );
    }
}

#[test]
fn in_progress_rebase_is_refused_like_real_git() {
    let (real, ours) = (Fixture::new(), Fixture::new());
    seed_edit_conflict(&real);
    seed_edit_conflict(&ours);
    let dates = ("1786610300 +0530", "1786610300 +0530");
    assert_eq!(real.rebase_real(&["rebase", "main"], dates).0, 1);
    assert_eq!(ours.rebase_our(&["rebase", "main"], dates).0, 1);

    let (rc_g, out_g, err_g) = real.rebase_real(&["rebase", "main"], dates);
    let (rc_o, out_o, err_o) = ours.rebase_our(&["rebase", "main"], dates);
    assert_eq!((rc_g, rc_o), (128, 128));
    assert_eq!(out_o, out_g);
    // git's die() text wraps at the console width, so byte-compare ours
    // against the file-captured form; git's own bytes are checked by the
    // stable fragments below.
    assert_eq!(
        String::from_utf8_lossy(&err_o),
        "fatal: It seems that there is already a rebase-merge directory, and\n\
         I wonder if you are in the middle of another rebase.  If that is the\n\
         case, please try\n\
         \tgit rebase (--continue | --abort | --skip)\n\
         If that is not the case, please\n\
         \trm -fr \".git/rebase-merge\"\n\
         and run me again.  I am stopping in case you still have something\n\
         valuable there.\n\n"
    );
    let block = String::from_utf8_lossy(&err_g);
    assert!(
        block.starts_with("fatal: It seems that there is already a rebase-merge directory, and\n")
    );
    assert!(block.contains("\tgit rebase (--continue | --abort | --skip)\n"));
    assert!(block.contains("\trm -fr \".git/rebase-merge\"\n"));
    assert!(block.contains("valuable there."));
}

#[test]
fn originally_empty_commits_are_replayed_like_real_git() {
    let (real, ours) = pair();
    for f in [&real, &ours] {
        assert_eq!(f.real(&["checkout", "-q", "main"]).0, 0);
        // An empty commit on main only: main gains "empty" (tree == base).
        let (rc, _, err) = f.run(
            "git",
            &["commit", "-q", "-m", "empty", "--allow-empty"],
            &commit_env(("1786610600 +0530", "1786610600 +0530")),
        );
        assert_eq!(rc, 0, "stderr: {}", String::from_utf8_lossy(&err));
        assert_eq!(f.real(&["checkout", "-q", "feature"]).0, 0);
    }
    let dates = ("1786610700 +0530", "1786610800 +0530");
    let (rc_g, _, err_g) = real.rebase_real(&["rebase", "main"], dates);
    let (rc_o, _, err_o) = ours.rebase_our(&["rebase", "main"], dates);
    assert_eq!((rc_g, rc_o), (0, 0));

    // The empty commit was replayed (git 2.55 keeps originally-empty
    // commits, drop-empty=false), so the final shas match.
    assert_eq!(ours.head_sha(), real.head_sha());
    let (_, lg, _) = real.real(&["log", "--format=%h %s"]);
    assert!(String::from_utf8_lossy(&lg).contains("empty"));
    let (_, lo, _) = ours.real(&["log", "--format=%h %s"]);
    assert_eq!(lo, lg);
    let _ = err_g;
    let _ = err_o;
}

#[test]
fn merge_commits_are_flattened_in_topo_order_like_git() {
    let (real, ours) = pair();
    // feature: f1..f2, then a topic merged in (second parent) â€” the replay
    // must include the merged side (rev-list --reverse --topo-order) while
    // dropping the merge commit itself.
    for f in [&real, &ours] {
        assert_eq!(f.real(&["checkout", "-q", "-b", "topic"]).0, 0);
        write_file(&f.dir, "t.txt", "t\n");
        assert_eq!(f.real(&["add", "t.txt"]).0, 0);
        f.commit("topic", "1786610400 +0530");
        assert_eq!(f.real(&["checkout", "-q", "feature"]).0, 0);
        let (rc, _, err) = f.run(
            "git",
            &["merge", "-q", "--no-ff", "topic", "-m", "join"],
            &commit_env(("1786610500 +0530", "1786610500 +0530")),
        );
        assert_eq!(rc, 0, "stderr: {}", String::from_utf8_lossy(&err));
        assert_eq!(f.real(&["checkout", "-q", "main"]).0, 0);
        write_file(&f.dir, "c.txt", "c\nmain2\n");
        assert_eq!(f.real(&["add", "c.txt"]).0, 0);
        f.commit("main2", "1786610600 +0530");
        assert_eq!(f.real(&["checkout", "-q", "feature"]).0, 0);
    }
    let dates = ("1786610700 +0530", "1786610800 +0530");
    let (rc_g, _, err_g) = real.rebase_real(&["rebase", "main"], dates);
    let (rc_o, _, err_o) = ours.rebase_our(&["rebase", "main"], dates);
    assert_eq!(
        (rc_g, rc_o),
        (0, 0),
        "git: {}\nours: {}",
        String::from_utf8_lossy(&err_g),
        String::from_utf8_lossy(&err_o)
    );

    // Same final history: feat, feat2, topic replayed (merge dropped) â€”
    // flattened and sha-identical.
    assert_eq!(ours.head_sha(), real.head_sha());
    let (_, lg, _) = real.real(&["log", "--format=%h %s"]);
    let (_, lo, _) = ours.real(&["log", "--format=%h %s"]);
    assert_eq!(lo, lg);
    let history = String::from_utf8_lossy(&lg);
    assert!(history.contains("topic") && !history.contains("join"));
    let (_, pg, _) = real.real(&["log", "-1", "--format=%P"]);
    assert_eq!(String::from_utf8(pg).unwrap().split_whitespace().count(), 1);
}

#[test]
fn real_git_accepts_our_rebased_repo() {
    let (_, ours) = pair();
    let dates = ("1786610400 +0530", "1786610500 +0530");
    assert_eq!(ours.rebase_our(&["rebase", "main"], dates).0, 0);
    // fsck clean, reflog readable, log sane.
    let (rc, out, _) = ours.real(&["fsck"]);
    assert_eq!(rc, 0, "fsck: {}", String::from_utf8_lossy(&out));
    let (_, log, _) = ours.real(&["log", "--format=%h %s"]);
    assert!(!String::from_utf8_lossy(&log).is_empty());

    // Real git sees our paused rebase: `status` reads our state dir
    // (head-name/onto) and reports the in-progress state (free interop).
    let ours = Fixture::new();
    seed_edit_conflict(&ours);
    let dates = ("1786610300 +0530", "1786610300 +0530");
    assert_eq!(ours.rebase_our(&["rebase", "main"], dates).0, 1);
    let (_, out, _) = ours.real(&["status"]);
    let status = String::from_utf8_lossy(&out);
    assert!(status.contains("You are currently rebasing branch 'feature' on "));
    assert!(status.contains("onto "));
    // Resolve and finish with OUR --continue; real git accepts the result.
    write_file(&ours.dir, "a.txt", "main-line\nmore\n");
    assert_eq!(ours.real(&["add", "a.txt"]).0, 0);
    assert_eq!(
        ours.rebase_our(
            &["rebase", "--continue"],
            ("1786610400 +0530", "1786610400 +0530")
        )
        .0,
        0
    );
    let (rc, out, _) = ours.real(&["fsck"]);
    assert_eq!(
        rc,
        0,
        "fsck after our continue: {}",
        String::from_utf8_lossy(&out)
    );
    assert!(ours.state_file("onto").is_none());
}

#[test]
fn bad_upstream_is_fatal_like_real_git() {
    let (real, ours) = pair();
    let dates = ("1786610400 +0530", "1786610400 +0530");
    let (rc_g, out_g, err_g) = real.rebase_real(&["rebase", "nope"], dates);
    let (rc_o, out_o, err_o) = ours.rebase_our(&["rebase", "nope"], dates);
    assert_eq!((rc_g, rc_o), (128, 128));
    assert_eq!(out_o, out_g);
    assert_eq!(err_o, err_g);
    assert_eq!(
        String::from_utf8_lossy(&err_g),
        "fatal: invalid upstream 'nope'\n"
    );
}
