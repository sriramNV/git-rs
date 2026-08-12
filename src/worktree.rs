//! Worktree helpers shared by `add` and `status`: stat fields, blob
//! content, recursive walking, index path resolution.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::error::{GitError, IoContext, Result};
use crate::ignore::IgnoreMatcher;
use crate::store::ObjectStore;

/// Stat fields git stores in the index, derived from `fs::Metadata`.
#[derive(Debug, Clone, Copy)]
pub struct WorkStat {
    pub ctime_sec: i32,
    pub ctime_nsec: i32,
    pub mtime_sec: i32,
    pub mtime_nsec: i32,
    pub mode: u32,
    pub size: u32,
}

/// Split a `SystemTime` into (seconds, sub-second nanoseconds).
fn split_time(t: std::time::SystemTime) -> (i32, i32) {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => (d.as_secs() as i32, d.subsec_nanos() as i32),
        Err(_) => (0, 0),
    }
}

/// Stat a worktree entry via `symlink_metadata` (no following).
/// Windows: dev/ino/uid/gid are written as 0 — real git writes a
/// pseudo-inode there which needs FFI (banned); git re-checks content
/// on stat mismatch, so output stays identical (probed).
pub fn stat_file(path: &Path) -> Result<WorkStat> {
    let md = fs::symlink_metadata(path).context(path, "stat file")?;
    let modified = md.modified().unwrap_or(UNIX_EPOCH);
    let created = md.created().unwrap_or(modified);
    let (ctime_sec, ctime_nsec) = split_time(created);
    let (mtime_sec, mtime_nsec) = split_time(modified);
    let mode = if md.file_type().is_symlink() {
        0o120000
    } else {
        0o100644 // ponytail: exec bit untracked; core.filemode=false world
    };
    let size = md.len().min(u32::MAX as u64) as u32;
    Ok(WorkStat {
        ctime_sec,
        ctime_nsec,
        mtime_sec,
        mtime_nsec,
        mode,
        size,
    })
}

/// Blob content for a worktree entry: the file bytes, or for a symlink the
/// target path bytes (mode 120000).
pub fn blob_content(path: &Path) -> Result<Vec<u8>> {
    let md = fs::symlink_metadata(path).context(path, "symlink_metadata")?;
    if md.file_type().is_symlink() {
        let target = fs::read_link(path).context(path, "read symlink")?;
        Ok(target.to_string_lossy().into_owned().into_bytes())
    } else {
        fs::read(path).context(path, "read file")
    }
}

/// Hash a worktree entry as a blob, storing it if `write` is true.
pub fn hash_entry(store: &ObjectStore, path: &Path, write: bool) -> Result<[u8; 20]> {
    let content = blob_content(path)?;
    let id = ObjectStore::hash(crate::store::Kind::Blob, &content);
    if write {
        store.write_object(crate::store::Kind::Blob, &content)?;
    }
    parse_oid(&id)
}

/// 40-char hex to raw oid bytes, without panic paths.
pub fn parse_oid(s: &str) -> Result<[u8; 20]> {
    if s.len() != 40 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(GitError::Invalid(format!("bad object id '{s}'")));
    }
    let bytes = s.as_bytes();
    let mut out = [0u8; 20];
    for (i, pair) in bytes.chunks(2).enumerate() {
        out[i] = (nibble(pair[0]) << 4) | nibble(pair[1]);
    }
    Ok(out)
}

fn nibble(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        _ => unreachable!(), // parse_oid validated input
    }
}

/// Resolve the index path (`GIT_INDEX_FILE` env overrides `.git/index`).
pub fn index_path(git_dir: &Path) -> PathBuf {
    env::var("GIT_INDEX_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| git_dir.join("index"))
}

/// The working tree root for a git dir: its parent (v1: repo layout
/// `<root>/.git`; `GIT_DIR` pointing elsewhere means root = `GIT_DIR/..`).
pub fn repo_root(git_dir: &Path) -> Result<PathBuf> {
    git_dir.parent().map(PathBuf::from).ok_or_else(|| {
        GitError::Fatal(format!(
            "cannot determine repo root for '{}'",
            git_dir.display()
        ))
    })
}

/// Make a possibly-relative git dir (e.g. `.git` from discover()) absolute
/// so that root/index/walk paths survive any process cwd.
pub fn abs_git_dir(git_dir: &Path) -> Result<PathBuf> {
    if git_dir.is_absolute() {
        Ok(git_dir.to_path_buf())
    } else {
        let cwd = std::env::current_dir().context("<cwd>", "current directory")?;
        Ok(cwd.join(git_dir))
    }
}

/// A leaf encountered while walking: a file, or a directory that holds an
/// embedded repo (`.git` inside) — treated as an opaque untracked blob.
#[derive(Debug)]
pub struct WalkItem {
    /// Path relative to `root`, `/`-separated bytes.
    pub path: Vec<u8>,
    /// True only for embedded-repo directories.
    pub is_dir: bool,
}

/// Recursively collect all non-ignored files under `root` (skipping
/// `git_dir` and any directory containing a `.git` entry, which is
/// reported as a leaf instead of being descended). Ignored directories are
/// pruned — matching git's walk.
pub fn walk_worktree(
    root: &Path,
    git_dir: &Path,
    matcher: &IgnoreMatcher,
) -> Result<Vec<WalkItem>> {
    let mut out = Vec::new();
    walk_dir(root, root, git_dir, matcher, &mut Vec::new(), &mut out)?;
    Ok(out)
}

fn walk_dir(
    root: &Path,
    dir: &Path,
    git_dir: &Path,
    matcher: &IgnoreMatcher,
    rel: &mut Vec<u8>,
    out: &mut Vec<WalkItem>,
) -> Result<()> {
    if dir == git_dir {
        return Ok(());
    }
    let entries = fs::read_dir(dir)
        .map_err(|e| GitError::io(dir.display().to_string(), "read directory", e))?;
    let mut embedded = false;
    let mut subdirs: Vec<(std::ffi::OsString, Vec<u8>)> = Vec::new();
    for e in entries {
        let e =
            e.map_err(|e| GitError::io(dir.display().to_string(), "read directory entry", e))?;
        let name = e.file_name().to_string_lossy().into_owned();
        let ft = e
            .file_type()
            .map_err(|e| GitError::io(dir.display().to_string(), "stat directory entry", e))?;
        if ft.is_dir() {
            if name == ".git" && dir != root {
                embedded = true;
                continue;
            }
            let child = dir.join(e.file_name());
            if child != git_dir {
                subdirs.push((e.file_name(), name.into_bytes()));
            }
        } else if ft.is_file() {
            out.push(WalkItem {
                path: join_rel(rel, &name.into_bytes()),
                is_dir: false,
            });
        }
    }
    if embedded {
        out.push(WalkItem {
            path: rel.clone(),
            is_dir: true,
        });
        return Ok(());
    }
    for (os_name, name_bytes) in subdirs {
        let rel_len = rel.len();
        rel.extend_from_slice(&name_bytes);
        rel.push(b'/');
        if matcher.is_ignored(rel, true) {
            rel.truncate(rel_len);
            continue;
        }
        walk_dir(root, &dir.join(&os_name), git_dir, matcher, rel, out)?;
        rel.truncate(rel_len);
    }
    Ok(())
}

fn join_rel(rel: &[u8], name: &[u8]) -> Vec<u8> {
    let mut p = rel.to_vec();
    p.extend_from_slice(name);
    p
}
