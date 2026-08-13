//! `git-rs diff [--cached|--staged] [-- <paths>]` — worktree vs index
//! (default) or index vs HEAD, byte-identical to real git's unified output
//! (probed against git 2.55).
//!
//! v1 scope (D-014): plain path prefix matching for `-- <paths>` (no globs
//! or pathspec magic); paths that don't match anything produce no output;
//! gitlink (160000) entries are skipped; oids are abbreviated to 7 chars
//! (git's default for small repos); unmatched pathspecs are silently
//! ignored. `-M`/`-C` print "not supported" and exit 1.
//!
//! Path quoting follows git's `quote_two` (diff.c CQUOTE_NODQ): spaces are
//! NOT quoted, non-ASCII is octal-escaped inside quotes (core.quotepath on
//! by default); `---`/`+++` labels gain a trailing tab when they contain a
//! space; the `Binary files` line uses the `/dev/null`-aware labels.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use crate::diff::{FileDiff, diff_lines, is_binary, render, split_lines};
use crate::error::{GitError, IoContext, Result};
use crate::index::Index;
use crate::object::Commit;
use crate::object::Tree;
use crate::refs::Refs;
use crate::store::{Kind, ObjectStore};
use crate::worktree::{abs_git_dir, repo_root, stat_file};

/// Path -> (file mode, oid) maps for the HEAD tree and the index.
type ModeOidMap = HashMap<Vec<u8>, (u32, [u8; 20])>;

/// `git-rs diff [--cached|--staged] [-- <paths>]`
pub fn run_diff(args: &[String]) -> Result<()> {
    let mut cached = false;
    let mut pathspecs: Vec<String> = Vec::new();
    let mut after_sep = false;
    for arg in args {
        if after_sep {
            pathspecs.push(arg.clone());
            continue;
        }
        match arg.as_str() {
            "--" => after_sep = true,
            "--cached" | "--staged" => cached = true,
            "-M" | "--find-renames" | "-C" | "--find-copies" => {
                return Err(GitError::Invalid(format!(
                    "diff: '{arg}' (rename/copy detection) is not supported in v1"
                )));
            }
            s if s.starts_with('-') && s.len() > 1 => {
                return Err(GitError::Invalid(format!("diff: unknown option '{s}'")));
            }
            s => pathspecs.push(s.to_string()),
        }
    }

    let refs = Refs::discover()?;
    let git_dir = abs_git_dir(refs.git_dir())?;
    let root = repo_root(&git_dir)?;
    let index_path = crate::worktree::index_path(&git_dir);
    let idx = if index_path.exists() {
        Index::read(&index_path)?
    } else {
        Index::new()
    };
    let store = ObjectStore::discover()?;
    let cwd = std::env::current_dir().context("<cwd>", "current directory")?;
    let prefix = crate::commands::status::cwd_prefix(&root, &cwd)?;

    // HEAD blobs: path -> (mode, oid). Unborn HEAD = empty.
    let head = load_head_tree(&store, &refs)?;
    let mut index_map: ModeOidMap = HashMap::new();
    for e in idx
        .entries()
        .iter()
        .filter(|e| e.stage() == 0 && e.mode != 0o160000)
    {
        index_map.insert(e.path.clone(), (e.mode, e.oid));
    }

    let mut paths: Vec<Vec<u8>> = index_map
        .keys()
        .cloned()
        .chain(head.keys().cloned())
        .collect();
    paths.sort();
    paths.dedup();
    let paths: Vec<&Vec<u8>> = paths
        .iter()
        .filter(|p| keep(p, &prefix, &pathspecs))
        .collect();

    let mut out = Vec::new();
    for p in paths {
        let side = |mode: u32, oid: &[u8; 20]| -> Result<(u32, String, Vec<u8>)> {
            let (_, content) = store.read_object(&crate::commands::status::hex(oid))?;
            Ok((mode, crate::commands::status::hex(oid), content))
        };
        let (old_mode, old_oid, old_content) = if cached {
            match head.get(p) {
                Some((m, oid)) => side(*m, oid)?,
                None => (0, zero_oid(), Vec::new()),
            }
        } else {
            match index_map.get(p) {
                Some((m, oid)) => side(*m, oid)?,
                // Staged deletion: the path is in HEAD but no longer in the
                // index; worktree diff has no old side for it (git skips it).
                None => (0, zero_oid(), Vec::new()),
            }
        };
        let (new_mode, new_oid, new_content) = if cached {
            match index_map.get(p) {
                Some((m, oid)) => side(*m, oid)?,
                None => (0, zero_oid(), Vec::new()),
            }
        } else {
            worktree_side(&root, p)?
        };
        if old_mode == new_mode && old_content == new_content {
            continue;
        }
        if old_mode == 0o160000 || new_mode == 0o160000 {
            continue; // gitlink, v1 skips (D-014)
        }
        let a = quote_two("a", p);
        let b = quote_two("b", p);
        let mut f = FileDiff {
            hdr_old: a.clone(),
            hdr_new: b.clone(),
            body_old: if old_mode == 0 { "/dev/null".into() } else { a },
            body_new: if new_mode == 0 { "/dev/null".into() } else { b },
            old_oid,
            new_oid,
            old_mode,
            new_mode,
            binary: is_binary(&old_content) || is_binary(&new_content),
            hunks: Vec::new(),
        };
        if !f.binary {
            let old_lines = split_lines(&old_content);
            let new_lines = split_lines(&new_content);
            f.hunks = diff_lines(&old_lines, &new_lines);
        }
        out.extend_from_slice(&render(&f));
    }
    std::io::stdout()
        .lock()
        .write_all(&out)
        .context("stdout", "write diff output")?;
    Ok(())
}

/// The worktree side of a diff: (stat mode, blob hash, content), or all
/// zeros/empty when the file is gone (deleted).
fn worktree_side(root: &Path, rel: &[u8]) -> Result<(u32, String, Vec<u8>)> {
    let abs = root.join(String::from_utf8_lossy(rel).replace('/', std::path::MAIN_SEPARATOR_STR));
    let md = match std::fs::symlink_metadata(&abs) {
        Ok(md) => md,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((0, zero_oid(), Vec::new()));
        }
        Err(e) => return Err(GitError::io(abs.display().to_string(), "stat file", e)),
    };
    if md.is_dir() {
        return Ok((0, zero_oid(), Vec::new()));
    }
    let content = crate::worktree::blob_content(&abs)?;
    let mode = stat_file(&abs)?.mode;
    Ok((mode, ObjectStore::hash(Kind::Blob, &content), content))
}

/// Does `path` (repo-relative) pass the cwd-prefix filter and the
/// `-- <paths>` pathspecs? Pathspecs are cwd-relative, plain prefix matches
/// (`/x` is repo-root-relative, `.` matches everything under cwd).
fn keep(path: &[u8], prefix: &[u8], pathspecs: &[String]) -> bool {
    let under = |base: &[u8], p: &[u8]| {
        base.is_empty()
            || p == base
            || (p.len() > base.len() && p.starts_with(base) && p[base.len()] == b'/')
    };
    if !under(prefix, path) {
        return false;
    }
    if pathspecs.is_empty() {
        return true;
    }
    for spec in pathspecs {
        let spec = spec.strip_prefix('/').unwrap_or(spec);
        let mut base = prefix.to_vec();
        if !base.is_empty() {
            base.push(b'/');
        }
        if spec != "." {
            base.extend_from_slice(spec.as_bytes());
        }
        if under(&base, path) {
            return true;
        }
    }
    false
}

/// Map of path -> (mode, oid) for every blob in HEAD's tree.
fn load_head_tree(store: &ObjectStore, refs: &Refs) -> Result<ModeOidMap> {
    let mut out = HashMap::new();
    let Some(head) = refs.resolve("HEAD")? else {
        return Ok(out);
    };
    let (kind, content) = store.read_object(&head)?;
    if kind != Kind::Commit {
        return Ok(out);
    }
    let commit = Commit::parse(&content)?;
    let (_, tcontent) = store.read_object(&crate::commands::status::hex(&commit.tree))?;
    collect_tree(store, &tcontent, &mut Vec::new(), &mut out)?;
    Ok(out)
}

fn collect_tree(
    store: &ObjectStore,
    content: &[u8],
    prefix: &mut Vec<u8>,
    out: &mut HashMap<Vec<u8>, (u32, [u8; 20])>,
) -> Result<()> {
    let tree = Tree::parse(content)?;
    for e in tree.entries {
        if e.mode == 0o040000 {
            let len = prefix.len();
            prefix.extend_from_slice(&e.name);
            prefix.push(b'/');
            let (_, sub) = store.read_object(&crate::commands::status::hex(&e.oid))?;
            collect_tree(store, &sub, prefix, out)?;
            prefix.truncate(len);
        } else {
            let mut path = prefix.clone();
            path.extend_from_slice(&e.name);
            out.insert(path, (e.mode, e.oid));
        }
    }
    Ok(())
}

fn zero_oid() -> String {
    "0".repeat(40)
}

/// `<prefix>/<path>` (e.g. `a/x.txt`), quoted like git's `quote_two` with
/// `CQUOTE_NODQ` (diff.c): the whole label is wrapped in one pair of quotes
/// only when a byte needs escaping (`< 0x20`, `0x7f`, `"`, `\`, or `> 0x7f`
/// with core.quotepath on). Spaces stay literal and unquoted — unlike
/// status's C-quoting.
fn quote_two(prefix: &str, path: &[u8]) -> String {
    let needs = path
        .iter()
        .any(|&b| b < 0x20 || b == 0x7f || b == b'"' || b == b'\\' || b > 0x7f);
    if !needs {
        return format!("{prefix}/{}", String::from_utf8_lossy(path));
    }
    let mut s = format!("\"{prefix}/");
    for &b in path {
        match b {
            b'\t' => s.push_str("\\t"),
            b'\n' => s.push_str("\\n"),
            b'"' => s.push_str("\\\""),
            b'\\' => s.push_str("\\\\"),
            0x7f => s.push_str("\\177"),
            _ if !(0x20..=0x7f).contains(&b) => s.push_str(&format!("\\{b:03o}")),
            _ => s.push(b as char),
        }
    }
    s.push('"');
    s
}
