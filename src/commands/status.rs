//! `git-rs status --short` — porcelain status, byte-identical to real
//! git's `git status --short` (probed against git 2.55).
//!
//! Columns: X = index vs HEAD (`A`/`M`/`D`/`R`), Y = worktree vs index
//! (`M`/`D`/` `). Untracked files and directories print as `?? <path>`,
//! with directory collapsing (an all-untracked subtree prints once, at its
//! topmost directory with no tracked descendants — probed). Paths are
//! relative to the current directory (`../` for parents) and C-quoted
//! (core.quotePath semantics: octal escapes for non-ASCII/control bytes).
//! v1: exact-content rename detection between HEAD and index only; unstaged
//! renames and merge-conflict entries are not reported (D-013).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::error::{GitError, IoContext, Result};
use crate::ignore::IgnoreMatcher;
use crate::index::Index;
use crate::object::Commit;
use crate::object::Tree;
use crate::refs::Refs;
use crate::store::{Kind, ObjectStore};
use crate::worktree::{repo_root, walk_worktree};

/// `git-rs status [--short]`
pub fn run_status(args: &[String]) -> Result<()> {
    for arg in args {
        match arg.as_str() {
            "--short" | "-s" => {}
            s if s.starts_with('-') => {
                return Err(GitError::Invalid(format!("status: unknown option '{arg}'")));
            }
            s => {
                return Err(GitError::Invalid(format!(
                    "status: unexpected argument '{s}'"
                )));
            }
        }
    }

    let refs = Refs::discover()?;
    let git_dir = crate::worktree::abs_git_dir(refs.git_dir())?;
    let root = repo_root(&git_dir)?;
    let index_path = crate::worktree::index_path(&git_dir);
    let idx = if index_path.exists() {
        Index::read(&index_path)?
    } else {
        Index::new()
    };
    let matcher = IgnoreMatcher::load(&root, &git_dir)?;
    let store = ObjectStore::discover()?;
    let cwd = std::env::current_dir().context("<cwd>", "current directory")?;
    let prefix = cwd_prefix(&root, &cwd)?;

    let head = load_head_blobs(&store, &refs)?;
    let items = walk_worktree(&root, &git_dir, &matcher)?;
    let wt_files: HashSet<&[u8]> = items
        .iter()
        .filter(|i| !i.is_dir)
        .map(|i| i.path.as_slice())
        .collect();

    // X column: index (stage 0) vs HEAD.
    let mut lines: Vec<(Vec<u8>, String)> = Vec::new(); // section 1: index/worktree changes
    let mut staged_del: Vec<(Vec<u8>, [u8; 20])> = Vec::new(); // (path, head oid)
    let mut staged_add: Vec<(Vec<u8>, [u8; 20])> = Vec::new(); // (path, index oid)
    for e in idx.entries().iter().filter(|e| e.stage() == 0) {
        let x = match head.get(&e.path) {
            None => b'A',
            Some(ho) if *ho != e.oid => b'M',
            Some(_) => b' ',
        };
        if x == b'A' {
            staged_add.push((e.path.clone(), e.oid));
        }
        let y = if wt_files.contains(&e.path.as_slice()) {
            let abs = root.join(rel_os_path(&e.path));
            match crate::worktree::hash_entry(&store, &abs, false) {
                Ok(h) if h == e.oid => b' ',
                _ => b'M',
            }
        } else {
            b'D'
        };
        if x != b' ' || y != b' ' {
            let p = rel_to_cwd(&e.path, &prefix);
            lines.push((e.path.clone(), format!("{}{} {}", x as char, y as char, p)));
        }
    }
    for (path, oid) in &head {
        if !idx
            .entries()
            .iter()
            .any(|e| e.stage() == 0 && e.path == *path)
        {
            staged_del.push((path.clone(), *oid));
            let p = rel_to_cwd(path, &prefix);
            lines.push((path.clone(), format!("D  {p}")));
        }
    }

    // Renames: exact content match between staged deletions and staged
    // additions (probed: `R  old -> new`).
    let mut renames: HashSet<Vec<u8>> = HashSet::new();
    let mut renames_out: Vec<(Vec<u8>, String)> = Vec::new();
    for (old, old_oid) in &staged_del {
        if let Some((new, _)) = staged_add.iter().find(|(_, oid)| oid == old_oid) {
            renames.insert(new.clone());
            renames.insert(old.clone());
            let o = rel_to_cwd(old, &prefix);
            let n = rel_to_cwd(new, &prefix);
            renames_out.push((new.clone(), format!("R  {o} -> {n}")));
        }
    }
    lines.retain(|(p, _)| !renames.contains(p));
    lines.extend(renames_out);

    // Untracked: worktree files not in the index and not ignored, with
    // directory collapsing, plus embedded-repo directories.
    let mut untracked: Vec<Vec<u8>> = Vec::new();
    let mut untracked_lines: Vec<(Vec<u8>, String)> = Vec::new();
    for it in &items {
        if it.is_dir {
            let raw = raw_to_cwd(&it.path, &prefix);
            let p = String::from_utf8_lossy(&raw).into_owned();
            untracked_lines.push((raw, format!("?? {p}/")));
            continue;
        }
        if !idx.entries().iter().any(|e| e.path == it.path) && !matcher.is_ignored(&it.path, false)
        {
            untracked.push(it.path.clone());
        }
    }
    untracked.sort();
    let tracked_dirs = tracked_ancestors(idx.entries());
    let mut seen: HashSet<Vec<u8>> = HashSet::new();
    for f in untracked {
        let mut disp = f.clone();
        while let Some(i) = disp.iter().rposition(|&b| b == b'/') {
            let parent = disp[..i].to_vec();
            if tracked_dirs.contains(&parent) {
                break;
            }
            disp = parent;
        }
        if !seen.insert(disp.clone()) {
            continue;
        }
        let p = rel_to_cwd(&disp, &prefix);
        let trailing = if disp != f { "/" } else { "" };
        let mut raw = raw_to_cwd(&disp, &prefix);
        if disp != f {
            raw.push(b'/');
        }
        untracked_lines.push((raw, format!("?? {p}{trailing}")));
    }

    // Section order (probed): tracked changes first, then untracked; each
    // section sorted by the displayed (unquoted) path — collapsed dirs
    // sort as `dir/`, which pushes them after `dir`-prefixed files.
    lines.sort_by(|a, b| a.0.cmp(&b.0));
    untracked_lines.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, line) in lines.into_iter().chain(untracked_lines) {
        println!("{line}");
    }
    Ok(())
}

/// Every ancestor directory of every index path (directories that contain
/// tracked files block untracked collapse).
fn tracked_ancestors(entries: &[crate::index::IndexEntry]) -> HashSet<Vec<u8>> {
    let mut dirs = HashSet::new();
    for e in entries {
        if let Some(i) = e.path.iter().rposition(|&b| b == b'/') {
            let mut d = e.path[..i].to_vec();
            loop {
                dirs.insert(d.clone());
                match d.iter().rposition(|&b| b == b'/') {
                    Some(j) => d = d[..j].to_vec(),
                    None => break,
                }
            }
        }
    }
    dirs
}

/// Map of path -> blob oid for every blob in HEAD's tree.
fn load_head_blobs(store: &ObjectStore, refs: &Refs) -> Result<HashMap<Vec<u8>, [u8; 20]>> {
    let mut out = HashMap::new();
    let Some(head) = refs.resolve("HEAD")? else {
        return Ok(out);
    };
    let (kind, content) = store.read_object(&head)?;
    if kind != Kind::Commit {
        return Ok(out);
    }
    let commit = Commit::parse(&content)?;
    let tree_hex = hex(&commit.tree);
    let (_, tcontent) = store.read_object(&tree_hex)?;
    collect_tree(store, &tcontent, &mut vec![], &mut out)?;
    Ok(out)
}

fn collect_tree(
    store: &ObjectStore,
    content: &[u8],
    prefix: &mut Vec<u8>,
    out: &mut HashMap<Vec<u8>, [u8; 20]>,
) -> Result<()> {
    let tree = Tree::parse(content)?;
    for e in tree.entries {
        if e.mode == 0o040000 {
            let len = prefix.len();
            prefix.extend_from_slice(&e.name);
            prefix.push(b'/');
            let hex = hex(&e.oid);
            let (_, sub) = store.read_object(&hex)?;
            collect_tree(store, &sub, prefix, out)?;
            prefix.truncate(len);
        } else {
            let mut path = prefix.clone();
            path.extend_from_slice(&e.name);
            out.insert(path, e.oid);
        }
    }
    Ok(())
}

/// C-quote a path like git (core.quotePath=true): spaces, bytes below 0x20
/// or from 0x7f up, quotes, and backslashes force quoting. Only the
/// escape-able bytes are rewritten — space prints as-is inside the quotes.
fn c_quote(path: &[u8]) -> String {
    let needs = path
        .iter()
        .any(|&b| !(0x21..0x7f).contains(&b) || b == b'"' || b == b'\\');
    if !needs {
        return String::from_utf8_lossy(path).into_owned();
    }
    let mut s = String::from("\"");
    for &b in path {
        match b {
            b'"' => s.push_str("\\\""),
            b'\\' => s.push_str("\\\\"),
            b'\n' => s.push_str("\\n"),
            b'\t' => s.push_str("\\t"),
            b'\r' => s.push_str("\\r"),
            b if !(0x20..0x7f).contains(&b) => s.push_str(&format!("\\{b:03o}")),
            _ => s.push(b as char),
        }
    }
    s.push('"');
    s
}

/// Path relative to the current directory, `../`-prefixed when above it,
/// then C-quoted as one string (git quotes the whole relative path).
fn rel_to_cwd(rel: &[u8], prefix: &[u8]) -> String {
    let raw = raw_to_cwd(rel, prefix);
    c_quote(&raw)
}

/// Same as `rel_to_cwd` but without quoting — the sort key.
fn raw_to_cwd(rel: &[u8], prefix: &[u8]) -> Vec<u8> {
    let rel_segs: Vec<&[u8]> = rel
        .split(|&b| b == b'/')
        .filter(|s| !s.is_empty())
        .collect();
    let pre_segs: Vec<&[u8]> = prefix
        .split(|&b| b == b'/')
        .filter(|s| !s.is_empty())
        .collect();
    let mut shared = 0;
    while shared < pre_segs.len() && shared < rel_segs.len() && pre_segs[shared] == rel_segs[shared]
    {
        shared += 1;
    }
    let mut out = Vec::new();
    for _ in shared..pre_segs.len() {
        out.extend_from_slice(b"../");
    }
    out.extend(rel_segs[shared..].join(&b'/'));
    out
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

fn rel_os_path(rel: &[u8]) -> PathBuf {
    let s = String::from_utf8_lossy(rel);
    PathBuf::from(s.replace('/', std::path::MAIN_SEPARATOR_STR))
}

fn hex(oid: &[u8; 20]) -> String {
    oid.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_quote_basics() {
        assert_eq!(c_quote(b"a.txt"), "a.txt");
        assert_eq!(c_quote(b"a b.txt"), "\"a b.txt\"");
        assert_eq!(c_quote(b"cafu\xe9.txt"), "\"cafu\\351.txt\"");
        assert_eq!(c_quote(b"a\"b"), "\"a\\\"b\"");
        assert_eq!(c_quote(b"a\\b"), "\"a\\\\b\"");
        assert_eq!(c_quote(b"n\nl"), "\"n\\nl\"");
    }

    #[test]
    fn rel_to_cwd_prefixes_parents() {
        assert_eq!(rel_to_cwd(b"a.txt", b""), "a.txt");
        assert_eq!(rel_to_cwd(b"sub/f.txt", b"sub"), "f.txt");
        assert_eq!(rel_to_cwd(b"a/f.txt", b"sub"), "../a/f.txt");
        assert_eq!(rel_to_cwd(b"x/f.txt", b"a/b"), "../../x/f.txt");
        assert_eq!(rel_to_cwd(b"a b.txt", b""), "\"a b.txt\"");
    }

    #[test]
    fn tracked_ancestors_collects_all() {
        let e = |p: &[u8]| crate::index::IndexEntry {
            ctime_sec: 0,
            ctime_nsec: 0,
            mtime_sec: 0,
            mtime_nsec: 0,
            dev: 0,
            ino: 0,
            mode: 0o100644,
            uid: 0,
            gid: 0,
            size: 0,
            oid: [0; 20],
            flags: 0,
            extended_flags: 0,
            path: p.to_vec(),
        };
        let dirs = tracked_ancestors(&[e(b"a/b/c.txt"), e(b"x.txt")]);
        assert!(dirs.contains(b"a/b".as_slice()));
        assert!(dirs.contains(b"a".as_slice()));
        assert!(!dirs.contains(b"x".as_slice()));
        assert!(!dirs.contains(b"".as_slice()));
    }
}
