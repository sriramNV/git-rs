//! `git-rs rebase <upstream>` and `git-rs rebase (--continue|--abort|--skip)`.
//!
//! Byte-parity goals (probed against git 2.55): there is NO fast-forward —
//! a diverged HEAD always replays every commit with new shas; the only
//! no-op is `Current branch <b> is up to date.` (stdout, rc 0) when the
//! upstream contains HEAD. Progress `Rebasing (k/N)` and the success line
//! `Successfully rebased and updated refs/heads/<b>.` go to stderr;
//! `Auto-merging`/`CONFLICT` lines to stdout; the conflict-stop block
//! (`error: could not apply <7sha>... <subject>` + hints + `Could not apply
//! <7sha>... # <subject>`) to stderr, rc 1. Reflogs: `rebase (start):
//! checkout <upstream>`, `rebase (pick|continue): <subject>`, `rebase
//! (finish): returning to refs/heads/<b>` (HEAD) and `rebase (finish):
//! refs/heads/<b> onto <onto>` (branch); abort logs only HEAD with `rebase
//! (abort): returning to refs/heads/<b>`. ORIG_HEAD = pre-rebase HEAD.
//!
//! Replay = cherry-pick: a 3-way merge of the pick's parent tree (base),
//! the current HEAD tree (ours), and the pick's tree (theirs), so conflict
//! markers and stages are byte-identical to merge's; the marker label is
//! `<7sha> (<subject>)` (probed). Author (name/email/date) is preserved
//! verbatim, committer is fresh. Originally-empty commits are replayed and
//! kept; a pick that becomes empty (merged tree == current tree) is dropped
//! silently (D-019: git's `warning: skipped previously applied commit` is
//! not emitted in v1 — same end state).
//!
//! Locked deviations (D-019): detached HEAD is refused (git supports it)
//! with our own fatal wording; state under `.git/rebase-merge/` is our own
//! persistence format (a real git's directory is refused with its probed
//! block); `rebase -i` style options, `--onto`, `--keep-empty` and
//! `--reapply-cherry-picks` are not implemented.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use crate::commands::commit::{
    commit_identities, strip_comment_lines, tree_from_index, write_commit,
};
use crate::commands::merge::{apply_merged_files, print_merged_lines};
use crate::commands::reset::hard_sync;
use crate::commands::show::stat_summary;
use crate::error::{GitError, IoContext, Result};
use crate::index::Index;
use crate::merge::{MergeFile, MergeResult, merge_trees};
use crate::object::{Commit, Ident};
use crate::refs::Refs;
use crate::revwalk::{hex, merge_base, resolve_rev, unborn_fatal};
use crate::store::{Kind, ObjectStore};
use crate::worktree::{abs_git_dir, index_path, parse_oid, repo_root, tree_entries};

/// One commit replayed onto the new base, as tracked in the todo list.
#[derive(Clone)]
struct Pick {
    sha: [u8; 20],
    subject: String,
}

/// The `--continue`/`--abort`/`--skip` state persisted under
/// `.git/rebase-merge/` (own format — D-019).
struct State {
    dir: PathBuf,
    head_name: String,
    onto: String,
    orig_head: String,
    msgnum: usize,
    todo: Vec<Pick>,
    message: String,
    author_script: String,
}

/// Run `git-rs rebase <upstream>` / `--continue` / `--abort` / `--skip`.
pub fn run_rebase(args: &[String]) -> Result<()> {
    let mut mode: Option<String> = None;
    let mut upstream: Option<String> = None;
    for a in args {
        match a.as_str() {
            "--continue" | "--abort" | "--skip" => {
                if mode.is_some() || upstream.is_some() {
                    return Err(GitError::Invalid("rebase: too many arguments".into()));
                }
                mode = Some(a.clone());
            }
            s if s.starts_with('-') => {
                return Err(GitError::Invalid(format!("rebase: unknown option '{s}'")));
            }
            s => {
                if mode.is_some() || upstream.is_some() {
                    return Err(GitError::Invalid("rebase: too many arguments".into()));
                }
                upstream = Some(s.to_string());
            }
        }
    }

    let refs = Refs::discover()?;
    let git_dir = abs_git_dir(refs.git_dir())?;
    let root = repo_root(&git_dir)?;
    let ipath = index_path(&git_dir);
    let store = ObjectStore::discover()?;
    let state_dir = git_dir.join("rebase-merge");

    if let Some(m) = mode {
        if !state_dir.exists() {
            return Err(GitError::Fatal("no rebase in progress".into()));
        }
        let state = load_state(&state_dir)?;
        return match m.as_str() {
            "--abort" => do_abort(&refs, &store, &root, &ipath, &state),
            "--skip" => do_skip(&refs, &store, &root, &ipath, &state),
            _ => do_continue(&refs, &store, &root, &ipath, &state),
        };
    }

    // A real git's (or our own) in-progress rebase: refuse with git's block
    // (probed byte-for-byte from a file capture — 341 bytes, including the
    // wrapped `the\ncase, please try` and `something\nvaluable there.` lines
    // and the trailing blank line).
    if state_dir.exists() {
        return Err(GitError::Fatal(
            "It seems that there is already a rebase-merge directory, and\n\
             I wonder if you are in the middle of another rebase.  If that is the\n\
             case, please try\n\
             \tgit rebase (--continue | --abort | --skip)\n\
             If that is not the case, please\n\
             \trm -fr \".git/rebase-merge\"\n\
             and run me again.  I am stopping in case you still have something\n\
             valuable there.\n"
                .into(),
        ));
    }

    // Detached HEAD: git supports it, v1 does not (D-019) — our own wording.
    let Some(branch) = refs.head_branch() else {
        return Err(GitError::Fatal(
            "rebase: detached HEAD is not supported in v1".into(),
        ));
    };
    let head = refs.resolve("HEAD")?.ok_or_else(|| unborn_fatal(&refs))?;
    let head_oid = parse_oid(&head)?;
    let upstream_str = upstream
        .clone()
        .ok_or_else(|| GitError::Invalid("rebase: no upstream given".into()))?;
    let upstream_oid = resolve_rev(&refs, &store, &upstream_str)?
        .ok_or_else(|| GitError::Fatal(format!("invalid upstream '{upstream_str}'")))?;

    let fork = merge_base(&store, head_oid, upstream_oid)?;

    // Up to date: the upstream tip is the (single) merge base AND head's
    // first-parent chain down to it is merge-free (git's
    // can_fast_forward + is_linear_history, rebase.c — a merge commit on
    // the chain replays instead, probed) -> git's message, no state, rc 0.
    if fork == Some(upstream_oid) && is_linear_to(&store, upstream_oid, head_oid)? {
        println!("Current branch {branch} is up to date.");
        return Ok(());
    }

    // The range: every commit reachable from HEAD but not from the fork, in
    // git's `rev-list --reverse --topo-order` order (probed); merge commits
    // are dropped from the replay (flattening) but still gate their parents.
    // An empty range (HEAD's commits all already upstream) fast-forwards
    // through the same start/finish machinery: git detaches at the upstream,
    // syncs the worktree, moves the branch ref to the upstream tip and
    // prints only the success line (probed).
    let picks = collect_range(&store, head_oid, fork)?;

    let onto_hex = hex(&upstream_oid);
    let head_tree = get_commit_tree(&store, head_oid)?;

    // Pre-rebase state (git's layout): ORIG_HEAD, then the state dir.
    write_state_file(&git_dir, "ORIG_HEAD", &format!("{head}\n"))?;
    fs::create_dir_all(&state_dir).context(&state_dir, "create rebase state")?;
    write_state_file(&state_dir, "head-name", &format!("refs/heads/{branch}\n"))?;
    write_state_file(&state_dir, "onto", &format!("{onto_hex}\n"))?;
    write_state_file(&state_dir, "orig-head", &format!("{head}\n"))?;
    write_state_file(&state_dir, "end", &format!("{}\n", picks.len()))?;
    write_todo(&state_dir, &picks)?;

    // Park HEAD at the onto commit (detached) and move worktree + index to
    // its tree — reset --hard semantics. Reflog: `rebase (start): checkout
    // <upstream>`.
    refs.set_head_sha(
        &onto_hex,
        &format!("rebase (start): checkout {upstream_str}"),
    )?;
    let mut idx = read_index(&ipath);
    let onto_tree = get_commit_tree(&store, upstream_oid)?;
    hard_sync(
        &store,
        &root,
        &mut idx,
        &ipath,
        Some(&head_tree),
        &onto_tree,
    )?;

    replay(
        &refs,
        &store,
        &root,
        &ipath,
        &state_dir,
        &branch,
        &onto_hex,
        upstream_oid,
        onto_tree,
        1,
        picks,
    )
}

/// Replay the remaining picks (todo order) onto the parked HEAD. `start` is
/// the 1-based msgnum of the first pick. Returns rc 1 on the first conflict
/// (everything already printed; state dir left in place).
#[allow(clippy::too_many_arguments)] // replay context bundle, fine at this size
fn replay(
    refs: &Refs,
    store: &ObjectStore,
    root: &Path,
    ipath: &Path,
    state_dir: &Path,
    branch: &str,
    onto_hex: &str,
    mut cur_head: [u8; 20],
    mut cur_head_tree: String,
    start: usize,
    mut remaining: Vec<Pick>,
) -> Result<()> {
    let end = remaining.len() + start - 1;
    let mut msgnum = start;
    let mut idx = read_index(ipath);
    while let Some(pick) = remaining.first().cloned() {
        remaining.remove(0);
        write_state_file(state_dir, "msgnum", &format!("{msgnum}\n"))?;
        // git's progress uses `\r` (overwrite-in-place), no newline —
        // byte-verified in captured stderr.
        eprint!("Rebasing ({msgnum}/{end})\r");

        let commit = load_commit(store, pick.sha)?;
        // Base tree: the pick's first parent, or the empty tree for a root
        // (rebase onto unrelated histories replays the whole branch).
        let base_tree = match commit.parents.first() {
            Some(p) => get_commit_tree(store, *p)?,
            None => tree_from_index(store, &[])?,
        };
        let theirs_tree = hex(&commit.tree);
        let label = format!("{} ({})", short_sha(&hex(&pick.sha)), pick.subject);
        let merged = merge_trees(store, &base_tree, &cur_head_tree, &theirs_tree)?;
        write_pick_state(state_dir, &commit, &merged)?;
        let ours = tree_entries(store, &cur_head_tree)?;
        let (new_idx, conflicted) = apply_merged_files(store, root, &idx, &ours, &merged, &label)?;
        new_idx.write(ipath)?;

        if conflicted {
            print_merged_lines(&merged.files, &label, false);
            eprintln!(
                "error: could not apply {}... {}\n\
                 hint: Resolve all conflicts manually, mark them as resolved with\n\
                 hint: \"git add/rm <conflicted_files>\", then run \"git rebase --continue\".\n\
                 hint: You can instead skip this commit: run \"git rebase --skip\".\n\
                 hint: To abort and get back to the state before \"git rebase\", run \"git rebase \
                 --abort\".\n\
                 hint: Disable this message with \"git config set advice.mergeConflict false\"\n\
                 Could not apply {}... # {}",
                short_sha(&hex(&pick.sha)),
                pick.subject,
                short_sha(&hex(&pick.sha)),
                pick.subject
            );
            return Err(GitError::Invalid(String::new()));
        }

        idx = new_idx;
        let tree = tree_from_index(store, idx.entries())?;
        advance_state(state_dir, msgnum + 1, &pick, &remaining)?;
        msgnum += 1;
        if tree == cur_head_tree {
            // Became empty: drop silently (git's patch-id skip ends the same
            // way; its `warning: skipped previously applied commit` hint is
            // not emitted in v1 — D-019).
            continue;
        }

        // Replay commit: the pick's author (name/email/date verbatim) and a
        // fresh committer; message preserved verbatim.
        let (_, committer) = commit_identities()?;
        let message = String::from_utf8_lossy(&commit.message).into_owned();
        let id = write_commit(
            store,
            &commit.author,
            &committer,
            &tree,
            vec![cur_head],
            &message,
        )?;
        refs.set_head_sha(&id, &format!("rebase (pick): {}", pick.subject))?;
        cur_head = parse_oid(&id)?;
        cur_head_tree = tree;
    }

    // Finish: move the branch to the new tip (branch reflog, probed wording)
    // and return HEAD to the symref; then drop the state and report success
    // (stderr, probed).
    refs.update(
        &format!("refs/heads/{branch}"),
        &hex(&cur_head),
        &format!("rebase (finish): refs/heads/{branch} onto {onto_hex}"),
    )?;
    refs.set_head_symref(
        branch,
        &format!("rebase (finish): returning to refs/heads/{branch}"),
    )?;
    let _ = fs::remove_dir_all(state_dir);
    eprintln!("Successfully rebased and updated refs/heads/{branch}.");
    Ok(())
}

/// `--continue`: commit the staged resolution as the current pick (original
/// author from the state, fresh committer, message from the state file with
/// `#` comments stripped), print git's commit-summary block, then replay the
/// rest.
fn do_continue(
    refs: &Refs,
    store: &ObjectStore,
    root: &Path,
    ipath: &Path,
    state: &State,
) -> Result<()> {
    let idx = read_index(ipath);

    // Unmerged index (probed block, on stdout, rc 1): one `path: needs
    // merge` line per unique conflict path.
    let mut unmerged: Vec<Vec<u8>> = Vec::new();
    for e in idx.entries() {
        if e.stage() != 0 && !unmerged.contains(&e.path) {
            unmerged.push(e.path.clone());
        }
    }
    if !unmerged.is_empty() {
        for path in &unmerged {
            println!("{}: needs merge", String::from_utf8_lossy(path));
        }
        println!("You must edit all merge conflicts and then");
        println!("mark them as resolved using git add");
        return Err(GitError::Invalid(String::new()));
    }

    let head = refs.resolve("HEAD")?.ok_or_else(|| unborn_fatal(refs))?;
    let head_oid = parse_oid(&head)?;
    let head_tree = get_commit_tree(store, head_oid)?;
    let tree = tree_from_index(store, idx.entries())?;
    let pick = state
        .todo
        .first()
        .ok_or_else(|| GitError::Fatal("rebase: no picks remain to continue".into()))?;
    let mut remaining = state.todo.clone();
    remaining.remove(0);

    if tree != head_tree {
        let author = parse_author_script(&state.author_script)?;
        let (_, committer) = commit_identities()?;
        let message = clean_state_message(&state.message);
        let id = write_commit(store, &author, &committer, &tree, vec![head_oid], &message)?;
        refs.set_head_sha(&id, &format!("rebase (continue): {}", pick.subject))?;
        // git's commit summary block (stdout) — the Author line appears when
        // the author identity differs from the committer's.
        println!("[detached HEAD {}] {}", short_sha(&id), pick.subject);
        if author != committer {
            println!(" Author: {} <{}>", author.name, author.email);
        }
        println!("{}", stat_summary(store, Some(&head_tree), &tree)?);
        replay(
            refs,
            store,
            root,
            ipath,
            &state.dir,
            &branch_of(&state.head_name),
            &state.onto,
            parse_oid(&id)?,
            tree,
            state.msgnum + 1,
            remaining,
        )
    } else {
        // Nothing staged on continue: drop the pick silently.
        advance_state(&state.dir, state.msgnum + 1, pick, &remaining)?;
        replay(
            refs,
            store,
            root,
            ipath,
            &state.dir,
            &branch_of(&state.head_name),
            &state.onto,
            head_oid,
            head_tree,
            state.msgnum + 1,
            remaining,
        )
    }
}

/// `--skip`: discard the current pick, reset the worktree/index back to the
/// parked HEAD (clearing the conflict state), and replay the rest. Silent.
fn do_skip(
    refs: &Refs,
    store: &ObjectStore,
    root: &Path,
    ipath: &Path,
    state: &State,
) -> Result<()> {
    let mut remaining = state.todo.clone();
    let pick = remaining
        .first()
        .ok_or_else(|| GitError::Fatal("rebase: no picks remain to skip".into()))?
        .clone();
    remaining.remove(0);
    advance_state(&state.dir, state.msgnum + 1, &pick, &remaining)?;

    let head = refs.resolve("HEAD")?.ok_or_else(|| unborn_fatal(refs))?;
    let head_oid = parse_oid(&head)?;
    let head_tree = get_commit_tree(store, head_oid)?;
    let mut idx = read_index(ipath);
    hard_sync(store, root, &mut idx, ipath, Some(&head_tree), &head_tree)?;
    replay(
        refs,
        store,
        root,
        ipath,
        &state.dir,
        &branch_of(&state.head_name),
        &state.onto,
        head_oid,
        head_tree,
        state.msgnum + 1,
        remaining,
    )
}

/// `--abort`: restore the branch ref to the pre-rebase commit (no branch
/// reflog — probed), reset worktree + index hard, point HEAD back at the
/// branch, and drop the state. Silent, rc 0.
fn do_abort(
    refs: &Refs,
    store: &ObjectStore,
    root: &Path,
    ipath: &Path,
    state: &State,
) -> Result<()> {
    refs.update_quiet(&state.head_name, &state.orig_head)?;
    let head = refs.resolve("HEAD")?.ok_or_else(|| unborn_fatal(refs))?;
    let cur_tree = get_commit_tree(store, parse_oid(&head)?)?;
    let orig_tree = get_commit_tree(store, parse_oid(&state.orig_head)?)?;
    let mut idx = read_index(ipath);
    hard_sync(store, root, &mut idx, ipath, Some(&cur_tree), &orig_tree)?;
    let branch = branch_of(&state.head_name);
    refs.set_head_symref(
        &branch,
        &format!("rebase (abort): returning to refs/heads/{branch}"),
    )?;
    fs::remove_dir_all(&state.dir).context(&state.dir, "remove rebase state")?;
    Ok(())
}

// --- state files ---------------------------------------------------------

fn load_state(dir: &Path) -> Result<State> {
    let read = |name: &str| -> Result<String> {
        let path = dir.join(name);
        fs::read_to_string(&path)
            .context(&path, "read rebase state")
            .map(|s| s.trim_end_matches('\n').to_string())
    };
    let todo_raw = read("git-rebase-todo")?;
    let mut todo = Vec::new();
    for line in todo_raw.lines() {
        if let Some((sha, subject)) = line
            .strip_prefix("pick ")
            .and_then(|rest| rest.split_once(" # "))
        {
            todo.push(Pick {
                sha: parse_oid(sha)?,
                subject: subject.to_string(),
            });
        }
    }
    Ok(State {
        dir: dir.to_path_buf(),
        head_name: read("head-name")?,
        onto: read("onto")?,
        orig_head: read("orig-head")?,
        msgnum: read("msgnum")?.parse().unwrap_or(1),
        todo,
        message: read("message")?,
        author_script: read("author-script")?,
    })
}

/// Write the remaining-todo file and msgnum after a pick completed.
fn advance_state(dir: &Path, msgnum: usize, pick: &Pick, remaining: &[Pick]) -> Result<()> {
    append_state_line(
        dir,
        "done",
        &format!("pick {} # {}\n", hex(&pick.sha), pick.subject),
    )?;
    write_todo(dir, remaining)?;
    write_state_file(dir, "msgnum", &format!("{msgnum}\n"))?;
    Ok(())
}

fn write_todo(dir: &Path, picks: &[Pick]) -> Result<()> {
    let mut todo = String::new();
    for p in picks {
        writeln!(&mut todo, "pick {} # {}", hex(&p.sha), p.subject).unwrap();
    }
    write_state_file(dir, "git-rebase-todo", &todo)
}

/// The message file (original message + git's `# Conflicts:` block when the
/// pick conflicts) and the author-script (git's exact format: single-quoted
/// with `'\''` escapes, date as `@<ts> <tz>`).
fn write_pick_state(dir: &Path, commit: &Commit, merged: &MergeResult) -> Result<()> {
    let mut message = String::from_utf8_lossy(&commit.message).into_owned();
    if merged
        .files
        .iter()
        .any(|f| matches!(f, MergeFile::Conflict(_)))
    {
        let mut block = String::new();
        for f in &merged.files {
            if let MergeFile::Conflict(c) = f {
                writeln!(&mut block, "#\t{}", String::from_utf8_lossy(&c.path)).unwrap();
            }
        }
        let base = message.trim_end_matches('\n');
        message = format!("{base}\n\n# Conflicts:\n{block}");
    }
    let a = &commit.author;
    let script = format!(
        "GIT_AUTHOR_NAME={}\nGIT_AUTHOR_EMAIL={}\nGIT_AUTHOR_DATE='@{} {}'\n",
        sq_quote(&a.name),
        sq_quote(&a.email),
        a.ts,
        format_tz(a.tz)
    );
    write_state_file(dir, "message", &message)?;
    write_state_file(dir, "author-script", &script)?;
    Ok(())
}

/// Parsed state author back into an `Ident` (name/email/date verbatim).
fn parse_author_script(script: &str) -> Result<Ident> {
    let mut name = String::new();
    let mut email = String::new();
    let mut ts = 0i64;
    let mut tz = 0i32;
    for line in script.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value
            .strip_prefix('\'')
            .and_then(|v| v.strip_suffix('\''))
            .unwrap_or(value)
            .replace("'\\''", "'");
        match key {
            "GIT_AUTHOR_NAME" => name = value,
            "GIT_AUTHOR_EMAIL" => email = value,
            "GIT_AUTHOR_DATE" => {
                let date = value.strip_prefix('@').unwrap_or(&value);
                if let Some((t, z)) = date.split_once(' ') {
                    ts = t.trim().parse().unwrap_or(0);
                    tz = parse_tz_value(z.trim());
                }
            }
            _ => {}
        }
    }
    Ident::new(name, email, ts, tz)
}

fn sq_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn format_tz(tz: i32) -> String {
    let sign = if tz < 0 { '-' } else { '+' };
    format!("{sign}{:04}", tz.abs())
}

fn parse_tz_value(s: &str) -> i32 {
    let (sign, digits) = match s.strip_prefix('-') {
        Some(d) => (-1, d),
        None => match s.strip_prefix('+') {
            Some(d) => (1, d),
            None => return 0,
        },
    };
    sign * digits.parse::<i32>().unwrap_or(0)
}

/// The state's message with `#` comment lines stripped and git's default
/// cleanup (same transform as MERGE_MSG on commit).
fn clean_state_message(raw: &str) -> String {
    crate::commands::commit::clean_message(&[strip_comment_lines(raw)])
}

fn branch_of(head_name: &str) -> String {
    head_name
        .strip_prefix("refs/heads/")
        .unwrap_or(head_name)
        .to_string()
}

// --- shared plumbing -----------------------------------------------------

/// Commits to replay: everything reachable from HEAD that is not reachable
/// from the fork point, in git's `rev-list --reverse --topo-order` order
/// (probed against git 2.55: the topo-indegree stack — seeds pushed in
/// date order, a commit's parents pushed in parent order, LIFO pops —
/// with the final list reversed). Merge commits gate their parents but are
/// dropped from the replay. Date ties break by descending sha (git keeps
/// its walk insertion order; D-019 — distinct dates make the case moot).
fn collect_range(store: &ObjectStore, head: [u8; 20], fork: Option<[u8; 20]>) -> Result<Vec<Pick>> {
    use std::collections::{HashMap, HashSet};
    let upstream = match fork {
        Some(f) => ancestors(store, f)?,
        None => HashSet::new(),
    };
    let own = ancestors(store, head)?;
    let mut in_range: Vec<(i64, [u8; 20])> = own
        .iter()
        .filter(|s| !upstream.contains(*s))
        .map(|s| {
            let c = load_commit(store, *s)?;
            Ok((c.committer.ts, *s))
        })
        .collect::<Result<_>>()?;
    in_range.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));

    let set: HashSet<[u8; 20]> = in_range.iter().map(|(_, s)| *s).collect();
    let mut indegree: HashMap<[u8; 20], usize> = HashMap::new();
    for (_, sha) in &in_range {
        let c = load_commit(store, *sha)?;
        for p in &c.parents {
            if set.contains(p) {
                *indegree.entry(*p).or_insert(0) += 1;
            }
        }
    }
    let mut stack: Vec<[u8; 20]> = in_range
        .iter()
        .filter(|(_, s)| indegree.get(s).copied().unwrap_or(0) == 0)
        .map(|(_, s)| *s)
        .collect();
    let mut topo = Vec::new();
    while let Some(sha) = stack.pop() {
        topo.push(sha);
        let c = load_commit(store, sha)?;
        for p in &c.parents {
            if !set.contains(p) {
                continue;
            }
            let d = indegree.get_mut(p).unwrap();
            *d -= 1;
            if *d == 0 {
                stack.push(*p);
            }
        }
    }

    let mut picks = Vec::new();
    for sha in topo.into_iter().rev() {
        let c = load_commit(store, sha)?;
        if c.parents.len() > 1 {
            continue; // merge commit: flatten (dropped, not replayed)
        }
        picks.push(Pick {
            sha,
            subject: subject_of(String::from_utf8_lossy(&c.message).as_ref()),
        });
    }
    Ok(picks)
}

/// True when the first-parent chain from `head` down to `fork` contains no
/// merge commit (git's `is_linear_history`; a merge on the chain blocks the
/// preemptive fast-forward even when the fork is the upstream tip).
fn is_linear_to(store: &ObjectStore, fork: [u8; 20], head: [u8; 20]) -> Result<bool> {
    let mut cur = Some(head);
    while let Some(sha) = cur {
        if sha == fork {
            return Ok(true);
        }
        let c = load_commit(store, sha)?;
        if c.parents.len() > 1 {
            return Ok(false);
        }
        cur = c.parents.first().copied();
    }
    Ok(true)
}

/// Every commit reachable from `tip` via parents (including `tip`).
fn ancestors(store: &ObjectStore, tip: [u8; 20]) -> Result<std::collections::HashSet<[u8; 20]>> {
    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![tip];
    while let Some(s) = stack.pop() {
        if !seen.insert(s) {
            continue;
        }
        stack.extend(load_commit(store, s)?.parents);
    }
    Ok(seen)
}

fn read_index(ipath: &Path) -> Index {
    if ipath.exists() {
        Index::read(ipath).unwrap_or_else(|_| Index::new())
    } else {
        Index::new()
    }
}

fn load_commit(store: &ObjectStore, sha: [u8; 20]) -> Result<Commit> {
    let (kind, content) = store.read_object(&hex(&sha))?;
    if kind != Kind::Commit {
        return Err(GitError::Corrupt(format!("{} is not a commit", hex(&sha))));
    }
    Commit::parse(&content)
}

fn get_commit_tree(store: &ObjectStore, sha: [u8; 20]) -> Result<String> {
    Ok(hex(&load_commit(store, sha)?.tree))
}

fn subject_of(message: &str) -> String {
    message.lines().next().unwrap_or("").to_string()
}

fn short_sha(sha: &str) -> String {
    if sha.len() >= 7 {
        sha[..7].to_string()
    } else {
        sha.to_string()
    }
}

fn write_state_file(dir: &Path, name: &str, content: &str) -> Result<()> {
    let path = dir.join(name);
    fs::write(&path, content).context(&path, "write rebase state")?;
    Ok(())
}

fn append_state_line(dir: &Path, name: &str, line: &str) -> Result<()> {
    let path = dir.join(name);
    let mut f = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .context(&path, "append rebase state")?;
    use std::io::Write;
    f.write_all(line.as_bytes())
        .context(&path, "append rebase state")?;
    Ok(())
}
