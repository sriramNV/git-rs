//! Three-way merge algorithm (step 11): per-path merging of two trees
//! against their merge base, plus whole-file conflict-marker rendering.
//!
//! Resolution table (locked, probed against git 2.55): a path whose two
//! sides disagree takes the side that equals the base, survives when both
//! sides agree, and conflicts otherwise. There is no hunk-level auto-merge
//! (D-016): a conflicting file becomes one marker block spanning its whole
//! content. v1 has no rename detection and no exec-bit handling beyond the
//! modes already stored in trees.

use std::collections::BTreeMap;

use crate::error::Result;
use crate::store::ObjectStore;
use crate::worktree::tree_entries;

/// Why a path conflicts (drives the `CONFLICT (...)` message).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// Both sides modified the file to different content.
    Content,
    /// Both sides added the same path with different content.
    AddAdd,
    /// One side deleted the path, the other modified it.
    ModifyDelete,
}

/// One conflicting path with the three blobs `(mode, oid)`; a side is `None`
/// when the path is absent from it (add/add, modify/delete).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub path: Vec<u8>,
    pub kind: ConflictKind,
    pub base: Option<(u32, [u8; 20])>,
    pub ours: Option<(u32, [u8; 20])>,
    pub theirs: Option<(u32, [u8; 20])>,
}

/// A path's merge outcome, in path-byte order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeFile {
    /// Resolved: `oid` `None` = delete; `auto` = both sides changed it, so
    /// git prints an `Auto-merging` line even though it resolved.
    Resolved {
        path: Vec<u8>,
        mode: u32,
        oid: Option<[u8; 20]>,
        auto: bool,
    },
    Conflict(Conflict),
}

/// The full result of a three-way merge.
#[derive(Debug, Default)]
pub struct MergeResult {
    pub files: Vec<MergeFile>,
}

/// Merge two trees (`ours` against `theirs`, sharing `base`) into a list of
/// per-path resolutions. Paths identical in all three trees are omitted.
pub fn merge_trees(
    store: &ObjectStore,
    base: &str,
    ours: &str,
    theirs: &str,
) -> Result<MergeResult> {
    let b = collect(store, base)?;
    let o = collect(store, ours)?;
    let t = collect(store, theirs)?;

    let mut paths: Vec<Vec<u8>> = b.keys().chain(o.keys()).chain(t.keys()).cloned().collect();
    paths.sort();
    paths.dedup();

    let mut out = MergeResult::default();
    for path in paths {
        let bside = b.get(&path).copied();
        let oside = o.get(&path).copied();
        let tside = t.get(&path).copied();
        let (bid, oid_, tid) = (
            bside.map(|(_, x)| x),
            oside.map(|(_, x)| x),
            tside.map(|(_, x)| x),
        );
        match (bid, oid_, tid) {
            (Some(x), Some(y), Some(z)) if x == y && y == z => {} // untouched
            (Some(_), Some(y), Some(z)) if y == z => {
                // Both sides changed to the same content.
                out.files.push(resolved(path, oside, true));
            }
            (Some(x), Some(y), Some(_)) if x == y => {
                out.files.push(resolved(path, tside, false));
            }
            (Some(x), Some(_), Some(z)) if x == z => {
                out.files.push(resolved(path, oside, false));
            }
            (Some(_), Some(_), Some(_)) => out.files.push(MergeFile::Conflict(Conflict {
                path,
                kind: ConflictKind::Content,
                base: bside,
                ours: oside,
                theirs: tside,
            })),
            (Some(x), Some(y), None) if x == y => {
                // Only they deleted it.
                out.files.push(resolved(path, None, false));
            }
            (Some(_), Some(_), None) => out.files.push(MergeFile::Conflict(Conflict {
                path,
                kind: ConflictKind::ModifyDelete,
                base: bside,
                ours: oside,
                theirs: tside,
            })),
            (Some(x), None, Some(z)) if x == z => {
                // Only we deleted it.
                out.files.push(resolved(path, None, false));
            }
            (Some(_), None, Some(_)) => out.files.push(MergeFile::Conflict(Conflict {
                path,
                kind: ConflictKind::ModifyDelete,
                base: bside,
                ours: oside,
                theirs: tside,
            })),
            (None, Some(y), Some(z)) if y == z => {
                out.files.push(resolved(path, oside, false));
            }
            (None, Some(_), Some(_)) => out.files.push(MergeFile::Conflict(Conflict {
                path,
                kind: ConflictKind::AddAdd,
                base: bside,
                ours: oside,
                theirs: tside,
            })),
            (Some(_), None, None) => out.files.push(resolved(path, None, false)),
            (None, None, Some(_)) => out.files.push(resolved(path, tside, false)),
            (None, Some(_), None) => out.files.push(resolved(path, oside, false)),
            (None, None, None) => unreachable!("path absent from all three trees"),
        }
    }
    Ok(out)
}

fn resolved(path: Vec<u8>, side: Option<(u32, [u8; 20])>, auto: bool) -> MergeFile {
    match side {
        Some((mode, oid)) => MergeFile::Resolved {
            path,
            mode,
            oid: Some(oid),
            auto,
        },
        None => MergeFile::Resolved {
            path,
            mode: 0,
            oid: None,
            auto: false,
        },
    }
}

/// Flatten a tree into `(path bytes, (mode, oid))`.
#[allow(clippy::type_complexity)] // BTreeMap<Vec<u8>, (u32, [u8; 20])>
fn collect(store: &ObjectStore, tree: &str) -> Result<BTreeMap<Vec<u8>, (u32, [u8; 20])>> {
    let mut m = BTreeMap::new();
    for (path, mode, oid) in tree_entries(store, tree)? {
        m.insert(path, (mode, oid));
    }
    Ok(m)
}

/// Render the whole-file conflict markers, byte-identical to git's
/// (probed, git 2.55): sections joined by `\n`, a newline inserted after
/// content that lacks one, file ends with `>>>>>>> <label>\n`.
pub fn conflict_marker(ours: &[u8], theirs: &[u8], theirs_label: &str) -> Vec<u8> {
    let mut out: Vec<u8> = b"<<<<<<< HEAD\n".to_vec();
    out.extend_from_slice(ours);
    if !ours.ends_with(b"\n") {
        out.push(b'\n');
    }
    out.extend_from_slice(b"=======\n");
    out.extend_from_slice(theirs);
    if !theirs.ends_with(b"\n") {
        out.push(b'\n');
    }
    out.extend_from_slice(b">>>>>>> ");
    out.extend_from_slice(theirs_label.as_bytes());
    out.push(b'\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::tree::{Tree, TreeEntry};
    use crate::worktree::parse_oid;
    use std::env;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_store() -> ObjectStore {
        let dir = env::temp_dir().join(format!(
            "git-rs-merge-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        ObjectStore::new(dir)
    }

    /// Single- and multi-level tree from `(path, content)` pairs, modes 100644.
    fn tree_of(store: &ObjectStore, files: &[(&str, &str)]) -> String {
        let mut by_dir: std::collections::BTreeMap<String, Vec<(String, u32, [u8; 20])>> =
            std::collections::BTreeMap::new();
        for (path, content) in files {
            let oid = parse_oid(&store.write_blob(content.as_bytes()).unwrap()).unwrap();
            let dirs: Vec<&str> = path.split('/').collect();
            for i in 0..dirs.len().saturating_sub(1) {
                let parent = dirs[..i].join("/");
                by_dir
                    .entry(parent)
                    .or_default()
                    .push((dirs[i].to_string(), 0o040000, [0u8; 20]));
            }
            let entry = by_dir
                .entry(if dirs.len() > 1 {
                    dirs[..dirs.len() - 1].join("/")
                } else {
                    String::new()
                })
                .or_default();
            if !entry.iter().any(|(n, _, _)| n == dirs[dirs.len() - 1]) {
                entry.push((dirs[dirs.len() - 1].to_string(), 0o100644, oid));
            }
        }
        fn build(
            store: &ObjectStore,
            by_dir: &std::collections::BTreeMap<String, Vec<(String, u32, [u8; 20])>>,
            dir: &str,
        ) -> String {
            let mut entries: Vec<TreeEntry> = Vec::new();
            for (name, mode, oid) in by_dir.get(dir).into_iter().flatten() {
                let child_dir = if dir.is_empty() {
                    name.clone()
                } else {
                    format!("{dir}/{name}")
                };
                if by_dir.contains_key(&child_dir) {
                    entries.push(TreeEntry {
                        mode: 0o040000,
                        name: name.as_bytes().to_vec(),
                        oid: parse_oid(&build(store, by_dir, &child_dir)).unwrap(),
                    });
                } else {
                    entries.push(TreeEntry {
                        mode: *mode,
                        name: name.as_bytes().to_vec(),
                        oid: *oid,
                    });
                }
            }
            entries.sort_by(|a, b| a.name.cmp(&b.name));
            let tree = Tree { entries };
            store
                .write_object(crate::store::Kind::Tree, &tree.serialize().unwrap())
                .unwrap()
        }
        build(store, &by_dir, "")
    }

    fn merge_names(m: &MergeResult) -> Vec<&[u8]> {
        m.files
            .iter()
            .map(|f| match f {
                MergeFile::Resolved { path, .. } => path.as_slice(),
                MergeFile::Conflict(c) => c.path.as_slice(),
            })
            .collect()
    }

    #[test]
    fn disjoint_changes_merge_cleanly() {
        let store = temp_store();
        let base = tree_of(&store, &[("a.txt", "base"), ("b.txt", "base")]);
        let ours = tree_of(&store, &[("a.txt", "ours"), ("b.txt", "base")]);
        let theirs = tree_of(&store, &[("a.txt", "base"), ("b.txt", "theirs")]);
        let m = merge_trees(&store, &base, &ours, &theirs).unwrap();
        assert_eq!(
            merge_names(&m),
            vec![b"a.txt".as_slice(), b"b.txt".as_slice()]
        );
        let (take_ours, take_theirs) = match &m.files[..] {
            [
                MergeFile::Resolved {
                    path,
                    oid: Some(o),
                    auto: false,
                    ..
                },
                MergeFile::Resolved {
                    path: p2,
                    oid: Some(o2),
                    auto: false,
                    ..
                },
            ] if path == b"a.txt" && p2 == b"b.txt" => (
                store.read_object(&crate::revwalk::hex(o)).unwrap().1,
                store.read_object(&crate::revwalk::hex(o2)).unwrap().1,
            ),
            _ => panic!("unexpected resolution"),
        };
        assert_eq!(take_ours, b"ours");
        assert_eq!(take_theirs, b"theirs");
    }

    #[test]
    fn both_changed_identically_resolves_with_auto_flag() {
        let store = temp_store();
        let base = tree_of(&store, &[("a.txt", "base")]);
        let ours = tree_of(&store, &[("a.txt", "same")]);
        let theirs = tree_of(&store, &[("a.txt", "same")]);
        let m = merge_trees(&store, &base, &ours, &theirs).unwrap();
        assert_eq!(m.files.len(), 1);
        match &m.files[0] {
            MergeFile::Resolved {
                auto: true,
                oid: Some(_),
                ..
            } => {}
            other => panic!("expected auto-resolved, got {other:?}"),
        }
    }

    #[test]
    fn content_conflict_carries_all_three_blobs() {
        let store = temp_store();
        let base = tree_of(&store, &[("a.txt", "base")]);
        let ours = tree_of(&store, &[("a.txt", "ours")]);
        let theirs = tree_of(&store, &[("a.txt", "theirs")]);
        let m = merge_trees(&store, &base, &ours, &theirs).unwrap();
        assert_eq!(m.files.len(), 1);
        match &m.files[0] {
            MergeFile::Conflict(c) if c.kind == ConflictKind::Content => {
                assert!(c.base.is_some() && c.ours.is_some() && c.theirs.is_some());
            }
            other => panic!("expected content conflict, got {other:?}"),
        }
    }

    #[test]
    fn add_add_and_identical_adds() {
        let store = temp_store();
        let base = tree_of(&store, &[("keep", "k")]);
        let ours = tree_of(&store, &[("keep", "k"), ("new", "ours")]);
        let theirs = tree_of(&store, &[("keep", "k"), ("new", "theirs")]);
        let m = merge_trees(&store, &base, &ours, &theirs).unwrap();
        match &m.files[..] {
            [MergeFile::Conflict(c)] if c.kind == ConflictKind::AddAdd => {
                assert_eq!(c.path, b"new");
                assert!(c.base.is_none() && c.ours.is_some() && c.theirs.is_some());
                assert!(c.ours.as_ref().unwrap().1 != c.theirs.as_ref().unwrap().1);
                assert_eq!(c.ours.as_ref().unwrap().0, 0o100644);
            }
            other => panic!("expected add/add conflict, got {other:?}"),
        }
        let same = tree_of(&store, &[("keep", "k"), ("new", "same")]);
        let m2 = merge_trees(&store, &base, &same, &same).unwrap();
        match &m2.files[..] {
            [
                MergeFile::Resolved {
                    oid: Some(_),
                    auto: false,
                    ..
                },
            ] => {}
            other => panic!("expected clean both-added-same, got {other:?}"),
        }
        let ours2 = tree_of(&store, &[("keep", "k"), ("new", "both")]);
        let m3 = merge_trees(&store, &base, &ours2, &same).unwrap();
        match &m3.files[..] {
            [MergeFile::Conflict(c)] if c.kind == ConflictKind::AddAdd => {}
            other => panic!("expected add/add conflict, got {other:?}"),
        }
    }

    #[test]
    fn modify_delete_conflicts_in_both_directions() {
        let store = temp_store();
        let base = tree_of(&store, &[("a.txt", "base")]);
        // They deleted, we modified: ours present, theirs absent.
        let ours = tree_of(&store, &[("a.txt", "ours")]);
        let theirs = tree_of(&store, &[]);
        let m = merge_trees(&store, &base, &ours, &theirs).unwrap();
        match &m.files[..] {
            [MergeFile::Conflict(c)] if c.kind == ConflictKind::ModifyDelete => {
                assert!(c.base.is_some() && c.ours.is_some() && c.theirs.is_none());
            }
            other => panic!("expected modify/delete, got {other:?}"),
        }
        // We deleted, they modified: ours absent, theirs present.
        let m2 = merge_trees(&store, &base, &theirs, &ours).unwrap();
        match &m2.files[..] {
            [MergeFile::Conflict(c)] if c.kind == ConflictKind::ModifyDelete => {
                assert!(c.base.is_some() && c.ours.is_none() && c.theirs.is_some());
            }
            other => panic!("expected modify/delete, got {other:?}"),
        }
    }

    #[test]
    fn delete_side_vs_unchanged_side_is_clean() {
        let store = temp_store();
        // They deleted a.txt, we did not touch it; we deleted b.txt, they
        // did not touch it; both deleted d.txt; c.txt is untouched in all
        // three trees (omitted entirely).
        let base = tree_of(
            &store,
            &[
                ("a.txt", "base"),
                ("b.txt", "base"),
                ("c.txt", "base"),
                ("d.txt", "base"),
            ],
        );
        let ours = tree_of(&store, &[("a.txt", "base"), ("c.txt", "base")]);
        let theirs = tree_of(&store, &[("b.txt", "base"), ("c.txt", "base")]);
        let m = merge_trees(&store, &base, &ours, &theirs).unwrap();
        assert_eq!(
            merge_names(&m),
            vec![
                b"a.txt".as_slice(),
                b"b.txt".as_slice(),
                b"d.txt".as_slice()
            ],
        );
        for f in &m.files {
            match f {
                MergeFile::Resolved {
                    oid: None,
                    auto: false,
                    ..
                } => {}
                other => panic!("expected clean delete, got {other:?}"),
            }
        }
    }

    #[test]
    fn added_by_one_side_only() {
        let store = temp_store();
        let base = tree_of(&store, &[("keep", "k")]);
        let ours = tree_of(&store, &[("keep", "k"), ("ours.txt", "o")]);
        let theirs = tree_of(&store, &[("keep", "k"), ("theirs.txt", "t")]);
        let m = merge_trees(&store, &base, &ours, &theirs).unwrap();
        assert_eq!(
            merge_names(&m),
            vec![b"ours.txt".as_slice(), b"theirs.txt".as_slice()]
        );
        assert!(m.files.iter().all(|f| matches!(
            f,
            MergeFile::Resolved {
                oid: Some(_),
                auto: false,
                ..
            }
        )));
    }

    #[test]
    fn unaffected_paths_are_omitted() {
        let store = temp_store();
        let base = tree_of(&store, &[("a.txt", "base"), ("b.txt", "base")]);
        let m = merge_trees(&store, &base, &base, &base).unwrap();
        assert!(m.files.is_empty());
    }

    #[test]
    fn nested_paths_use_path_order() {
        let store = temp_store();
        let base = tree_of(&store, &[("zz/one.txt", "1")]);
        let ours = tree_of(&store, &[("zz/one.txt", "ours")]);
        let theirs = tree_of(&store, &[("aa/two.txt", "2"), ("zz/one.txt", "1")]);
        let m = merge_trees(&store, &base, &ours, &theirs).unwrap();
        assert_eq!(
            merge_names(&m),
            vec![b"aa/two.txt".as_slice(), b"zz/one.txt".as_slice()]
        );
    }

    #[test]
    fn conflict_marker_bytes_match_git() {
        // Probed bytes (git 2.55) for content without trailing newlines:
        // <<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> nt\n
        assert_eq!(
            conflict_marker(b"ours", b"theirs", "nt"),
            b"<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> nt\n"
        );
        // Content with trailing newlines passes through verbatim (CRLF
        // included — bytes probed from git).
        assert_eq!(
            conflict_marker(b"mm-ours \r\n", b"mm-theirs \r\n", "mm"),
            b"<<<<<<< HEAD\nmm-ours \r\n=======\nmm-theirs \r\n>>>>>>> mm\n"
        );
        // Empty content still gets separators.
        assert_eq!(
            conflict_marker(b"", b"x", "b"),
            b"<<<<<<< HEAD\n\n=======\nx\n>>>>>>> b\n"
        );
    }
}
