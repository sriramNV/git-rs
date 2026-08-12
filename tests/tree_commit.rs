//! Tree/commit/tag integration tests: our serialized objects must produce
//! the same shas real git produces for identical input (`git mktree`,
//! `git commit-tree`, annotated tags), and we must parse real git's
//! objects back byte-identically.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use git_rs::object::{Commit, Ident, Object, Tag, Tree, TreeEntry};
use git_rs::store::{Kind, ObjectStore};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn scratch_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "git-rs-tree-{}-{name}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

fn git(dir: &PathBuf, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run real git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git_stdin(dir: &PathBuf, args: &[&str], stdin: &str) -> String {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn real git");
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A real git repo whose objects we create and compare against.
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = scratch_dir("repo");
        fs::create_dir_all(&dir).unwrap();
        let init = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(init.status.success(), "git init failed");
        Fixture { dir }
    }

    fn git(&self, args: &[&str]) -> String {
        git(&self.dir, args)
    }

    fn git_stdin(&self, args: &[&str], stdin: &str) -> String {
        git_stdin(&self.dir, args, stdin)
    }

    fn store(&self) -> ObjectStore {
        ObjectStore::new(self.dir.join(".git").join("objects"))
    }

    fn fsck_clean(&self) {
        let out = Command::new("git")
            .args(["fsck", "--strict"])
            .current_dir(&self.dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git fsck failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn raw_oid(hex: &str) -> [u8; 20] {
    let mut oid = [0u8; 20];
    for i in 0..20 {
        oid[i] = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
    }
    oid
}

/// Blob ids via real git hash-object -w.
fn blobs(fixture: &Fixture, contents: &[(&str, &str)]) -> Vec<String> {
    contents
        .iter()
        .map(|(name, content)| {
            let mut out = Command::new("git")
                .args(["hash-object", "-w", "--stdin"])
                .current_dir(&fixture.dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            use std::io::Write;
            out.stdin
                .take()
                .unwrap()
                .write_all(content.as_bytes())
                .unwrap();
            let out = out.wait_with_output().unwrap();
            assert!(out.status.success(), "hash-object failed");
            let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
            assert!(!id.is_empty(), "hash-object for {name} produced no id");
            id
        })
        .collect()
}

#[test]
fn tree_sha_matches_git_mktree() {
    let fixture = Fixture::new();
    let ids = blobs(
        &fixture,
        &[("a.txt", "hello\n"), ("run.sh", "#!/bin/sh\necho hi\n")],
    );

    // Real git mktree with the same entries.
    let mktree_input = format!(
        "100644 blob {}\ta.txt\n100755 blob {}\trun.sh\n",
        ids[0], ids[1]
    );
    let real_sha = fixture.git_stdin(&["mktree"], &mktree_input);

    // Our tree: same entries (unsorted; serialize must sort them).
    let our_bytes = Tree {
        entries: vec![
            TreeEntry {
                mode: 0o100755,
                name: b"run.sh".to_vec(),
                oid: raw_oid(&ids[1]),
            },
            TreeEntry {
                mode: 0o100644,
                name: b"a.txt".to_vec(),
                oid: raw_oid(&ids[0]),
            },
        ],
    }
    .serialize()
    .unwrap();
    let our_sha = ObjectStore::hash(Kind::Tree, &our_bytes);
    assert_eq!(our_sha, real_sha, "tree sha must match git mktree");

    // Write our tree into the store; git fsck must accept it, and parsing
    // the real git tree back must round-trip byte-identically.
    fixture
        .store()
        .write_object(Kind::Tree, &our_bytes)
        .unwrap();
    fixture.fsck_clean();
    let (kind, content) = fixture.store().read_object(&real_sha).unwrap();
    assert_eq!(kind, Kind::Tree);
    let parsed = Tree::parse(&content).unwrap();
    assert_eq!(parsed.serialize().unwrap(), content);
}

#[test]
fn nested_tree_matches_git_mktree() {
    let fixture = Fixture::new();
    let ids = blobs(&fixture, &[("inner.txt", "x\n")]);

    // sub/ contains inner.txt; root contains sub/ and top.txt.
    let sub_input = format!("100644 blob {}\tinner.txt\n", ids[0]);
    let sub_sha = fixture.git_stdin(&["mktree"], &sub_input);

    let mut entries = vec![
        TreeEntry {
            mode: 0o040000,
            name: b"sub".to_vec(),
            oid: raw_oid(&sub_sha),
        },
        TreeEntry {
            mode: 0o100644,
            name: b"top.txt".to_vec(),
            oid: raw_oid(&ids[0]),
        },
    ];
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let root_input = format!(
        "040000 tree {}\tsub\n100644 blob {}\ttop.txt\n",
        sub_sha, ids[0]
    );
    let real_sha = fixture.git_stdin(&["mktree"], &root_input);
    let our_bytes = Tree { entries }.serialize().unwrap();
    assert_eq!(ObjectStore::hash(Kind::Tree, &our_bytes), real_sha);

    // Round-trip: parse the real git tree, entries are already sorted.
    fixture
        .store()
        .write_object(Kind::Tree, &our_bytes)
        .unwrap();
    let (_, content) = fixture.store().read_object(&real_sha).unwrap();
    let parsed = Tree::parse(&content).unwrap();
    assert_eq!(parsed.entries.len(), 2);
    assert_eq!(parsed.entries[0].name, b"sub");
    assert_eq!(parsed.entries[0].mode, 0o040000);
    assert_eq!(parsed.serialize().unwrap(), content);
    fixture.fsck_clean();
}

fn fixed_ident<'a>(name: &'a str, email: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![
        ("GIT_AUTHOR_NAME", name),
        ("GIT_AUTHOR_EMAIL", email),
        ("GIT_AUTHOR_DATE", "1700000000 +0530"),
        ("GIT_COMMITTER_NAME", name),
        ("GIT_COMMITTER_EMAIL", email),
        ("GIT_COMMITTER_DATE", "1700000000 +0530"),
    ]
}

fn our_ident(name: &str, email: &str) -> Ident {
    Ident::new(name, email, 1700000000, 530).unwrap()
}

#[test]
fn commit_sha_matches_git_commit_tree() {
    let fixture = Fixture::new();
    let ids = blobs(&fixture, &[("a.txt", "hello\n")]);
    let tree_sha = fixture.git_stdin(&["mktree"], &format!("100644 blob {}\ta.txt\n", ids[0]));

    // Real git commit-tree with pinned identity (no parents).
    let out = Command::new("git")
        .args(["commit-tree", &tree_sha, "-m", "subject\n\nbody"])
        .current_dir(&fixture.dir)
        .envs(fixed_ident("A U Thor", "a@example.com"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "commit-tree failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let real_sha = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // Our commit with identical input.
    let our_commit = Commit {
        tree: raw_oid(&tree_sha),
        parents: vec![],
        author: our_ident("A U Thor", "a@example.com"),
        committer: our_ident("A U Thor", "a@example.com"),
        message: b"subject\n\nbody\n".to_vec(),
    };
    let our_bytes = our_commit.serialize().unwrap();
    assert_eq!(
        ObjectStore::hash(Kind::Commit, &our_bytes),
        real_sha,
        "commit sha must match git commit-tree"
    );

    // With a parent: our parents list must reproduce git's sha too.
    let out = Command::new("git")
        .args(["commit-tree", &tree_sha, "-p", &real_sha, "-m", "second"])
        .current_dir(&fixture.dir)
        .envs(fixed_ident("A U Thor", "a@example.com"))
        .output()
        .unwrap();
    assert!(out.status.success(), "commit-tree -p failed");
    let real_second = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let our_second = Commit {
        tree: raw_oid(&tree_sha),
        parents: vec![raw_oid(&real_sha)],
        author: our_ident("A U Thor", "a@example.com"),
        committer: our_ident("A U Thor", "a@example.com"),
        message: b"second\n".to_vec(),
    };
    assert_eq!(
        ObjectStore::hash(Kind::Commit, &our_second.serialize().unwrap()),
        real_second,
        "commit with parent must match git"
    );

    // Store our commit; real git parses it; we parse git's commit back.
    fixture
        .store()
        .write_object(Kind::Commit, &our_second.serialize().unwrap())
        .unwrap();
    fixture.fsck_clean();
    let (kind, content) = fixture.store().read_object(&real_second).unwrap();
    assert_eq!(kind, Kind::Commit);
    let parsed = Commit::parse(&content).unwrap();
    assert_eq!(parsed, our_second);
    assert_eq!(parsed.serialize().unwrap(), content);
}

#[test]
fn tag_sha_matches_git_annotated_tag() {
    let fixture = Fixture::new();
    let ids = blobs(&fixture, &[("a.txt", "hello\n")]);
    let tree_sha = fixture.git_stdin(&["mktree"], &format!("100644 blob {}\ta.txt\n", ids[0]));
    let out = Command::new("git")
        .args(["commit-tree", &tree_sha, "-m", "c1"])
        .current_dir(&fixture.dir)
        .envs(fixed_ident("A U Thor", "a@example.com"))
        .output()
        .unwrap();
    assert!(out.status.success());
    let commit_sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    fixture.git(&["update-ref", "refs/heads/master", &commit_sha]);

    // Real annotated tag with pinned tagger identity.
    let out = Command::new("git")
        .args(["tag", "-a", "v1.0", "-m", "release notes"])
        .current_dir(&fixture.dir)
        .envs(fixed_ident("T Ag", "t@example.com"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git tag failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let real_sha = fixture.git(&["rev-parse", "v1.0"]);

    // Our tag with identical input. git tag appends a trailing newline to
    // the message.
    let our_tag = Tag {
        object: raw_oid(&commit_sha),
        obj_type: "commit".into(),
        name: "v1.0".into(),
        tagger: our_ident("T Ag", "t@example.com"),
        message: b"release notes\n".to_vec(),
    };
    let our_bytes = our_tag.serialize().unwrap();
    assert_eq!(
        ObjectStore::hash(Kind::Tag, &our_bytes),
        real_sha,
        "tag sha must match git tag -a"
    );

    // Store ours, parse git's back.
    fixture.store().write_object(Kind::Tag, &our_bytes).unwrap();
    fixture.fsck_clean();
    let (kind, content) = fixture.store().read_object(&real_sha).unwrap();
    assert_eq!(kind, Kind::Tag);
    let parsed = Tag::parse(&content).unwrap();
    assert_eq!(parsed, our_tag);
    assert_eq!(parsed.serialize().unwrap(), content);
}

#[test]
fn cat_file_and_object_dispatch_agree_with_git() {
    let fixture = Fixture::new();
    let ids = blobs(
        &fixture,
        &[("a.txt", "hello\n"), ("run.sh", "#!/bin/sh\necho hi\n")],
    );
    let mktree_input = format!(
        "100644 blob {}\ta.txt\n100755 blob {}\trun.sh\n",
        ids[0], ids[1]
    );
    let tree_sha = fixture.git_stdin(&["mktree"], &mktree_input);
    let out = Command::new("git")
        .args(["commit-tree", &tree_sha, "-m", "c1"])
        .current_dir(&fixture.dir)
        .envs(fixed_ident("A U Thor", "a@example.com"))
        .output()
        .unwrap();
    let commit_sha = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // Object dispatch: parse every kind real git wrote.
    for (id, expected_kind) in [
        (&ids[0], Kind::Blob),
        (&tree_sha, Kind::Tree),
        (&commit_sha, Kind::Commit),
    ] {
        let (kind, content) = fixture.store().read_object(id).unwrap();
        assert_eq!(kind, expected_kind);
        let obj = Object::parse(kind, &content).unwrap();
        assert_eq!(obj.kind(), expected_kind);
        // Serialization must reproduce the exact raw bytes git stored.
        assert_eq!(obj.serialize().unwrap(), content);
    }
}

#[test]
fn cat_file_p_pretty_prints_tree_like_git() {
    let fixture = Fixture::new();
    let ids = blobs(
        &fixture,
        &[("a.txt", "hello\n"), ("run.sh", "#!/bin/sh\necho hi\n")],
    );
    let mktree_input = format!(
        "100644 blob {}\ta.txt\n100755 blob {}\trun.sh\n",
        ids[0], ids[1]
    );
    let tree_sha = fixture.git_stdin(&["mktree"], &mktree_input);

    // Real git cat-file -p output for the tree.
    let real = fixture.git(&["cat-file", "-p", &tree_sha]);

    // Our cat-file -p via the binary.
    let our = Command::new(env!("CARGO_BIN_EXE_git-rs"))
        .args(["cat-file", "-p", &tree_sha])
        .current_dir(&fixture.dir)
        .output()
        .unwrap();
    assert!(
        our.status.success(),
        "our cat-file failed: {}",
        String::from_utf8_lossy(&our.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&our.stdout), real);
}
