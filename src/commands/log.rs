//! `git-rs log [--oneline] [-n <k>] [--all] [--graph]` — commit history.
//!
//! v1 prints the oneline format always (`<abbrev-sha> <subject>`; the full
//! header format with Author/Date is deferred, D-015), walking HEAD in
//! committer-date order like real git. `--all` seeds every branch, tag, and
//! HEAD; `-n` caps the count; `--graph` prefixes each line with `* ` (v1:
//! linear histories only — merge-corner glyphs land with merge support).
//! An unborn HEAD is fatal exactly like git (`your current branch 'main'
//! does not have any commits yet`), unless `--all` found nothing, which is
//! silent (exit 0, probed).

use crate::error::{GitError, Result};
use crate::object::Commit;
use crate::refs::Refs;
use crate::revwalk::{hex, resolve_rev, unborn_fatal, Revwalk};
use crate::store::{Kind, ObjectStore};

/// `git-rs log [--oneline] [-n <k>] [--all] [--graph]`
pub fn run_log(args: &[String]) -> Result<()> {
    let mut all = false;
    let mut graph = false;
    let mut n: Option<usize> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--oneline" => {}
            "--all" => all = true,
            "--graph" => graph = true,
            "-n" => {
                i += 1;
                let Some(raw) = args.get(i) else {
                    return Err(GitError::Invalid(
                        "log: option '-n' requires a count".into(),
                    ));
                };
                n = match raw.parse() {
                    Ok(v) => Some(v),
                    Err(_) => {
                        return Err(GitError::Invalid(format!("log: invalid count '{raw}'")));
                    }
                };
            }
            s if s.starts_with('-') => {
                return Err(GitError::Invalid(format!("log: unknown option '{s}'")));
            }
            s => {
                return Err(GitError::Invalid(format!(
                    "log: revision argument '{s}' not implemented in v1; use --all or nothing"
                )));
            }
        }
        i += 1;
    }

    let refs = Refs::discover()?;
    let store = ObjectStore::discover()?;
    let mut walk = Revwalk::new(store.clone());
    walk.set_limit(n.unwrap_or(usize::MAX));

    if all {
        for prefix in ["refs/heads", "refs/tags"] {
            for name in refs.list_names(prefix)? {
                if let Some(sha) = resolve_rev(&refs, &store, &name)? {
                    walk.seed(sha)?;
                }
            }
        }
        if let Some(sha) = resolve_rev(&refs, &store, "HEAD")? {
            walk.seed(sha)?;
        }
    } else {
        match resolve_rev(&refs, &store, "HEAD")? {
            Some(sha) => walk.seed(sha)?,
            None => return Err(unborn_fatal(&refs)),
        }
    }

    while let Some(sha) = walk.next()? {
        let subject = commit_subject(&store, &sha)?;
        let short = &hex(&sha)[..7];
        if graph {
            println!("* {short} {subject}");
        } else {
            println!("{short} {subject}");
        }
    }
    Ok(())
}

fn commit_subject(store: &ObjectStore, sha: &[u8; 20]) -> Result<String> {
    let (kind, content) = store.read_object(&hex(sha))?;
    if kind != Kind::Commit {
        return Err(GitError::Corrupt(format!("{} is not a commit", hex(sha))));
    }
    let commit = Commit::parse(&content)?;
    Ok(String::from_utf8_lossy(&commit.message)
        .split('\n')
        .next()
        .unwrap_or("")
        .to_string())
}