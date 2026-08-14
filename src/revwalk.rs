//! Revision walking: seed a walk at one or more commits and yield them in
//! committer-date order (newest first), de-duplicating across branches.
//!
//! Also holds the shared revision-resolution helpers used by `log` and
//! `show`: name → commit resolution, the "unborn branch" fatal, and git's
//! `ambiguous argument` block.

use std::collections::{BinaryHeap, HashSet};

use crate::error::{GitError, Result};
use crate::object::{Commit, Tag};
use crate::refs::Refs;
use crate::store::{Kind, ObjectStore};

/// Heap item: `(committer_ts, sha)`, largest ts pops first (BinaryHeap is a
/// max-heap). Ties break by larger sha — deterministic, and irrelevant for
/// histories with distinct dates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Item(i64, [u8; 20]);

/// A commit walker. Seeds are commits; parents are pushed as walked.
pub struct Revwalk {
    store: ObjectStore,
    heap: BinaryHeap<Item>,
    seen: HashSet<[u8; 20]>,
    /// Remaining commits to yield (`-n`); `None` = unlimited.
    limit: Option<usize>,
}

impl Revwalk {
    pub fn new(store: ObjectStore) -> Self {
        Revwalk {
            store,
            heap: BinaryHeap::new(),
            seen: HashSet::new(),
            limit: None,
        }
    }

    /// Cap the number of commits yielded (`log -n <k>`).
    pub fn set_limit(&mut self, n: usize) {
        self.limit = Some(n);
    }

    /// Add a seed commit (tags peel to their commit). Re-seeding an already
    /// seen commit is a no-op.
    pub fn seed(&mut self, sha: [u8; 20]) -> Result<()> {
        if !self.seen.insert(sha) {
            return Ok(());
        }
        let (kind, content) = self.store.read_object(&hex(&sha))?;
        match kind {
            Kind::Commit => {
                let commit = Commit::parse(&content)?;
                self.heap.push(Item(commit.committer.ts, sha));
            }
            Kind::Tag => {
                let tag = Tag::parse(&content)?;
                self.seed(tag.object)?;
            }
            _ => {
                return Err(GitError::Corrupt(format!(
                    "{} is not a commit or tag",
                    hex(&sha)
                )));
            }
        }
        Ok(())
    }

    /// Pop the next commit in committer-date order, or `None` when the walk
    /// is exhausted or the `-n` limit is reached.
    pub fn pop_next(&mut self) -> Result<Option<[u8; 20]>> {
        if self.limit == Some(0) {
            return Ok(None);
        }
        let Some(item) = self.heap.pop() else {
            return Ok(None);
        };
        if let Some(n) = self.limit.as_mut() {
            *n -= 1;
        }
        let (_kind, content) = self.store.read_object(&hex(&item.1))?;
        let commit = Commit::parse(&content)?;
        for parent in &commit.parents {
            if !self.seen.contains(parent) {
                // Parents are commits by construction; date already read.
                self.seen.insert(*parent);
                let (_, pcontent) = self.store.read_object(&hex(parent))?;
                let p = Commit::parse(&pcontent)?;
                self.heap.push(Item(p.committer.ts, *parent));
            }
        }
        Ok(Some(item.1))
    }
}

/// Resolve a revision name to a commit sha: a full 40-hex name, `HEAD`, a
/// fully qualified ref, or `refs/heads/<name>` / `refs/tags/<name>`.
/// Tags peel to their commit. Returns `None` when nothing resolves.
pub fn resolve_rev(refs: &Refs, store: &ObjectStore, name: &str) -> Result<Option<[u8; 20]>> {
    if name.len() == 40 && name.bytes().all(|b| b.is_ascii_hexdigit()) {
        return match store.read_object(name) {
            Ok(_) => Ok(Some(parse_oid(name)?)),
            Err(GitError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        };
    }
    // `HEAD~N` / `<rev>~N`: Nth parent (first-parent walk). Nested ~ works
    // via recursion; walking past the root yields None (git: ambiguous
    // argument).
    if let Some((base, n_str)) = name.split_once('~') {
        let n: usize = n_str
            .parse()
            .map_err(|_| GitError::Invalid(format!("revision '{name}' does not exist")))?;
        let mut sha = match resolve_rev(refs, store, base)? {
            Some(s) => s,
            None => return Ok(None),
        };
        for _ in 0..n {
            let (kind, content) = store.read_object(&hex(&sha))?;
            if kind != Kind::Commit {
                return Ok(None);
            }
            let commit = Commit::parse(&content)?;
            match commit.parents.first() {
                Some(p) => sha = *p,
                None => return Ok(None),
            }
        }
        return Ok(Some(sha));
    }
    let candidates: Vec<String> = if name.starts_with("refs/") {
        vec![name.to_string()]
    } else {
        match name {
            // Only literal HEAD/@ resolve to HEAD; arbitrary names must NOT
            // fall back to HEAD (git: unknown revision / pathspec error).
            "HEAD" | "@" => vec!["HEAD".to_string()],
            _ => vec![format!("refs/heads/{name}"), format!("refs/tags/{name}")],
        }
    };
    let mut peeled = None;
    for c in &candidates {
        if let Some(sha) = refs.resolve(c)?
            && let Some(oid) = peel_to_commit(store, &sha)?
        {
            peeled = Some(oid);
            break;
        }
    }
    Ok(peeled)
}

/// Follow a ref value until it names a commit (tags peel; anything else is
/// `None` — the ref exists but is not a commit).
pub fn peel_to_commit(store: &ObjectStore, sha: &str) -> Result<Option<[u8; 20]>> {
    let mut current = sha.to_string();
    for _ in 0..10 {
        let oid = parse_oid(&current)?;
        let (kind, content) = match store.read_object(&current) {
            Ok(v) => v,
            Err(GitError::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        match kind {
            Kind::Commit => return Ok(Some(oid)),
            Kind::Tag => {
                let tag = Tag::parse(&content)?;
                current = hex(&tag.object);
            }
            _ => return Ok(None),
        }
    }
    Err(GitError::Corrupt(format!("tag chain too deep at '{sha}'")))
}

/// Best common ancestor of two commits, or `None` when the histories do not
/// share a root (unreachable from one another). Two-pass walk: mark every
/// ancestor of `a`, then walk `b`'s ancestry until a marked commit appears.
/// With multiple merge bases the first hit is returned (git's tie-break is
/// richer; v1 callers only test ancestry, where any hit is correct).
pub fn merge_base(store: &ObjectStore, a: [u8; 20], b: [u8; 20]) -> Result<Option<[u8; 20]>> {
    if a == b {
        return Ok(Some(a));
    }
    let mut marked: HashSet<[u8; 20]> = HashSet::new();
    let mut frontier: Vec<[u8; 20]> = vec![a];
    while let Some(sha) = frontier.pop() {
        if !marked.insert(sha) {
            continue;
        }
        let parents = commit_parents(store, sha)?;
        frontier.extend(parents);
    }
    frontier = vec![b];
    while let Some(sha) = frontier.pop() {
        if marked.contains(&sha) {
            return Ok(Some(sha));
        }
        frontier.extend(commit_parents(store, sha)?);
    }
    Ok(None)
}

/// Parent oids of a commit (the empty list for root commits).
fn commit_parents(store: &ObjectStore, sha: [u8; 20]) -> Result<Vec<[u8; 20]>> {
    let (kind, content) = store.read_object(&hex(&sha))?;
    match kind {
        Kind::Commit => Ok(Commit::parse(&content)?.parents),
        _ => Err(GitError::Corrupt(format!("{} is not a commit", hex(&sha)))),
    }
}

/// Git's `ambiguous argument` error block (exit 128). v1 checks revisions
/// only — a valid path would misreport, matching nobody because commit
/// pathspecs are not implemented.
pub fn object_name_error(rev: &str) -> GitError {
    GitError::Fatal(format!(
        "ambiguous argument '{rev}': unknown revision or path not in the working tree.\n\
         Use '--' to separate paths from revisions, like this:\n\
         'git <command> [<revision>...] -- [<file>...]'"
    ))
}

/// Fatal for a missing HEAD: `your current branch '<branch>' does not have
/// any commits yet` (exit 128). `<branch>` is the symref target minus the
/// `refs/heads/` prefix; a non-symref HEAD reports `HEAD`.
pub fn unborn_fatal(refs: &Refs) -> GitError {
    let branch = refs.head_branch().unwrap_or_else(|| "HEAD".to_string());
    GitError::Fatal(format!(
        "your current branch '{branch}' does not have any commits yet"
    ))
}

fn parse_oid(sha: &str) -> Result<[u8; 20]> {
    let mut oid = [0u8; 20];
    for i in 0..20 {
        oid[i] = u8::from_str_radix(&sha[2 * i..2 * i + 2], 16)
            .map_err(|_| GitError::Corrupt(format!("bad sha '{sha}'")))?;
    }
    Ok(oid)
}

/// Lowercase hex of an oid ([u8; 20]).
pub fn hex(oid: &[u8; 20]) -> String {
    oid.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn short_oid(first: u8) -> [u8; 20] {
        let mut o = [0u8; 20];
        o[0] = first;
        o
    }

    #[test]
    fn hex_renders_lowercase() {
        assert_eq!(hex(&[0xab; 20]), "ab".repeat(20));
        assert_eq!(hex(&short_oid(0x0f)), format!("0f{}", "00".repeat(19)));
        assert_eq!(hex(&parse_oid(&"ab".repeat(20)).unwrap()), "ab".repeat(20));
    }

    #[test]
    fn parse_oid_rejects_bad_input() {
        assert!(parse_oid("xyz").is_err());
        assert!(parse_oid(&"g1".repeat(20)).is_err());
    }

    #[test]
    fn item_ordering_pops_newest_first() {
        let mut heap = BinaryHeap::new();
        heap.push(Item(10, short_oid(1)));
        heap.push(Item(20, short_oid(2)));
        heap.push(Item(15, short_oid(3)));
        let mut order = Vec::new();
        while let Some(i) = heap.pop() {
            order.push(i.1[0]);
        }
        assert_eq!(order, vec![2, 3, 1]);
    }

    #[test]
    fn object_name_error_matches_git() {
        let err = object_name_error("nope");
        assert!(format!("fatal: {err}").contains(
            "fatal: ambiguous argument 'nope': unknown revision or path not in the working tree."
        ));
        assert!(format!("{err}").contains("Use '--' to separate paths"));
    }

    fn write_commit(store: &ObjectStore, parents: &[[u8; 20]], first_byte: u8) -> [u8; 20] {
        let mut tree = [0u8; 20];
        tree[0] = first_byte;
        let commit = crate::object::Commit {
            tree,
            parents: parents.to_vec(),
            author: crate::object::Ident::new("A", "a@b", 1, 0).unwrap(),
            committer: crate::object::Ident::new("A", "a@b", 1, 0).unwrap(),
            message: vec![b'm'],
        };
        parse_oid(
            &store
                .write_object(Kind::Commit, &commit.serialize().unwrap())
                .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn merge_base_direct_ancestor() {
        let store = ObjectStore::new(temp_dir());
        let root = write_commit(&store, &[], 1);
        let child = write_commit(&store, &[root], 2);
        let grand = write_commit(&store, &[child], 3);
        assert_eq!(merge_base(&store, root, grand).unwrap(), Some(root));
        assert_eq!(merge_base(&store, child, grand).unwrap(), Some(child));
        assert_eq!(merge_base(&store, grand, child).unwrap(), Some(child));
    }

    #[test]
    fn merge_base_divergent_branches() {
        let store = ObjectStore::new(temp_dir());
        let root = write_commit(&store, &[], 1);
        let left = write_commit(&store, &[root], 2);
        let right = write_commit(&store, &[root], 3);
        assert_eq!(merge_base(&store, left, right).unwrap(), Some(root));
        assert_eq!(merge_base(&store, right, left).unwrap(), Some(root));
    }

    #[test]
    fn merge_base_same_commit() {
        let store = ObjectStore::new(temp_dir());
        let root = write_commit(&store, &[], 1);
        assert_eq!(merge_base(&store, root, root).unwrap(), Some(root));
    }

    #[test]
    fn merge_base_disconnected_returns_none() {
        let store = ObjectStore::new(temp_dir());
        let a = write_commit(&store, &[], 1);
        let b = write_commit(&store, &[], 2);
        assert_eq!(merge_base(&store, a, b).unwrap(), None);
    }

    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gitrs-revwalk-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
