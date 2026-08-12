//! Plumbing commands that talk to the object store directly.
//!
//! v1 houses `hash-object`, `cat-file`, and `update-ref` here; ls-tree and
//! the rest of the plumbing join this file as they are implemented.

use std::fs;
use std::io::{Read, Write};

use crate::error::{GitError, IoContext, Result};
use crate::refs::Refs;
use crate::store::{Kind, ObjectStore};

/// `git-rs hash-object [-w] [--stdin] <file>`
///
/// Compute the blob object id for a file (or stdin with `--stdin`).
/// With `-w`, write the object into the store first.
pub fn run_hash_object(args: &[String]) -> Result<()> {
    let mut write = false;
    let mut from_stdin = false;
    let mut files: Vec<&String> = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-w" => write = true,
            "--stdin" => from_stdin = true,
            s if s.starts_with('-') => {
                return Err(GitError::Invalid(format!(
                    "hash-object: unknown option '{arg}'"
                )));
            }
            _ => files.push(arg),
        }
    }
    if from_stdin && !files.is_empty() {
        return Err(GitError::Invalid(
            "hash-object: --stdin cannot be combined with file arguments".into(),
        ));
    }
    if files.len() > 1 {
        return Err(GitError::Invalid(
            "hash-object: multiple files not implemented in v1".into(),
        ));
    }

    let store = ObjectStore::discover()?;
    if from_stdin {
        let mut content = Vec::new();
        std::io::stdin()
            .read_to_end(&mut content)
            .context("<stdin>", "read stdin")?;
        let id = if write {
            store.write_blob(&content)?
        } else {
            ObjectStore::hash(Kind::Blob, &content)
        };
        println!("{id}");
        return Ok(());
    }

    let path = match files.first() {
        Some(p) => p.as_str(),
        None => {
            return Err(GitError::Invalid(
                "usage: git-rs hash-object [-w] [--stdin] <file>".into(),
            ));
        }
    };
    let content = fs::read(path).context(path, "read file")?;
    let id = if write {
        store.write_blob(&content)?
    } else {
        ObjectStore::hash(Kind::Blob, &content)
    };
    println!("{id}");
    Ok(())
}

/// `git-rs cat-file (-t | -s | -p) <object>`
///
/// Print the type, size, or content of an object. `-p` prints pretty:
/// blob/commit/tag content verbatim, trees in ls-tree format
/// (`<mode> <type> <sha>\t<name>`) — matching real git.
pub fn run_cat_file(args: &[String]) -> Result<()> {
    let mut want_type = false;
    let mut want_size = false;
    let mut want_pretty = false;
    let mut object = None;

    for arg in args {
        match arg.as_str() {
            "-t" => want_type = true,
            "-s" => want_size = true,
            "-p" => want_pretty = true,
            "-e" | "-c" | "--batch" | "--batch-check" => {
                return Err(GitError::Invalid(format!(
                    "cat-file: option '{arg}' not implemented in v1"
                )));
            }
            s if s.starts_with('-') => {
                return Err(GitError::Invalid(format!(
                    "cat-file: unknown option '{arg}'"
                )));
            }
            s => object = Some(s),
        }
    }

    let flags = [want_type, want_size, want_pretty]
        .iter()
        .filter(|f| **f)
        .count();
    if flags != 1 {
        return Err(GitError::Invalid(
            "usage: git-rs cat-file (-t | -s | -p) <object>".into(),
        ));
    }
    let id = object.ok_or_else(|| {
        GitError::Invalid("usage: git-rs cat-file (-t | -s | -p) <object>".into())
    })?;

    let store = ObjectStore::discover()?;
    let (kind, content) = store.read_object(id)?;

    if want_type {
        println!("{}", kind.as_str());
    } else if want_size {
        println!("{}", content.len());
    } else if kind == Kind::Tree {
        print_tree(&content)?;
    } else {
        std::io::stdout()
            .write_all(&content)
            .context("<stdout>", "write object content")?;
    }
    Ok(())
}

/// Pretty-print a tree like `git ls-tree` / `git cat-file -p`:
/// `<6-digit octal mode> <type> <sha>\t<name>` per entry, sorted (git
/// stores trees sorted, so stored order is fine).
fn print_tree(content: &[u8]) -> Result<()> {
    use crate::object::Tree;
    let tree = Tree::parse(content)?;
    let mut out = Vec::new();
    for (i, e) in tree.entries.iter().enumerate() {
        out.extend_from_slice(format!("{:06o}", e.mode).as_bytes());
        out.push(b' ');
        let kind = match e.mode {
            0o040000 => "tree",
            0o160000 => "commit",
            _ => "blob",
        };
        out.extend_from_slice(kind.as_bytes());
        out.push(b' ');
        for b in &e.oid {
            out.extend_from_slice(format!("{b:02x}").as_bytes());
        }
        out.push(b'\t');
        out.extend_from_slice(&e.name);
        if i + 1 < tree.entries.len() {
            out.push(b'\n');
        }
    }
    std::io::stdout()
        .write_all(&out)
        .context("<stdout>", "write tree content")
}

/// `git-rs update-ref [-m <reason>] <ref> <new> [<old>]`
///
/// Create or update a ref, atomically. With `<old>`, the update only
/// happens if the ref currently points at `<old>` (compare-and-swap).
/// Writes a reflog entry when `core.logallrefupdates` is enabled.
///
/// Real git lowercases object names; malformed shas get the classic
/// `not a valid [old] SHA1` fatal (exit 128) — probe-verified against
/// git 2.55.
pub fn run_update_ref(args: &[String]) -> Result<()> {
    let mut message: Option<String> = None;
    let mut positional: Vec<&String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-m" => {
                i += 1;
                let reason = args
                    .get(i)
                    .ok_or_else(|| {
                        GitError::Invalid("update-ref: option '-m' requires a reason".into())
                    })?
                    .clone();
                if reason.is_empty() {
                    return Err(GitError::Invalid(
                        "usage: git-rs update-ref [-m <reason>] <ref> <new> [<old>]".into(),
                    ));
                }
                message = Some(reason);
            }
            s if s.starts_with('-') => {
                return Err(GitError::Invalid(format!(
                    "update-ref: unknown option '{s}'"
                )));
            }
            _ => positional.push(&args[i]),
        }
        i += 1;
    }
    let (name, new_sha) = match (positional.first(), positional.get(1)) {
        (Some(name), Some(new)) => (name.as_str(), new.as_str()),
        _ => {
            return Err(GitError::Invalid(
                "usage: git-rs update-ref [-m <reason>] <ref> <new> [<old>]".into(),
            ));
        }
    };
    let old = positional.get(2).map(|s| s.as_str());

    let new_sha = new_sha.to_ascii_lowercase();
    if !is_40_hex(&new_sha) {
        return Err(GitError::Fatal(format!("{new_sha}: not a valid SHA1")));
    }
    let old = old.map(str::to_ascii_lowercase);
    if let Some(old) = &old
        && !is_40_hex(old)
    {
        return Err(GitError::Fatal(format!("{old}: not a valid old SHA1")));
    }

    let refs = Refs::discover()?;
    if let Some(old) = &old {
        let current = refs.resolve(name)?;
        // git semantics: old == ZERO_SHA means "must not exist" (create-only).
        if old == crate::refs::ZERO_SHA {
            if current.is_some() {
                return Err(GitError::Fatal(format!(
                    "update_ref failed for ref '{name}': cannot lock ref '{name}': reference already exists"
                )));
            }
        } else {
            match current {
                Some(actual) if actual == *old => {}
                Some(actual) => {
                    return Err(GitError::Fatal(format!(
                        "update_ref failed for ref '{name}': cannot lock ref '{name}': is at {actual} but expected {old}"
                    )));
                }
                None => {
                    return Err(GitError::Fatal(format!(
                        "update_ref failed for ref '{name}': cannot lock ref '{name}': unable to resolve reference '{name}'"
                    )));
                }
            }
        }
    }
    refs.update(name, &new_sha, message.as_deref().unwrap_or(""))
}

/// A 40-hex sha, case-insensitive (git lowercases object names).
fn is_40_hex(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}
