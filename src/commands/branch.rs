use crate::error::{GitError, Result};
use crate::refs::Refs;
use crate::revwalk::{merge_base, resolve_rev, unborn_fatal};
use crate::store::ObjectStore;
use crate::worktree::{abs_git_dir, repo_root};

/// Run `git branch [<name>] | -a | -d <name> | -D <name>`.
pub fn run_branch(args: &[String]) -> Result<()> {
    let mut list = false;
    let mut delete = false;
    let mut force = false;
    let mut name: Option<String> = None;
    let mut start: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-a" | "--list" => list = true,
            "-d" => delete = true,
            "-D" => {
                delete = true;
                force = true;
            }
            "-q" | "--quiet" => {}
            s if s.starts_with('-') => {
                return Err(GitError::Invalid(format!("branch: unknown option '{s}'")));
            }
            s => {
                if name.is_none() {
                    name = Some(s.to_string());
                } else if start.is_none() {
                    start = Some(s.to_string());
                } else {
                    return Err(GitError::Invalid("branch: too many arguments".into()));
                }
            }
        }
        i += 1;
    }

    let refs = Refs::discover()?;
    let store = ObjectStore::discover()?;

    // List mode
    if list {
        let branches = refs.list_names("refs/heads/")?;
        let branches: Vec<String> = branches
            .iter()
            .map(|b| b.trim_start_matches("refs/heads/").to_string())
            .collect();
        return list_branches(&refs, &branches);
    }

    // Need a name for create/delete
    let Some(name) = name else {
        return Err(GitError::Invalid("branch: no branch name given".into()));
    };
    Refs::validate_name(&format!("refs/heads/{name}"))?;

    // Delete mode
    if delete {
        return delete_branch(&refs, &store, &name, force);
    }

    // Create mode
    create_branch(&refs, &store, &name, start.as_deref())
}

/// `git branch -a`: one branch per line, `*` on the current branch;
/// detached HEAD prints a pseudo row first (probed: no column padding when
/// piped, detached row sorts first).
fn list_branches(refs: &Refs, branches: &[String]) -> Result<()> {
    let mut sorted: Vec<String> = branches.to_vec();
    sorted.sort();

    if refs.head_branch().is_none()
        && let Some(sha) = refs.resolve("HEAD")?
    {
        println!("* (HEAD detached at {})", &sha[..7.min(sha.len())]);
    }
    let current = refs.head_branch();
    for b in sorted {
        let prefix = if Some(&b) == current.as_ref() {
            "* "
        } else {
            "  "
        };
        println!("{prefix}{b}");
    }
    Ok(())
}

fn create_branch(refs: &Refs, store: &ObjectStore, name: &str, start: Option<&str>) -> Result<()> {
    // Check if branch already exists
    if refs.resolve(&format!("refs/heads/{name}"))?.is_some() {
        return Err(GitError::Fatal(format!(
            "a branch named '{name}' already exists"
        )));
    }

    // Resolve start point (default HEAD)
    let start_rev = start.unwrap_or("HEAD");
    let start_sha = match resolve_rev(refs, store, start_rev)? {
        Some(s) => s,
        None => {
            return Err(GitError::Fatal(format!(
                "branch: not a valid revision: '{start_rev}'",
            )));
        }
    };

    let sha_hex = hex(&start_sha);
    let message = match start {
        Some(s) => format!("branch: Created from {s}"),
        None => {
            let from = refs.head_branch().unwrap_or_else(|| "HEAD".to_string());
            format!("branch: Created from {from}")
        }
    };

    refs.update(&format!("refs/heads/{name}"), &sha_hex, &message)?;
    Ok(())
}

fn delete_branch(refs: &Refs, store: &ObjectStore, name: &str, force: bool) -> Result<()> {
    // Cannot delete current branch (probed: `error:` + rc 1)
    if let Some(current) = refs.head_branch()
        && current == name
    {
        let root = repo_root(&abs_git_dir(refs.git_dir())?)?;
        let root_str = root.to_string_lossy().replace('\\', "/");
        return Err(GitError::Invalid(format!(
            "error: cannot delete branch '{name}' used by worktree at '{root_str}'"
        )));
    }

    // Resolve branch tip
    let sha = match refs.resolve(&format!("refs/heads/{name}"))? {
        Some(s) => s,
        None => {
            return Err(GitError::Invalid(format!(
                "error: branch '{name}' not found"
            )));
        }
    };
    let sha_bytes = parse_oid(&sha)?;

    // If not forcing, check if merged into HEAD
    if !force {
        let head = match refs.resolve("HEAD")? {
            Some(h) => parse_oid(&h)?,
            None => return Err(unborn_fatal(refs)),
        };
        let mb = merge_base(store, head, sha_bytes)?;
        // Branch is merged if its tip is an ancestor of HEAD (merge_base == branch_tip)
        if mb != Some(sha_bytes) {
            eprintln!("error: the branch '{name}' is not fully merged");
            eprintln!("hint: If you are sure you want to delete it, run 'git branch -D {name}'");
            eprintln!(
                "hint: Disable this message with \"git config set advice.forceDeleteBranch false\""
            );
            return Err(GitError::Invalid(String::new())); // exit 1
        }
    }

    // Delete
    refs.delete(&format!("refs/heads/{name}"))?;
    let short = &sha[..7.min(sha.len())];
    println!("Deleted branch {name} (was {short}).");
    Ok(())
}

fn hex(oid: &[u8; 20]) -> String {
    oid.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_oid(sha: &str) -> Result<[u8; 20]> {
    let mut oid = [0u8; 20];
    for i in 0..20 {
        oid[i] = u8::from_str_radix(&sha[2 * i..2 * i + 2], 16)
            .map_err(|_| GitError::Corrupt(format!("bad sha '{sha}'")))?;
    }
    Ok(oid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Kind;
    use std::env;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_refs() -> (Refs, ObjectStore, std::path::PathBuf) {
        let dir = env::temp_dir().join(format!(
            "git-rs-branch-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let refs = Refs::new(&dir);
        let store = ObjectStore::new(dir.join("objects"));
        (refs, store, dir)
    }

    fn commit(store: &ObjectStore, parents: &[[u8; 20]], first: u8) -> [u8; 20] {
        let mut tree = [0u8; 20];
        tree[0] = first;
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
    fn create_and_delete_branch() {
        let (refs, store, _dir) = temp_refs();
        let root = commit(&store, &[], 1);
        let sha_hex = hex(&root);
        refs.update("refs/heads/main", &sha_hex, "initial").unwrap();
        refs.update("HEAD", &sha_hex, "init").unwrap();

        create_branch(&refs, &store, "feature", Some("main")).unwrap();
        assert!(refs.resolve("refs/heads/feature").unwrap().is_some());

        delete_branch(&refs, &store, "feature", false).unwrap();
        assert!(refs.resolve("refs/heads/feature").unwrap().is_none());
    }

    #[test]
    fn delete_current_branch_fails() {
        let (refs, store, _dir) = temp_refs();
        let root = commit(&store, &[], 1);
        refs.update("refs/heads/main", &hex(&root), "initial")
            .unwrap();
        refs.set_head_symref("main", "init").unwrap();

        let err = delete_branch(&refs, &store, "main", false).unwrap_err();
        assert!(format!("{err}").contains("cannot delete branch 'main' used by worktree"));
    }

    #[test]
    fn delete_unmerged_requires_force() {
        let (refs, store, _dir) = temp_refs();
        let root = commit(&store, &[], 1);
        let child = commit(&store, &[root], 2);
        refs.update("refs/heads/main", &hex(&root), "initial")
            .unwrap();
        refs.update("refs/heads/feature", &hex(&child), "create")
            .unwrap();
        refs.set_head_symref("main", "init").unwrap();

        // Message lines go to stderr (git parity); the returned error is the
        // exit-1 sentinel. Message content is verified in integration tests.
        let err = delete_branch(&refs, &store, "feature", false).unwrap_err();
        assert!(matches!(err, GitError::Invalid(ref s) if s.is_empty()));

        // Force delete succeeds
        delete_branch(&refs, &store, "feature", true).unwrap();
        assert!(refs.resolve("refs/heads/feature").unwrap().is_none());
    }
}
