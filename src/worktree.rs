//! Worktree helpers shared by `add` and `status`: stat fields, blob
//! content, recursive walking, index path resolution.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::error::{GitError, IoContext, Result};
use crate::ignore::IgnoreMatcher;
use crate::object::Tree;
use crate::store::{Kind, ObjectStore};

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

/// Stat a worktree entry, zeroing all fields when it is missing (git stores
/// zeroed stats for index entries whose worktree copy is absent; content is
/// re-checked by hash anyway).
pub fn stat_file_or_zero(path: &Path) -> WorkStat {
    stat_file(path).unwrap_or(WorkStat {
        ctime_sec: 0,
        ctime_nsec: 0,
        mtime_sec: 0,
        mtime_nsec: 0,
        mode: 0,
        size: 0,
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

/// Flatten a tree into leaf paths, each with its mode and blob oid.
/// Subtree entries are descended (paths `/`-separated bytes); gitlinks
/// (mode 160000) are skipped — ponytail: embedded repos are out of scope.
pub fn tree_entries(store: &ObjectStore, tree: &str) -> Result<Vec<(Vec<u8>, u32, [u8; 20])>> {
    let mut out = Vec::new();
    collect_tree(store, tree, &mut Vec::new(), &mut out)?;
    Ok(out)
}

fn collect_tree(
    store: &ObjectStore,
    oid: &str,
    prefix: &mut Vec<u8>,
    out: &mut Vec<(Vec<u8>, u32, [u8; 20])>,
) -> Result<()> {
    let (kind, content) = store.read_object(oid)?;
    let tree = match kind {
        Kind::Tree => Tree::parse(&content)?,
        _ => {
            return Err(GitError::Corrupt(format!("'{oid}' is not a tree")));
        }
    };
    for e in &tree.entries {
        if e.is_dir() {
            if e.mode == 0o160000 {
                continue; // gitlink
            }
            let save = prefix.len();
            prefix.extend_from_slice(&e.name);
            prefix.push(b'/');
            collect_tree(store, &hex(&e.oid), prefix, out)?;
            prefix.truncate(save);
        } else {
            let mut path = prefix.clone();
            path.extend_from_slice(&e.name);
            out.push((path, e.mode, e.oid));
        }
    }
    Ok(())
}

fn hex(oid: &[u8; 20]) -> String {
    oid.iter().map(|b| format!("{b:02x}")).collect()
}

/// Make the worktree match `new_tree` starting from `old_tree` (both hex
/// tree oids; `None` = unborn, every new path is written). Deletes paths
/// that disappeared, rewrites changed blobs atomically (temp + rename),
/// leaves untracked files alone, prunes empty directories. Used by
/// `checkout` and `reset --hard`.
/// `checkout -f` / `reset --hard`: sync to the target tree AND overwrite
/// every tracked file whose worktree content differs from the target oid
/// (discard local edits), deleting tree-only paths as usual.
pub fn force_sync_worktree(
    store: &ObjectStore,
    root: &Path,
    old_tree: Option<&str>,
    new_tree: &str,
) -> Result<()> {
    let old = match old_tree {
        Some(t) => tree_entries(store, t)?,
        None => Vec::new(),
    };
    let new = tree_entries(store, new_tree)?;

    for (path, _, _) in &old {
        if !new.iter().any(|(p, _, _)| p == path) {
            let abs = root.join(rel_os_path(path));
            remove_file_and_empty_dirs(&abs);
        }
    }
    for (path, mode, oid) in &new {
        let absent = fs::read(root.join(rel_os_path(path))).ok();
        let content_matches = absent
            .as_ref()
            .map(|c| ObjectStore::hash(Kind::Blob, c) == hex(oid))
            .unwrap_or(false);
        if !content_matches {
            write_blob(store, root, path, *mode, oid)?;
        }
    }
    Ok(())
}

pub fn sync_worktree(
    store: &ObjectStore,
    root: &Path,
    old_tree: Option<&str>,
    new_tree: &str,
) -> Result<()> {
    let old = match old_tree {
        Some(t) => tree_entries(store, t)?,
        None => Vec::new(),
    };
    let new = tree_entries(store, new_tree)?;

    for (path, _, _) in &old {
        if !new.iter().any(|(p, _, _)| p == path) {
            let abs = root.join(rel_os_path(path));
            remove_file_and_empty_dirs(&abs);
        }
    }
    for (path, mode, oid) in &new {
        let in_old = old
            .iter()
            .any(|(p, m, o)| p == path && m == mode && o == oid);
        if !in_old {
            write_blob(store, root, path, *mode, oid)?;
        }
    }
    Ok(())
}

fn remove_file_and_empty_dirs(abs: &Path) {
    let _ = fs::remove_file(abs);
    let mut dir = match abs.parent() {
        Some(d) => d.to_path_buf(),
        None => return,
    };
    loop {
        match fs::remove_dir(&dir) {
            Ok(()) => match dir.parent() {
                Some(p) => dir = p.to_path_buf(),
                None => return,
            },
            Err(_) => return, // not empty or not a dir — stop pruning
        }
    }
}

/// Write one leaf from a tree: symlinks (mode 120000) become symlinks,
/// everything else a regular file with the blob bytes.
fn write_blob(
    store: &ObjectStore,
    root: &Path,
    rel: &[u8],
    mode: u32,
    oid: &[u8; 20],
) -> Result<()> {
    let (kind, content) = store.read_object(&hex(oid))?;
    if kind != Kind::Blob {
        return Err(GitError::Corrupt(format!("'{}' is not a blob", hex(oid))));
    }
    let abs = root.join(rel_os_path(rel));
    let dir = abs
        .parent()
        .ok_or_else(|| GitError::Corrupt(format!("path '{}' has no parent", abs.display())))?;
    fs::create_dir_all(dir).context(dir, "create directory")?;
    if mode == 0o120000 {
        write_symlink(&abs, &content).context(&abs, "write symlink")?;
    } else {
        let tmp = dir.join(format!(".tmp-gitrs-{}", std::process::id()));
        let mut f = fs::File::create(&tmp).context(&tmp, "create temp file")?;
        use std::io::Write;
        f.write_all(&content).context(&tmp, "write temp file")?;
        f.sync_all().context(&tmp, "fsync temp file")?;
        fs::rename(&tmp, &abs).context(&abs, "commit file")?;
    }
    Ok(())
}

/// Symlink with the blob bytes as target. On Windows, creation needs
/// privilege; without it fall back to a regular file holding the target
/// text — same as git with `core.symlinks=false` (probed).
#[cfg(windows)]
fn write_symlink(path: &Path, target: &[u8]) -> std::io::Result<()> {
    use std::os::windows::fs::symlink_file;
    match symlink_file(String::from_utf8_lossy(target).into_owned(), path) {
        Ok(()) => Ok(()),
        Err(_) => fs::write(path, target),
    }
}

#[cfg(unix)]
fn write_symlink(path: &Path, target: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::symlink;
    symlink(String::from_utf8_lossy(target).into_owned(), path)
}

/// Repo-relative bytes to an OS path (`/` → `MAIN_SEPARATOR`).
pub fn rel_os_path(rel: &[u8]) -> PathBuf {
    let s = String::from_utf8_lossy(rel);
    PathBuf::from(s.replace('/', std::path::MAIN_SEPARATOR_STR))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_fs() -> (ObjectStore, PathBuf) {
        let dir = env::temp_dir().join(format!(
            "git-rs-wt-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        (ObjectStore::new(dir.join("objects")), dir)
    }

    fn blob(store: &ObjectStore, content: &[u8]) -> [u8; 20] {
        parse_oid(&store.write_blob(content).unwrap()).unwrap()
    }

    /// Build a tree with entries [(path, mode, oid)], writing subtrees.
    fn tree_of(store: &ObjectStore, files: &[(&str, u32, [u8; 20])]) -> String {
        let mut by_dir: std::collections::BTreeMap<String, Vec<(String, u32, [u8; 20])>> =
            std::collections::BTreeMap::new();
        for (path, mode, oid) in files {
            let segs: Vec<&str> = path.split('/').collect();
            for i in 0..segs.len().saturating_sub(1) {
                // Register intermediate dirs as children of their parent.
                let parent = segs[..i].join("/");
                by_dir
                    .entry(parent)
                    .or_default()
                    .push((segs[i].to_string(), 0o040000, [0u8; 20]));
            }
            let (dir, name) = path
                .rsplit_once('/')
                .map_or((String::new(), path.to_string()), |(d, n)| {
                    (d.to_string(), n.to_owned())
                });
            by_dir.entry(dir).or_default().push((name, *mode, *oid));
        }
        fn build(
            store: &ObjectStore,
            by_dir: &std::collections::BTreeMap<String, Vec<(String, u32, [u8; 20])>>,
            dir: &str,
        ) -> String {
            let mut entries = Vec::new();
            let mut names: Vec<&str> = by_dir[dir].iter().map(|(n, _, _)| n.as_str()).collect();
            names.sort_unstable();
            names.dedup();
            for name in names {
                let child_dir = if dir.is_empty() {
                    name.to_string()
                } else {
                    format!("{dir}/{name}")
                };
                if by_dir.contains_key(&child_dir) {
                    let sub = build(store, by_dir, &child_dir);
                    entries.push(crate::object::TreeEntry {
                        mode: 0o040000,
                        name: name.as_bytes().to_vec(),
                        oid: parse_oid(&sub).unwrap(),
                    });
                } else {
                    let (_, mode, oid) = by_dir[dir].iter().find(|(n, _, _)| n == name).unwrap();
                    entries.push(crate::object::TreeEntry {
                        mode: *mode,
                        name: name.as_bytes().to_vec(),
                        oid: *oid,
                    });
                }
            }
            let tree = crate::object::Tree { entries };
            store
                .write_object(Kind::Tree, &tree.serialize().unwrap())
                .unwrap()
        }
        build(store, &by_dir, "")
    }

    #[test]
    fn sync_writes_adds_and_modifies() {
        let (store, root) = temp_fs();
        let b1 = blob(&store, b"one");
        let b2 = blob(&store, b"two");
        let t1 = tree_of(&store, &[("a.txt", 0o100644, b1)]);
        let t2 = tree_of(
            &store,
            &[("a.txt", 0o100644, b2), ("sub/b.txt", 0o100644, b1)],
        );
        sync_worktree(&store, &root, Some(&t1), &t2).unwrap();
        assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"two");
        assert_eq!(fs::read(root.join("sub/b.txt")).unwrap(), b"one");
    }

    #[test]
    fn sync_deletes_removed_and_prunes_empty_dirs() {
        let (store, root) = temp_fs();
        let b1 = blob(&store, b"one");
        let t1 = tree_of(
            &store,
            &[("keep.txt", 0o100644, b1), ("gone/dir/f.txt", 0o100644, b1)],
        );
        fs::create_dir_all(root.join("gone/dir")).unwrap();
        fs::write(root.join("gone/dir/f.txt"), b"one").unwrap();
        fs::write(root.join("keep.txt"), b"one").unwrap();
        let t2 = tree_of(&store, &[("keep.txt", 0o100644, b1)]);
        sync_worktree(&store, &root, Some(&t1), &t2).unwrap();
        assert!(!root.join("gone").exists());
        assert!(root.join("keep.txt").exists());
    }

    #[test]
    fn sync_preserves_untracked_files() {
        let (store, root) = temp_fs();
        let b1 = blob(&store, b"one");
        let t1 = tree_of(&store, &[("a.txt", 0o100644, b1)]);
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub/untracked.txt"), b"mine").unwrap();
        sync_worktree(&store, &root, Some(&t1), &t1).unwrap();
        assert_eq!(fs::read(root.join("sub/untracked.txt")).unwrap(), b"mine");
    }

    #[test]
    fn sync_unborn_writes_everything() {
        let (store, root) = temp_fs();
        let b1 = blob(&store, b"one");
        let t1 = tree_of(&store, &[("a.txt", 0o100644, b1)]);
        sync_worktree(&store, &root, None, &t1).unwrap();
        assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"one");
    }

    #[test]
    fn sync_skips_unchanged_files() {
        let (store, root) = temp_fs();
        let b1 = blob(&store, b"one");
        let t1 = tree_of(&store, &[("a.txt", 0o100644, b1)]);
        fs::write(root.join("a.txt"), b"one").unwrap();
        sync_worktree(&store, &root, Some(&t1), &t1).unwrap();
        assert!(fs::read_to_string(root.join("a.txt")).unwrap() == "one");
    }

    #[test]
    fn tree_entries_flattens_subtrees() {
        let (store, _root) = temp_fs();
        let b1 = blob(&store, b"one");
        let t = tree_of(
            &store,
            &[("a.txt", 0o100644, b1), ("x/y.txt", 0o100644, b1)],
        );
        let entries = tree_entries(&store, &t).unwrap();
        let paths: Vec<Vec<u8>> = entries.iter().map(|(p, _, _)| p.clone()).collect();
        assert_eq!(paths, vec![b"a.txt".to_vec(), b"x/y.txt".to_vec()]);
    }
}
