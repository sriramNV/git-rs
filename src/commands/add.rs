//! `git-rs add` — stage worktree changes into the index.
//!
//! v1: named pathspecs and `.` only (no `-A`/`-u`/`-f`), per-directory
//! `.gitignore` honored (D-013). `git add .` stages deletions of tracked
//! files missing from the worktree; a named path missing from disk but
//! tracked stages its deletion (probed: exit 0). Ignored paths abort the
//! whole command with git's exact message, staging nothing.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{GitError, IoContext, Result};
use crate::ignore::IgnoreMatcher;
use crate::index::{Index, IndexEntry};
use crate::refs::Refs;
use crate::store::ObjectStore;
use crate::worktree::{hash_entry, repo_root, stat_file, walk_worktree};

/// `git-rs add <pathspec>...`
pub fn run_add(args: &[String]) -> Result<()> {
    let mut paths: Vec<&str> = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-A" | "--all" | "-u" | "-f" | "-n" | "--dry-run" | "--ignore-errors" => {
                return Err(GitError::Invalid(format!(
                    "add: option '{arg}' not implemented in v1"
                )));
            }
            s if s.starts_with('-') => {
                return Err(GitError::Invalid(format!("add: unknown option '{arg}'")));
            }
            s => paths.push(s),
        }
    }
    if paths.is_empty() {
        return Err(GitError::Invalid(
            "Nothing specified, nothing added.".into(),
        ));
    }

    let refs = Refs::discover()?;
    let git_dir = crate::worktree::abs_git_dir(refs.git_dir())?;
    let root = repo_root(&git_dir)?;
    let index_path = crate::worktree::index_path(&git_dir);
    let mut idx = if index_path.exists() {
        Index::read(&index_path)?
    } else {
        Index::new()
    };
    let matcher = IgnoreMatcher::load(&root, &git_dir)?;
    let store = ObjectStore::discover()?;
    let cwd = std::env::current_dir().context("<cwd>", "current directory")?;
    let prefix = cwd_prefix(&root, &cwd)?;

    let mut dirty = false;
    let mut ignored: Vec<Vec<u8>> = Vec::new();

    for arg in &paths {
        if *arg == "." {
            let items = walk_worktree(&root, &git_dir, &matcher)?;
            let mut kept = HashSet::new();
            for it in &items {
                if it.is_dir {
                    continue;
                }
                if matcher.is_ignored(&it.path, false) {
                    continue; // `add .` skips ignored files silently (probed)
                }
                let e = build_entry(&root, &store, &it.path)?;
                idx.stage(e);
                kept.insert(it.path.clone());
                dirty = true;
            }
            let before = idx.entries().len();
            idx.entries_mut()
                .retain(|e| e.stage() != 0 || kept.contains(&e.path));
            if idx.entries().len() != before {
                dirty = true;
            }
            continue;
        }

        let rel = repo_rel(&root, &cwd, &prefix, arg)?;
        let abs = cwd.join(arg);
        let md = match fs::symlink_metadata(&abs) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if idx
                    .entries()
                    .iter()
                    .any(|en| en.path == rel && en.stage() == 0)
                {
                    idx.unstage(&rel);
                    dirty = true;
                } else {
                    return Err(GitError::Fatal(format!(
                        "pathspec '{arg}' did not match any files"
                    )));
                }
                continue;
            }
            Err(e) => {
                return Err(GitError::io(abs.display().to_string(), "stat path", e));
            }
        };

        if md.is_dir() {
            let items = walk_worktree(&root, &git_dir, &matcher)?;
            let mut deleted: HashSet<&[u8]> = HashSet::new();
            for it in &items {
                if it.is_dir || !under(&it.path, &rel) {
                    continue;
                }
                if matcher.is_ignored(&it.path, false) {
                    continue; // directory recursion: silent, like `add .`
                }
                let e = build_entry(&root, &store, &it.path)?;
                idx.stage(e);
                deleted.insert(it.path.as_slice());
                dirty = true;
            }
            // Deletions inside the named directory (git add <dir> = -A
            // for that directory).
            let before = idx.entries().len();
            idx.entries_mut().retain(|e| {
                e.stage() != 0 || !under(&e.path, &rel) || deleted.contains(e.path.as_slice())
            });
            if idx.entries().len() != before {
                dirty = true;
            }
        } else {
            if matcher.is_ignored(&rel, false) {
                ignored.push(rel.clone());
                continue;
            }
            let e = build_entry(&root, &store, &rel)?;
            idx.stage(e);
            dirty = true;
        }
    }

    if !ignored.is_empty() {
        let mut msg =
            "The following paths are ignored by one of your .gitignore files:\n".to_string();
        for p in &ignored {
            msg.push_str(&String::from_utf8_lossy(p));
            msg.push('\n');
        }
        msg.push_str("hint: Use -f if you really want to add them.\n");
        msg.push_str(
            "hint: Disable this message with \"git config set advice.addIgnoredFile false\"",
        );
        // Invalid: real git exits 1 for this error, not 128 (probed).
        return Err(GitError::Invalid(msg));
    }

    if dirty {
        idx.write(&index_path)?;
    }
    Ok(())
}

/// Stat + hash a worktree file into an index entry (writes the blob).
pub(crate) fn build_entry(root: &Path, store: &ObjectStore, rel: &[u8]) -> Result<IndexEntry> {
    let abs = root.join(rel_os_path(rel));
    let st = stat_file(&abs)?;
    let oid = hash_entry(store, &abs, true)?;
    Ok(IndexEntry {
        ctime_sec: st.ctime_sec,
        ctime_nsec: st.ctime_nsec,
        mtime_sec: st.mtime_sec,
        mtime_nsec: st.mtime_nsec,
        dev: 0,
        ino: 0,
        mode: st.mode,
        uid: 0,
        gid: 0,
        size: st.size,
        oid,
        flags: 0,
        extended_flags: 0,
        path: rel.to_vec(),
    })
}

/// Repo-relative prefix of `cwd` under `root`, as `/`-separated bytes.
fn cwd_prefix(root: &Path, cwd: &Path) -> Result<Vec<u8>> {
    let rel = cwd.strip_prefix(root).map_err(|_| {
        GitError::Fatal(format!(
            "cannot determine path relative to '{}' (run git-rs from inside the repository)",
            root.display()
        ))
    })?;
    Ok(rel
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
        .into_bytes())
}

/// Repo-relative path for a command-line argument (resolving `.`/`..`),
/// refusing paths that escape the repository (git's "is outside
/// repository" fatal). The escape check runs on the segment stack — `..`
/// popping an empty stack means the path leaves the repo (lexical
/// `starts_with` on joined paths would miss this).
fn repo_rel(_root: &Path, cwd: &Path, prefix: &[u8], arg: &str) -> Result<Vec<u8>> {
    let mut segs: Vec<&[u8]> = Vec::new();
    if !prefix.is_empty() {
        segs.extend(prefix.split(|&b| b == b'/').filter(|s| !s.is_empty()));
    }
    let normalized = arg.replace('\\', "/");
    for seg in normalized.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if segs.is_empty() {
                    return Err(GitError::Fatal(format!(
                        "'{}' is outside repository",
                        cwd.join(arg).display()
                    )));
                }
                segs.pop();
            }
            s => segs.push(s.as_bytes()),
        }
    }
    Ok(segs.join(&b'/'))
}

fn under(path: &[u8], dir: &[u8]) -> bool {
    path.len() > dir.len() && path.starts_with(dir) && path[dir.len()] == b'/'
}

fn rel_os_path(rel: &[u8]) -> PathBuf {
    let s = String::from_utf8_lossy(rel);
    PathBuf::from(s.replace('/', std::path::MAIN_SEPARATOR_STR))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_rel_resolves_dots() {
        let root = Path::new("C:/repo");
        let cwd = Path::new("C:/repo/sub");
        let p = repo_rel(root, cwd, b"sub", "./x/../y.txt").unwrap();
        assert_eq!(p, b"sub/y.txt");
        let p = repo_rel(root, cwd, b"sub", "..").unwrap();
        assert_eq!(p, b"");
        assert!(repo_rel(root, cwd, b"sub", "../..").is_err());
    }

    #[test]
    fn under_checks_dir_prefix() {
        assert!(under(b"a/b/c", b"a/b"));
        assert!(under(b"a/b", b"a"));
        assert!(!under(b"ab/c", b"a"));
        assert!(!under(b"a", b"a"));
    }
}
