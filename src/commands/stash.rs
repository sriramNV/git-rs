//! `git-rs stash` and `git-rs stash (list|pop|drop)`.
//!
//! Parent structure (probed against git 2.55):
//! - `stash@{0}` = worktree commit (WIP on <branch>: <head-sha> <subject>)
//! - Its parent = index commit (tree = current index, parent = HEAD)
//! - Index commit's parent = HEAD
//!
//! This exact structure is what makes `git stash list` / `git stash show -p`
//! work on our stashes.

use std::path::Path;

use crate::commands::commit::{commit_identities, tree_from_index, write_commit};
use crate::commands::merge::apply_merged_files;
use crate::error::{GitError, Result};
use crate::index::{Index, IndexEntry};
use crate::merge::merge_trees;
use crate::object::Commit;
use crate::refs::Refs;
use crate::revwalk::{hex, unborn_fatal};
use crate::store::{Kind, ObjectStore};
use crate::worktree::{abs_git_dir, index_path, parse_oid, rel_os_path, repo_root, tree_entries};

/// Run `git-rs stash` / `list` / `pop` / `drop`.
pub fn run_stash(args: &[String]) -> Result<()> {
    if args.is_empty() {
        return do_save();
    }
    match args[0].as_str() {
        "list" => do_list(),
        "pop" => do_pop(args.get(1).map(|s| s.as_str())),
        "drop" => do_drop(args.get(1).map(|s| s.as_str())),
        s => Err(GitError::Invalid(format!("stash: unknown subcommand '{s}'"))),
    }
}

/// `git-rs stash`: save the current index and worktree state as a stash commit.
fn do_save() -> Result<()> {
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

    // Must be on a branch (unborn or not) — HEAD symref required.
    let branch = refs.head_branch().ok_or_else(|| {
        GitError::Fatal("stash: cannot stash on detached HEAD".into())
    })?;

    let head = refs.resolve("HEAD")?.ok_or_else(|| unborn_fatal(&refs))?;
    let head_oid = parse_oid(&head)?;

    // Build the index tree from the current index (staged changes).
    let index_tree = tree_from_index(&store, idx.entries())?;

    // Build the worktree tree: for tracked files, use worktree content;
    // for untracked, ignore in v1 (no -u).
    let worktree_tree = build_worktree_tree(&store, &root, &idx, &ipath)?;

    // 1. Create the index commit: tree = index_tree, parent = HEAD.
    let (author, committer) = commit_identities()?;
    let index_msg = format!(
        "index on {}: {}",
        branch,
        short_subject(&refs, head_oid)?
    );
    let index_commit = write_commit(
        &store,
        &author,
        &committer,
        &index_tree,
        vec![head_oid],
        &index_msg,
    )?;

    // 2. Create the worktree commit: tree = worktree_tree, parent = index_commit.
    let worktree_msg = format!(
        "WIP on {}: {}",
        branch,
        short_subject(&refs, head_oid)?
    );
    let worktree_commit = write_commit(
        &store,
        &author,
        &committer,
        &worktree_tree,
        vec![parse_oid(&index_commit)?],
        &worktree_msg,
    )?;

    // 3. Update refs/stash to point to the worktree commit.
    let stash_ref = "refs/stash";
    refs.update(
        stash_ref,
        &worktree_commit,
        &format!("stash: {worktree_msg}"),
    )?;

    // 4. Reset worktree to HEAD (clean), but keep the index as-is.
    let head_tree = get_commit_tree(&store, head_oid)?;
    let idx = if ipath.exists() {
        Index::read(&ipath)?
    } else {
        Index::new()
    };
    crate::worktree::sync_worktree(&store, &root, Some(&head_tree), &head_tree)?;
    // Index stays at the pre-stash state (staged changes remain staged).
    idx.write(&ipath)?;

    println!("Saved working directory and index state {worktree_msg}");
    Ok(())
}

/// Build the worktree tree from the actual worktree files.
fn build_worktree_tree(
    store: &ObjectStore,
    root: &Path,
    idx: &Index,
    _ipath: &Path,
) -> Result<String> {
    // Walk the worktree to get all tracked files' current content.
    // For files in the index, hash the worktree version (or keep the
    // index version if unchanged). For untracked files, we ignore them in v1.
    let mut entries = Vec::new();
    for e in idx.entries().iter().filter(|e| e.stage() == 0) {
        let abs = root.join(rel_os_path(&e.path));
        match crate::worktree::hash_entry(store, &abs, true) {
            Ok(oid) => entries.push(IndexEntry {
                ctime_sec: 0,
                ctime_nsec: 0,
                mtime_sec: 0,
                mtime_nsec: 0,
                dev: 0,
                ino: 0,
                mode: e.mode,
                uid: 0,
                gid: 0,
                size: 0,
                oid,
                flags: 0,
                extended_flags: 0,
                path: e.path.clone(),
            }),
            Err(_) => {
                // File missing in worktree — use the indexed version.
                entries.push(e.clone());
            }
        }
    }
    tree_from_index(store, &entries)
}

/// `git-rs stash list`: show the stash stack.
fn do_list() -> Result<()> {
    let refs = Refs::discover()?;
    let stash_ref = refs.resolve("refs/stash")?;
    if stash_ref.is_none() {
        return Ok(()); // no stashes
    }
    let mut sha = stash_ref.unwrap();
    loop {
        let (kind, content) = ObjectStore::discover()?.read_object(&sha)?;
        if kind != Kind::Commit {
            break;
        }
        let commit = Commit::parse(&content)?;
        let subject = String::from_utf8_lossy(&commit.message)
            .lines()
            .next()
            .unwrap_or("")
            .to_string();
        let short = &sha[..7];
        println!("stash@{{0}}: {short}: {subject}");
        // Follow the parent chain: worktree -> index -> HEAD -> ...
        if commit.parents.is_empty() {
            break;
        }
        sha = hex(&commit.parents[0]);
        // Stop after the index commit (which has HEAD as parent).
        if commit.parents.len() == 1 {
            let (k, c) = ObjectStore::discover()?.read_object(&sha)?;
            if k == Kind::Commit {
                let idx_commit = Commit::parse(&c)?;
                if idx_commit.parents.len() == 1 {
                    break;
                }
            }
        }
    }
    Ok(())
}

/// `git-rs stash pop [stash]`: apply the stash and drop it on success.
fn do_pop(stash_ref_arg: Option<&str>) -> Result<()> {
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

    // Resolve the stash ref (default to refs/stash).
    let stash_ref_name = stash_ref_arg.unwrap_or("refs/stash");
    let stash_sha = refs
        .resolve(stash_ref_name)?
        .ok_or_else(|| GitError::Fatal(format!("stash: '{stash_ref_name}' not found")))?;

    // Parse the stash commit (worktree commit).
    let (kind, content) = store.read_object(&stash_sha)?;
    if kind != Kind::Commit {
        return Err(GitError::Corrupt("stash ref does not point to a commit".into()));
    }
    let worktree_commit = Commit::parse(&content)?;

    // Its first parent is the index commit.
    let index_sha = worktree_commit
        .parents
        .first()
        .ok_or_else(|| GitError::Corrupt("stash worktree commit missing parent".into()))?;
    let (_, index_content) = store.read_object(&hex(index_sha))?;
    let index_commit = Commit::parse(&index_content)?;

    // The index commit's parent is the HEAD at stash time.
    let _base_sha = index_commit
        .parents
        .first()
        .ok_or_else(|| GitError::Corrupt("stash index commit missing parent".into()))?;

    // Current HEAD (may have moved since stash was created).
    let head = refs.resolve("HEAD")?.ok_or_else(|| unborn_fatal(&refs))?;
    let head_oid = parse_oid(&head)?;
    let head_tree = get_commit_tree(&store, head_oid)?;

    // Get the three trees for 3-way merge:
    // - base = index commit tree (the index at stash time)
    // - ours = current HEAD tree
    // - theirs = worktree commit tree (the worktree at stash time)
    let base_tree = hex(&index_commit.tree);
    let theirs_tree = hex(&worktree_commit.tree);

    let merged = merge_trees(&store, &base_tree, &head_tree, &theirs_tree)?;
    let old = tree_entries(&store, &head_tree)?;
    let (new_idx, conflicted) = apply_merged_files(
        &store, &root, &idx, &old, &merged, "stash",
    )?;

    // Write the merged index (contains staged resolutions + conflicts).
    new_idx.write(&ipath)?;

    if conflicted {
        println!(
            "Auto-merging <paths>\n\
             CONFLICT (content): Merge conflict in <paths>\n\
             Automatic merge failed; fix conflicts and then commit the result."
        );
        return Err(GitError::Invalid(String::new()));
    }

    // Clean apply: update worktree to match.
    let tree = tree_from_index(&store, new_idx.entries())?;
    crate::worktree::sync_worktree(&store, &root, Some(&head_tree), &tree)?;

    // Drop the stash (remove the ref).
    refs.delete(stash_ref_name)?;

    println!("Dropped {stash_ref_name} ({})", &stash_sha[..7]);
    Ok(())
}

/// `git-rs stash drop [stash]`: remove a stash entry.
fn do_drop(stash_ref_arg: Option<&str>) -> Result<()> {
    let refs = Refs::discover()?;
    let stash_ref_name = stash_ref_arg.unwrap_or("refs/stash");
    let stash_sha = refs
        .resolve(stash_ref_name)?
        .ok_or_else(|| GitError::Fatal(format!("stash: '{stash_ref_name}' not found")))?;

    refs.delete(stash_ref_name)?;
    println!("Dropped {stash_ref_name} ({})", &stash_sha[..7]);
    Ok(())
}

fn get_commit_tree(store: &ObjectStore, sha: [u8; 20]) -> Result<String> {
    let (kind, content) = store.read_object(&hex(&sha))?;
    if kind != Kind::Commit {
        return Err(GitError::Corrupt("not a commit".into()));
    }
    Ok(hex(&Commit::parse(&content)?.tree))
}

fn short_subject(_refs: &Refs, head_oid: [u8; 20]) -> Result<String> {
    let (_, content) = ObjectStore::discover()?.read_object(&hex(&head_oid))?;
    let commit = Commit::parse(&content)?;
    let subject = String::from_utf8_lossy(&commit.message)
        .lines()
        .next()
        .unwrap_or("")
        .to_string();
    Ok(subject)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_git_dir() -> std::path::PathBuf {
        let dir = env::temp_dir().join(format!(
            "git-rs-stash-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("objects")).unwrap();
        fs::create_dir_all(dir.join("refs/heads")).unwrap();
        fs::create_dir_all(dir.join("logs")).unwrap();
        fs::write(dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(dir.join("config"), "[core]\n\trepositoryformatversion = 0\n").unwrap();
        dir
    }
}