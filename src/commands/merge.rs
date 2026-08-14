//! `git-rs merge [--abort] [-q] <branch|tag|sha>` and `git-rs merge-base <rev1> <rev2>`.
//!
//! Byte-parity goals (probed against git 2.55): merge output lines
//! (`Auto-merging`, `CONFLICT (...)`) go to stdout; the success line is
//! `Merge made by the 'ort' strategy.` with reflog
//! `merge <label>: Merge made by the 'ort' strategy.`; conflict state is
//! MERGE_HEAD/MERGE_MSG/ORIG_HEAD files, stages 1/2/3 in the index, and
//! whole-file markers in the worktree; abort restores ORIG_HEAD hard.
//!
//! Locked deviations (D-018): every successful merge creates a merge commit
//! — no fast-forward and no `Already up to date.` handling. The merged
//! diffstat (` a.txt | 2 +-` block) is not printed. The strict v1 merge gate
//! refuses any index != HEAD (git's is per-path untracked/touched).

use std::fs;
use std::path::Path;

use crate::commands::commit::{commit_identities, tree_from_index, write_commit};
use crate::commands::reset::run_reset;
use crate::error::{GitError, IoContext, Result};
use crate::index::{Index, IndexEntry};
use crate::merge::{Conflict, ConflictKind, MergeFile, merge_trees};
use crate::object::Commit;
use crate::refs::Refs;
use crate::revwalk::{hex, merge_base, object_name_error, resolve_rev, unborn_fatal};
use crate::store::{Kind, ObjectStore};
use crate::worktree::{
    abs_git_dir, index_path, parse_oid, rel_os_path, repo_root, stat_file_or_zero, tree_entries,
};

/// Run `git-rs merge [--abort] [-q] <branch|tag|sha>`.
pub fn run_merge(args: &[String]) -> Result<()> {
    let mut abort = false;
    let mut quiet = false;
    let mut target: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--abort" => abort = true,
            "-q" | "--quiet" => quiet = true,
            s if s.starts_with('-') => {
                return Err(GitError::Invalid(format!("merge: unknown option '{s}'")));
            }
            s if target.is_none() => target = Some(s.to_string()),
            _ => return Err(GitError::Invalid("merge: too many arguments".into())),
        }
        i += 1;
    }
    if abort {
        return do_abort();
    }
    let target = target.ok_or_else(|| GitError::Invalid("merge: no target given".into()))?;
    do_merge(&target, quiet)
}

/// `git-rs merge-base <rev1> <rev2>`: print the best common ancestor.
pub fn run_merge_base(args: &[String]) -> Result<()> {
    if args.len() != 2 {
        return Err(GitError::Invalid(
            "merge-base: expected exactly two revisions".into(),
        ));
    }
    let refs = Refs::discover()?;
    let store = ObjectStore::discover()?;
    let mut revs = Vec::new();
    for rev in args {
        let sha = resolve_rev(&refs, &store, rev)?
            .ok_or_else(|| GitError::NotFound(format!("Not a valid object name {rev}")))?;
        revs.push(sha);
    }
    match merge_base(&store, revs[0], revs[1])? {
        Some(sha) => {
            println!("{}", hex(&sha));
            Ok(())
        }
        None => Err(GitError::Fatal("no merge base found".into())),
    }
}

/// Merge target `target` into HEAD.
fn do_merge(target: &str, quiet: bool) -> Result<()> {
    let refs = Refs::discover()?;
    let git_dir = abs_git_dir(refs.git_dir())?;
    let root = repo_root(&git_dir)?;
    let ipath = index_path(&git_dir);
    let idx = if ipath.exists() {
        Index::read(&ipath)?
    } else {
        Index::new()
    };
    let store = ObjectStore::discover()?;

    let head = refs.resolve("HEAD")?.ok_or_else(|| unborn_fatal(&refs))?;
    let head_oid = parse_oid(&head)?;

    // In-progress state: an unmerged index (checked first, like git) or an
    // existing MERGE_HEAD.
    if idx.entries().iter().any(|e| e.stage() != 0) {
        eprintln!(
            "error: Merging is not possible because you have unmerged files.\n\
             hint: Fix them up in the work tree, and then use 'git add/rm <file>'\n\
             hint: as appropriate to mark resolution and make a commit.\n\
             fatal: Exiting because of an unresolved conflict."
        );
        return Err(GitError::Fatal(String::new()));
    }
    if git_dir.join("MERGE_HEAD").exists() {
        return Err(GitError::Fatal(
            "You have not concluded your merge (MERGE_HEAD exists).\n\
             Please commit your changes before you merge."
                .into(),
        ));
    }

    let target_sha =
        resolve_rev(&refs, &store, target)?.ok_or_else(|| object_name_error(target))?;
    let kind = target_kind(&refs, target);
    let label = match kind {
        TargetKind::Commit => short_sha(&hex(&target_sha)),
        _ => target.to_string(),
    };
    let head_tree = get_commit_tree(&store, head_oid)?;
    let target_tree = get_commit_tree(&store, target_sha)?;

    let base_sha = merge_base(&store, head_oid, target_sha)?
        .ok_or_else(|| GitError::Fatal("refusing to merge unrelated histories".into()))?;
    let base_tree = get_commit_tree(&store, base_sha)?;

    // Strict gate (locked): index must equal HEAD's tree.
    if tree_from_index(&store, idx.entries())? != head_tree {
        return Err(dirty_merge_error(&store, idx.entries(), &head_tree)?);
    }

    let old = tree_entries(&store, &head_tree)?;
    let merged = merge_trees(&store, &base_tree, &head_tree, &target_tree)?;
    // The merged index keeps the untouched entries (a file unchanged on both
    // sides stays), with every touched path replaced by its merged/staged
    // form. Path order matches git's sorted index at the end.
    let mut new_idx = Index::new();
    for e in idx.entries() {
        if e.stage() == 0 {
            new_idx.stage(e.clone());
        }
    }
    let mut conflicted = false;

    // Worktree first (files before index, so stat fields match), then
    // collect the merged index. Path order = git's processing order.
    for f in &merged.files {
        match f {
            MergeFile::Resolved {
                path, mode, oid, ..
            } => {
                new_idx.unstage(path);
                match oid {
                    Some(o) => {
                        let in_old = old
                            .iter()
                            .any(|(p, m, ob)| p == path && m == mode && ob == o);
                        if !in_old {
                            crate::worktree::write_blob(&store, &root, path, *mode, o)?;
                        }
                        let st = stat_file_or_zero(&root.join(rel_os_path(path)));
                        new_idx.stage(entry(&st, *mode, *o, 0, path));
                    }
                    None => {
                        crate::worktree::remove_file_and_empty_dirs(&root.join(rel_os_path(path)));
                    }
                }
            }
            MergeFile::Conflict(c) => {
                conflicted = true;
                new_idx.unstage(&c.path);
                if c.kind != ConflictKind::ModifyDelete {
                    write_marker(&store, &root, c, &label)?;
                } else if c.ours.is_none() {
                    // We deleted it; git leaves the modified side's version
                    // in the tree.
                    let (mode, oid) = c.theirs.unwrap();
                    crate::worktree::write_blob(&store, &root, &c.path, mode, &oid)?;
                }
                for (stage, side) in [(1u16, &c.base), (2, &c.ours), (3, &c.theirs)] {
                    if let Some((mode, oid)) = side {
                        new_idx.stage(entry(&zero_stat(), *mode, *oid, stage, &c.path));
                    }
                }
            }
        }
    }
    new_idx.entries_mut().sort_by(|a, b| a.path.cmp(&b.path));

    write_state_file(&git_dir, "ORIG_HEAD", &format!("{}\n", hex(&head_oid)))?;
    write_state_file(&git_dir, "MERGE_HEAD", &format!("{}\n", hex(&target_sha)))?;
    write_state_file(
        &git_dir,
        "MERGE_MSG",
        &merge_commit_message(&kind, &label, target_sha, &merged.files),
    )?;
    new_idx.write(&ipath)?;

    // Output, in path order.
    for f in &merged.files {
        match f {
            MergeFile::Resolved {
                path, auto: true, ..
            } => {
                if !quiet {
                    println!("Auto-merging {}", display(path));
                }
            }
            MergeFile::Conflict(c) => match c.kind {
                ConflictKind::Content | ConflictKind::AddAdd => {
                    if !quiet {
                        println!("Auto-merging {}", display(&c.path));
                        println!(
                            "CONFLICT ({}): Merge conflict in {}",
                            conflict_word(c.kind),
                            display(&c.path)
                        );
                    }
                }
                ConflictKind::ModifyDelete => {
                    let (deleted, modified) = if c.ours.is_some() {
                        (label.as_str(), "HEAD")
                    } else {
                        ("HEAD", label.as_str())
                    };
                    if !quiet {
                        println!(
                            "CONFLICT (modify/delete): {} deleted in {deleted} and modified in \
                             {modified}.  Version {modified} of {} left in tree.",
                            display(&c.path),
                            display(&c.path)
                        );
                    }
                }
            },
            MergeFile::Resolved { .. } => {}
        }
    }

    if !conflicted {
        let tree = tree_from_index(&store, new_idx.entries())?;
        let msg = merge_commit_message(&kind, &label, target_sha, &merged.files);
        let (author, committer) = commit_identities()?;
        let id = write_commit(
            &store,
            &author,
            &committer,
            &tree,
            vec![head_oid, target_sha],
            &msg,
        )?;
        refs.update(
            "HEAD",
            &id,
            &format!("merge {label}: Merge made by the 'ort' strategy."),
        )?;
        remove_state_file(&git_dir, "MERGE_HEAD");
        remove_state_file(&git_dir, "MERGE_MSG");
        if !quiet {
            println!("Merge made by the 'ort' strategy.");
        }
        Ok(())
    } else {
        println!("Automatic merge failed; fix conflicts and then commit the result.");
        // rc 1; everything else already printed.
        Err(GitError::Invalid(String::new()))
    }
}

/// `git-rs merge --abort`: restore ORIG_HEAD (hard) and drop merge state.
fn do_abort() -> Result<()> {
    let refs = Refs::discover()?;
    let git_dir = abs_git_dir(refs.git_dir())?;
    let merge_head = git_dir.join("MERGE_HEAD");
    if !merge_head.exists() {
        return Err(GitError::Fatal(
            "There is no merge to abort (MERGE_HEAD missing).".into(),
        ));
    }
    let orig_path = git_dir.join("ORIG_HEAD");
    let orig = fs::read_to_string(&orig_path).context(&orig_path, "read ORIG_HEAD")?;
    let hard: Vec<String> = vec!["--hard".into(), "-q".into(), orig.trim().to_string()];
    run_reset(&hard)?;
    remove_state_file(&git_dir, "MERGE_HEAD");
    remove_state_file(&git_dir, "MERGE_MSG");
    Ok(())
}

/// What the target argument named (drives default messages and labels).
#[derive(Clone, Copy, PartialEq, Eq)]
enum TargetKind {
    Branch,
    Tag,
    Commit,
}

fn target_kind(refs: &Refs, target: &str) -> TargetKind {
    let branch = refs.resolve(&format!("refs/heads/{target}")).ok().flatten();
    if branch.is_some() {
        return TargetKind::Branch;
    }
    let tag = refs.resolve(&format!("refs/tags/{target}")).ok().flatten();
    if tag.is_some() {
        return TargetKind::Tag;
    }
    TargetKind::Commit
}

/// The default merge commit message (and MERGE_MSG body): `Merge branch
/// '<x>'` / `Merge tag '<x>'` / `Merge commit '<short-sha>'` (v1 uses a
/// fixed 7-char abbreviation; git auto-abbreviates to the shortest unique
/// form — D-019). A conflicts block is appended when merging is
/// incomplete, like git's.
fn merge_commit_message(
    kind: &TargetKind,
    label: &str,
    target_sha: [u8; 20],
    files: &[MergeFile],
) -> String {
    let msg = match kind {
        TargetKind::Branch => format!("Merge branch '{label}'"),
        TargetKind::Tag => format!("Merge tag '{label}'"),
        TargetKind::Commit => format!("Merge commit '{}'", short_sha(&hex(&target_sha))),
    };
    let mut out = format!("{msg}\n");
    if files.iter().any(|f| matches!(f, MergeFile::Conflict(_))) {
        out.push('\n');
        out.push_str("# Conflicts:\n");
        for f in files.iter().filter_map(|f| match f {
            MergeFile::Conflict(c) => Some(display(&c.path)),
            _ => None,
        }) {
            out.push_str(&format!("#\t{f}\n"));
        }
    }
    out
}

/// The merged file's conflict marker for a content/add-add conflict, with
/// ours = `<<<<<<< HEAD` and theirs = the target label (probed bytes).
fn write_marker(store: &ObjectStore, root: &Path, c: &Conflict, label: &str) -> Result<()> {
    let ours = read_blob(store, &c.ours.unwrap().1)?;
    let theirs = read_blob(store, &c.theirs.unwrap().1)?;
    let marker = crate::merge::conflict_marker(&ours, &theirs, label);
    let abs = root.join(rel_os_path(&c.path));
    if let Some(dir) = abs.parent() {
        fs::create_dir_all(dir).context(dir, "create directory")?;
    }
    fs::write(&abs, &marker).context(&abs, "write conflict marker")?;
    Ok(())
}

fn read_blob(store: &ObjectStore, oid: &[u8; 20]) -> Result<Vec<u8>> {
    let (kind, content) = store.read_object(&hex(oid))?;
    if kind != Kind::Blob {
        return Err(GitError::Corrupt(format!("'{}' is not a blob", hex(oid))));
    }
    Ok(content)
}

/// The strict-merge dirty refusal (probed wording, git 2.55 — a genuine
/// diverged merge fails at the strategy with this block on stderr, exit 2;
/// ff/recursive paths word it differently).
fn dirty_merge_error(
    store: &ObjectStore,
    entries: &[IndexEntry],
    head_tree: &str,
) -> Result<GitError> {
    let head = tree_entries(store, head_tree)?;
    let mut diffs = Vec::new();
    for e in entries.iter().filter(|e| e.stage() == 0) {
        if !head.iter().any(|(p, _, o)| p == &e.path && *o == e.oid) {
            diffs.push(display(&e.path));
        }
    }
    let mut msg = String::from(
        "error: Your local changes to the following files would be overwritten by merge:\n",
    );
    for d in &diffs {
        msg.push_str(&format!("  {d}\n"));
    }
    msg.push_str("Merge with strategy ort failed.");
    Ok(GitError::Failure(msg))
}

/// A merged-index entry; stat fields from the freshly-written worktree copy.
fn entry(
    st: &crate::worktree::WorkStat,
    mode: u32,
    oid: [u8; 20],
    stage: u16,
    path: &[u8],
) -> IndexEntry {
    IndexEntry {
        ctime_sec: st.ctime_sec,
        ctime_nsec: st.ctime_nsec,
        mtime_sec: st.mtime_sec,
        mtime_nsec: st.mtime_nsec,
        dev: 0,
        ino: 0,
        mode,
        uid: 0,
        gid: 0,
        size: st.size,
        oid,
        flags: stage << 12, // bits 12-13, stage 0 = normal
        extended_flags: 0,
        path: path.to_vec(),
    }
}

fn zero_stat() -> crate::worktree::WorkStat {
    crate::worktree::WorkStat {
        ctime_sec: 0,
        ctime_nsec: 0,
        mtime_sec: 0,
        mtime_nsec: 0,
        mode: 0,
        size: 0,
    }
}

fn get_commit_tree(store: &ObjectStore, sha: [u8; 20]) -> Result<String> {
    let (kind, content) = store.read_object(&hex(&sha))?;
    if kind != Kind::Commit {
        return Err(GitError::Corrupt("not a commit".into()));
    }
    let commit = Commit::parse(&content)?;
    Ok(hex(&commit.tree))
}

fn conflict_word(kind: ConflictKind) -> &'static str {
    match kind {
        ConflictKind::Content => "content",
        ConflictKind::AddAdd => "add/add",
        ConflictKind::ModifyDelete => "modify/delete",
    }
}

fn write_state_file(git_dir: &Path, name: &str, content: &str) -> Result<()> {
    let path = git_dir.join(name);
    fs::write(&path, content).context(&path, "write merge state")?;
    Ok(())
}

fn remove_state_file(git_dir: &Path, name: &str) {
    let _ = fs::remove_file(git_dir.join(name));
}

fn display(path: &[u8]) -> String {
    String::from_utf8_lossy(path).into_owned()
}

fn short_sha(sha: &str) -> String {
    if sha.len() >= 7 {
        sha[..7].to_string()
    } else {
        sha.to_string()
    }
}
