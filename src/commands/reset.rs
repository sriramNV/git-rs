use std::path::Path;

use crate::error::{GitError, Result};
use crate::index::Index;
use crate::object::Commit;
use crate::refs::Refs;
use crate::revwalk::{object_name_error, resolve_rev};
use crate::store::{Kind, ObjectStore};
use crate::worktree::{abs_git_dir, index_path, repo_root, tree_entries};

/// Run `git reset [--soft|--mixed|--hard] [<commit>]`.
pub fn run_reset(args: &[String]) -> Result<()> {
    let mut mode = ResetMode::Mixed;
    let mut target: Option<String> = None;
    let mut quiet = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--soft" => mode = ResetMode::Soft,
            "--mixed" => mode = ResetMode::Mixed,
            "--hard" => mode = ResetMode::Hard,
            "-q" | "--quiet" => quiet = true,
            s if s.starts_with('-') => {
                return Err(GitError::Invalid(format!("reset: unknown option '{s}'")));
            }
            s => {
                if target.is_none() {
                    target = Some(s.to_string());
                } else {
                    return Err(GitError::Invalid("reset: too many arguments".into()));
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

    // Current HEAD
    let head = refs.resolve("HEAD")?;
    let old_tree = match head {
        Some(ref h) => Some(get_commit_tree(&store, parse_oid(h)?)?),
        None => None,
    };

    // Resolve target (default HEAD)
    let target_rev = target.unwrap_or_else(|| "HEAD".to_string());
    let target_sha =
        resolve_rev(&refs, &store, &target_rev)?.ok_or_else(|| object_name_error(&target_rev))?;

    // Target tree
    let target_tree = get_commit_tree(&store, target_sha)?;

    match mode {
        ResetMode::Soft => {
            // Just move HEAD
            let sha_hex = hex(&target_sha);
            refs.update("HEAD", &sha_hex, &format!("reset: moving to {target_rev}"))?;
            // Soft: no output
        }
        ResetMode::Mixed => {
            // Move HEAD + rewrite index
            let sha_hex = hex(&target_sha);
            refs.update("HEAD", &sha_hex, &format!("reset: moving to {target_rev}"))?;
            rewrite_index(&store, &root, &mut idx, &target_tree)?;
            idx.write(&ipath)?;

            // Unstaged changes after reset (stdout)
            if !quiet {
                print_unstaged_changes(&root, &git_dir, &store, &idx)?;
            }
        }
        ResetMode::Hard => {
            // Move HEAD + index + worktree (--hard also discards local edits);
            // files first, index last so stat fields match the final bytes.
            let sha_hex = hex(&target_sha);
            refs.update("HEAD", &sha_hex, &format!("reset: moving to {target_rev}"))?;
            crate::worktree::force_sync_worktree(&store, &root, old_tree.as_deref(), &target_tree)?;
            // Git's hard reset also deletes files tracked in the current
            // index but absent from the target tree (e.g. a merge's staged
            // additions, or a staged new file).
            let target_entries = tree_entries(&store, &target_tree)?;
            let target: Vec<&[u8]> = target_entries
                .iter()
                .map(|(p, _, _)| p.as_slice())
                .collect();
            for e in idx.entries().iter().filter(|e| e.stage() == 0) {
                if !target.contains(&e.path.as_slice()) {
                    crate::worktree::remove_file_and_empty_dirs(
                        &root.join(crate::worktree::rel_os_path(&e.path)),
                    );
                }
            }
            rewrite_index(&store, &root, &mut idx, &target_tree)?;
            idx.write(&ipath)?;

            // HEAD is now at <sha> <subject> (stdout)
            if !quiet {
                let short = short_sha(&sha_hex);
                let subj = commit_subject(&store, target_sha)?;
                println!("HEAD is now at {short} {subj}");
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResetMode {
    Soft,
    Mixed,
    Hard,
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

fn commit_subject(store: &ObjectStore, sha: [u8; 20]) -> Result<String> {
    let (kind, content) = store.read_object(&hex(&sha))?;
    if kind != Kind::Commit {
        return Ok("".to_string());
    }
    let commit = Commit::parse(&content)?;
    let msg = String::from_utf8_lossy(&commit.message);
    Ok(msg.lines().next().unwrap_or("").to_string())
}

fn print_unstaged_changes(
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
        println!("Unstaged changes after reset:");
        for line in out {
            println!("{line}");
        }
    }
    Ok(())
}

fn short_sha(sha: &str) -> String {
    if sha.len() >= 7 {
        sha[..7].to_string()
    } else {
        sha.to_string()
    }
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
