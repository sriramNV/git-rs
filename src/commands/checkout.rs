use std::path::Path;

use crate::error::{GitError, Result};
use crate::index::Index;
use crate::object::Commit;
use crate::refs::Refs;
use crate::revwalk::{object_name_error, peel_to_commit, resolve_rev};
use crate::store::{Kind, ObjectStore};
use crate::worktree::{abs_git_dir, index_path, repo_root, sync_worktree, tree_entries};

/// Run `git checkout [-b <name>] [-f] [-q] <branch|tag|sha>`.
pub fn run_checkout(args: &[String]) -> Result<()> {
    let mut create_branch: Option<String> = None;
    let mut force = false;
    let mut quiet = false;
    let mut target: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-b" => {
                i += 1;
                let Some(name) = args.get(i) else {
                    return Err(GitError::Invalid(
                        "checkout: option '-b' requires a branch name".into(),
                    ));
                };
                create_branch = Some(name.clone());
            }
            "-f" | "--force" => force = true,
            "-q" | "--quiet" => quiet = true,
            s if s.starts_with('-') => {
                return Err(GitError::Invalid(format!("checkout: unknown option '{s}'")));
            }
            s => {
                if target.is_none() {
                    target = Some(s.to_string());
                } else {
                    return Err(GitError::Invalid("checkout: too many arguments".into()));
                }
            }
        }
        i += 1;
    }

    let refs = Refs::discover()?;
    let git_dir = abs_git_dir(refs.git_dir())?;
    let root = repo_root(&git_dir)?;
    let ipath = index_path(&git_dir);
    let mut idx = if ipath.exists() {
        Index::read(&ipath)?
    } else {
        Index::new()
    };
    let store = ObjectStore::discover()?;

    // Need a target; `checkout -b <name>` starts from HEAD.
    let target = match target {
        Some(t) => t,
        None if create_branch.is_some() => "HEAD".to_string(),
        None => return Err(GitError::Invalid("checkout: no target given".into())),
    };

    // Resolve current HEAD commit/tree
    let head = refs.resolve("HEAD")?;
    let (old_tree, old_commit_sha) = match head {
        Some(h) => {
            let commit_sha = parse_oid(&h)?;
            (Some(get_commit_tree(&store, commit_sha)?), Some(commit_sha))
        }
        None => (None, None),
    };

    // -b: create new branch at target, then switch to it
    if let Some(branch_name) = create_branch {
        Refs::validate_name(&format!("refs/heads/{branch_name}"))?;
        if refs
            .resolve(&format!("refs/heads/{branch_name}"))?
            .is_some()
        {
            return Err(GitError::Fatal(format!(
                "a branch named '{branch_name}' already exists"
            )));
        }
        let target_sha =
            resolve_rev(&refs, &store, &target)?.ok_or_else(|| object_name_error(&target))?;
        let target_tree = get_commit_tree(&store, target_sha)?;
        let sha_hex = hex(&target_sha);
        refs.update(
            &format!("refs/heads/{branch_name}"),
            &sha_hex,
            "branch: Created from HEAD",
        )?;
        checkout_branch(
            &refs,
            &store,
            &root,
            &git_dir,
            &mut idx,
            &ipath,
            &branch_name,
            old_tree,
            target_tree,
            old_commit_sha,
            force,
            quiet,
            true,
        )?;
        return Ok(());
    }

    // Try as branch name first
    if let Some(sha) = refs.resolve(&format!("refs/heads/{target}"))? {
        let target_tree = get_commit_tree(&store, parse_oid(&sha)?)?;
        checkout_branch(
            &refs,
            &store,
            &root,
            &git_dir,
            &mut idx,
            &ipath,
            &target,
            old_tree,
            target_tree,
            old_commit_sha,
            force,
            quiet,
            false,
        )?;
        return Ok(());
    }

    // Try as tag (peels to commit)
    if let Some(sha) = refs.resolve(&format!("refs/tags/{target}"))? {
        if let Some(commit_sha) = peel_to_commit(&store, &sha)? {
            let target_tree = get_commit_tree(&store, commit_sha)?;
            checkout_detached(
                &refs,
                &store,
                &root,
                &git_dir,
                &mut idx,
                &ipath,
                commit_sha,
                old_tree,
                target_tree,
                old_commit_sha,
                force,
                quiet,
            )?;
            return Ok(());
        }
        return Err(GitError::Fatal(format!(
            "tag '{target}' does not point to a commit"
        )));
    }

    // Try as a revision-like name (HEAD, `<rev>~N`, 40-hex sha)
    if let Some(target_sha) = resolve_rev(&refs, &store, &target)? {
        let target_tree = get_commit_tree(&store, target_sha)?;
        checkout_detached(
            &refs,
            &store,
            &root,
            &git_dir,
            &mut idx,
            &ipath,
            target_sha,
            old_tree,
            target_tree,
            old_commit_sha,
            force,
            quiet,
        )?;
        return Ok(());
    }

    // Unknown target (probed: `error:` + rc 1, not fatal)
    Err(GitError::Invalid(format!(
        "error: pathspec '{target}' did not match any file(s) known to git"
    )))
}

#[allow(clippy::too_many_arguments)] // checkout context bundle, fine at this size
fn checkout_branch(
    refs: &Refs,
    store: &ObjectStore,
    root: &Path,
    git_dir: &Path,
    idx: &mut Index,
    ipath: &Path,
    branch: &str,
    old_tree: Option<String>,
    new_tree: String,
    old_commit_sha: Option<[u8; 20]>,
    force: bool,
    quiet: bool,
    is_new: bool,
) -> Result<()> {
    // Already on this branch: git never materializes or gates (probed), it
    // just prints `Already on '<branch>'` plus the post-checkout status.
    let was_on = refs.head_branch();
    if was_on.as_deref() == Some(branch) {
        if !quiet {
            eprintln!("Already on '{branch}'");
        }
        print_post_checkout_status(root, git_dir, store, idx)?;
        return Ok(());
    }

    // Clean-index gate (locked v1: index must match HEAD tree unless -f)
    if !force && !index_is_clean(store, idx, refs)? {
        return Err(dirty_index_error(store, idx, refs)?);
    }

    // Materialize (−f also discards local edits)
    if force {
        crate::worktree::force_sync_worktree(store, root, old_tree.as_deref(), &new_tree)?;
    } else {
        sync_worktree(store, root, old_tree.as_deref(), &new_tree)?;
    }

    // Rewrite index to new tree
    rewrite_index(store, root, idx, &new_tree)?;
    idx.write(ipath)?;

    // Move HEAD to branch
    let old_descr = old_head_descr(refs, old_commit_sha)?;
    refs.set_head_symref(
        branch,
        &format!("checkout: moving from {old_descr} to {branch}"),
    )?;

    // Output
    if !quiet {
        if is_new {
            eprintln!("Switched to a new branch '{branch}'");
        } else {
            eprintln!("Switched to branch '{branch}'");
        }
    }

    // Post-checkout status (stdout)
    print_post_checkout_status(root, git_dir, store, idx)?;

    Ok(())
}

#[allow(clippy::too_many_arguments)] // checkout context bundle, fine at this size
fn checkout_detached(
    refs: &Refs,
    store: &ObjectStore,
    root: &Path,
    git_dir: &Path,
    idx: &mut Index,
    ipath: &Path,
    commit_sha: [u8; 20],
    old_tree: Option<String>,
    new_tree: String,
    old_commit_sha: Option<[u8; 20]>,
    force: bool,
    quiet: bool,
) -> Result<()> {
    // Clean-index gate
    if !force && !index_is_clean(store, idx, refs)? {
        return Err(dirty_index_error(store, idx, refs)?);
    }

    // Materialize (−f also discards local edits)
    if force {
        crate::worktree::force_sync_worktree(store, root, old_tree.as_deref(), &new_tree)?;
    } else {
        sync_worktree(store, root, old_tree.as_deref(), &new_tree)?;
    }

    // Rewrite index
    rewrite_index(store, root, idx, &new_tree)?;
    idx.write(ipath)?;

    // Move HEAD to detached sha (writes the raw sha into the HEAD file —
    // never the branch ref).
    let sha_hex = hex(&commit_sha);
    let old_descr = old_head_descr(refs, old_commit_sha)?;
    refs.set_head_sha(
        &sha_hex,
        &format!("checkout: moving from {old_descr} to {sha_hex}"),
    )?;

    // Output
    if !quiet {
        let old_short = old_commit_sha
            .map(|s| short_sha(&hex(&s)))
            .unwrap_or_default();
        let old_subj = match old_commit_sha {
            Some(s) => commit_subject(store, s)?,
            None => String::new(),
        };
        eprintln!("Previous HEAD position was {old_short} {old_subj}");
        let new_short = short_sha(&sha_hex);
        let new_subj = commit_subject(store, commit_sha)?;
        eprintln!("HEAD is now at {new_short} {new_subj}");
    }

    // Post-checkout status
    print_post_checkout_status(root, git_dir, store, idx)?;

    Ok(())
}

/// Reflog destination for `checkout: moving from ...`: the branch name when
/// on a branch, else the full sha (detached), else `unborn`.
fn old_head_descr(refs: &Refs, old_commit_sha: Option<[u8; 20]>) -> Result<String> {
    if let Some(branch) = refs.head_branch() {
        Ok(branch)
    } else if let Some(sha) = old_commit_sha {
        Ok(hex(&sha))
    } else {
        Ok("unborn".to_string())
    }
}

fn short_sha(sha: &str) -> String {
    if sha.len() >= 7 {
        sha[..7].to_string()
    } else {
        sha.to_string()
    }
}

fn commit_subject(store: &ObjectStore, sha: [u8; 20]) -> Result<String> {
    let (kind, content) = store.read_object(&hex(&sha))?;
    if kind != Kind::Commit {
        return Ok("".to_string());
    }
    let commit = Commit::parse(&content)?;
    let msg = String::from_utf8_lossy(&commit.message);
    Ok(msg.lines().next().unwrap_or("").to_string())
}

fn index_is_clean(store: &ObjectStore, idx: &Index, refs: &Refs) -> Result<bool> {
    let idx_tree = crate::commands::commit::tree_from_index(store, idx.entries())?;
    let head_tree = match refs.resolve("HEAD")? {
        Some(sha) => get_commit_tree(store, parse_oid(&sha)?)?,
        None => return Ok(true), // unborn HEAD, index empty = clean
    };
    Ok(idx_tree == head_tree)
}

fn get_commit_tree(store: &ObjectStore, sha: [u8; 20]) -> Result<String> {
    let (kind, content) = store.read_object(&hex(&sha))?;
    if kind != Kind::Commit {
        return Err(GitError::Corrupt("not a commit".into()));
    }
    let commit = Commit::parse(&content)?;
    Ok(hex(&commit.tree))
}

fn rewrite_index(store: &ObjectStore, root: &Path, idx: &mut Index, tree: &str) -> Result<()> {
    let entries = tree_entries(store, tree)?;
    idx.entries_mut().clear();
    for (path, mode, oid) in entries {
        let abs = root.join(crate::worktree::rel_os_path(&path));
        let st = crate::worktree::stat_file_or_zero(&abs);
        idx.stage(crate::index::IndexEntry {
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
            flags: 0,
            extended_flags: 0,
            path,
        });
    }
    Ok(())
}

fn dirty_index_error(store: &ObjectStore, idx: &Index, refs: &Refs) -> Result<GitError> {
    let head_entries = match refs.resolve("HEAD")? {
        Some(h) => tree_entries(store, &get_commit_tree(store, parse_oid(&h)?)?)?,
        None => Vec::new(), // unborn: everything staged is a local change
    };
    let idx_tree = crate::commands::commit::tree_from_index(store, idx.entries())?;
    let idx_entries = tree_entries(store, &idx_tree)?;
    let mut diffs = Vec::new();
    for (path, _, oid) in &idx_entries {
        if !head_entries.iter().any(|(p, _, o)| p == path && o == oid) {
            diffs.push(String::from_utf8_lossy(path).to_string());
        }
    }
    let mut msg = String::from(
        "error: Your local changes to the following files would be overwritten by checkout:\n",
    );
    for d in &diffs {
        msg.push_str(&format!("\t{d}\n"));
    }
    msg.push_str("Please commit your changes or stash them before you switch branches.\nAborting");
    Ok(GitError::Invalid(msg))
}

fn print_post_checkout_status(
    root: &Path,
    git_dir: &Path,
    store: &ObjectStore,
    idx: &Index,
) -> Result<()> {
    let matcher = crate::ignore::IgnoreMatcher::load(root, git_dir)?;
    let items = crate::worktree::walk_worktree(root, git_dir, &matcher)?;
    let wt_files: std::collections::HashSet<&[u8]> = items
        .iter()
        .filter(|i| !i.is_dir)
        .map(|i| i.path.as_slice())
        .collect();

    let mut out = Vec::new();
    for e in idx.entries().iter().filter(|e| e.stage() == 0) {
        let y = if wt_files.contains(&e.path.as_slice()) {
            let abs = root.join(crate::worktree::rel_os_path(&e.path));
            match crate::worktree::hash_entry(store, &abs, false) {
                Ok(h) if h == e.oid => b' ',
                _ => b'M',
            }
        } else {
            b'D'
        };
        if y != b' ' {
            let p = String::from_utf8_lossy(&e.path);
            out.push(format!("M\t{p}"));
        }
    }
    if !out.is_empty() {
        for line in out {
            println!("{line}");
        }
    }
    Ok(())
}

fn hex(oid: &[u8; 20]) -> String {
    oid.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_oid(sha: &str) -> Result<[u8; 20]> {
    let mut oid = [0u8; 20];
    for i in 0..20 {
        oid[i] = u8::from_str_radix(&sha[2 * i..2 * i + 2], 16)
            .map_err(|_| GitError::Corrupt(format!("bad sha '{sha}'")))?;
    }
    Ok(oid)
}
