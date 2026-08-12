//! Plumbing commands that talk to the object store directly.
//!
//! v1 houses `hash-object` and `cat-file` here; ls-tree, update-ref, and the
//! rest of the plumbing join this file as they are implemented.

use std::fs;
use std::io::{Read, Write};

use crate::error::{GitError, IoContext, Result};
use crate::store::{Kind, ObjectStore};

/// `git-rs hash-object [-w] [--stdin] <file>`
///
/// Compute the blob object id for a file (or stdin with `--stdin`).
/// With `-w`, write the object into the store first.
pub fn run_hash_object(args: &[String]) -> Result<()> {
    let write = args.iter().any(|a| a == "-w");
    let from_stdin = args.iter().any(|a| a == "--stdin");
    let files: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    if from_stdin && !files.is_empty() {
        return Err(GitError::Invalid(
            "hash-object: --stdin cannot be combined with file arguments".into(),
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
            ))
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
/// Print the type, size, or content of an object. `-p` prints raw bytes
/// (blob content verbatim; tree/commit pretty-printing arrives with the
/// object-type steps).
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
                )))
            }
            s if s.starts_with('-') => {
                return Err(GitError::Invalid(format!("cat-file: unknown option '{arg}'")))
            }
            s => object = Some(s),
        }
    }

    let flags = [want_type, want_size, want_pretty].iter().filter(|f| **f).count();
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
    } else {
        std::io::stdout()
            .write_all(&content)
            .context("<stdout>", "write object content")?;
    }
    Ok(())
}
