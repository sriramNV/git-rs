use std::env;

use crate::config::Config;
use crate::error::{GitError, Result};
use crate::object::Tag;
use crate::refs::Refs;
use crate::revwalk::resolve_rev;
use crate::store::{Kind, ObjectStore};

/// Run `git tag [<name>] | -a <name> -m <msg> | -l | -d <name>`.
pub fn run_tag(args: &[String]) -> Result<()> {
    let mut list = false;
    let mut annotated = false;
    let mut delete = false;
    let mut message: Option<String> = None;
    let mut name: Option<String> = None;
    let mut target: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-l" | "--list" => list = true,
            "-a" => annotated = true,
            "-d" => delete = true,
            "-m" | "--message" => {
                i += 1;
                let Some(msg) = args.get(i) else {
                    return Err(GitError::Invalid(
                        "tag: option '-m' requires a message".into(),
                    ));
                };
                message = Some(msg.clone());
            }
            s if s.starts_with('-') => {
                return Err(GitError::Invalid(format!("tag: unknown option '{s}'")));
            }
            s => {
                if name.is_none() {
                    name = Some(s.to_string());
                } else if target.is_none() {
                    target = Some(s.to_string());
                } else {
                    return Err(GitError::Invalid("tag: too many arguments".into()));
                }
            }
        }
        i += 1;
    }

    let refs = Refs::discover()?;
    let store = ObjectStore::discover()?;

    // List mode
    if list {
        let mut names = refs.list_names("refs/tags/")?;
        for n in &mut names {
            *n = n.trim_start_matches("refs/tags/").to_string();
        }
        print_tag_list(&names);
        return Ok(());
    }

    // Delete mode
    if delete {
        let Some(name) = name else {
            return Err(GitError::Invalid("tag: no tag name given".into()));
        };
        return delete_tag(&refs, &name);
    }

    // Create mode (needs name)
    let Some(name) = name else {
        return Err(GitError::Invalid("tag: no tag name given".into()));
    };
    Refs::validate_name(&format!("refs/tags/{name}"))?;

    // Resolve target (default HEAD)
    let target_rev = target.unwrap_or_else(|| "HEAD".to_string());
    let target_sha = match resolve_rev(&refs, &store, &target_rev)? {
        Some(s) => s,
        None => {
            return Err(GitError::Fatal(format!(
                "tag: not a valid revision: '{target_rev}'",
            )));
        }
    };

    // Check if tag already exists
    if refs.resolve(&format!("refs/tags/{name}"))?.is_some() {
        return Err(GitError::Fatal(format!("tag '{name}' already exists")));
    }

    if annotated {
        create_annotated_tag(&refs, &store, &name, target_sha, message)?;
    } else {
        create_lightweight_tag(&refs, &name, target_sha)?;
    }
    Ok(())
}

fn print_tag_list(tags: &[String]) {
    // Plain byte/lexicographic sort — matches `git tag -l` (git 2.55 sorts
    // by refname, NOT versioncmp; verified by probe: v1.10 < v1.2 < v1.9).
    let mut sorted = tags.to_vec();
    sorted.sort();
    for t in sorted {
        println!("{t}");
    }
}

fn create_lightweight_tag(refs: &Refs, name: &str, sha: [u8; 20]) -> Result<()> {
    let sha_hex = hex(&sha);
    refs.update(&format!("refs/tags/{name}"), &sha_hex, "")?;
    Ok(())
}

fn create_annotated_tag(
    refs: &Refs,
    store: &ObjectStore,
    name: &str,
    sha: [u8; 20],
    message: Option<String>,
) -> Result<()> {
    // Tagger = committer chain, same as `git tag`: GIT_COMMITTER_NAME/EMAIL
    // > user config > GIT_AUTHOR_NAME/EMAIL (probed), with the committer
    // date env pair (or now).
    let global = crate::config::global_config_path();
    let cfg = Config::load_with(refs.git_dir(), global.as_deref())?;
    let (tagger_name, tagger_email) = env::var("GIT_COMMITTER_NAME")
        .ok()
        .or_else(|| cfg.get("user", "name").map(String::from))
        .or_else(|| env::var("GIT_AUTHOR_NAME").ok())
        .zip(
            env::var("GIT_COMMITTER_EMAIL")
                .ok()
                .or_else(|| cfg.get("user", "email").map(String::from))
                .or_else(|| env::var("GIT_AUTHOR_EMAIL").ok()),
        )
        .ok_or_else(|| GitError::Fatal("no user identity configured".into()))?;
    let (ts, tz) = crate::commands::commit::commit_dates("GIT_COMMITTER_DATE", "GIT_AUTHOR_DATE")?;
    let tagger = crate::object::Ident::new(&tagger_name, &tagger_email, ts, tz)?;

    let tag_content = annotated_tag_content(sha, name, &tagger, message)?;
    let tag_sha = store.write_object(Kind::Tag, &tag_content)?;

    // Write tag ref pointing to the tag object
    refs.update(&format!("refs/tags/{name}"), &tag_sha, "")?;
    Ok(())
}

/// Serialize an annotated tag object. Message: use provided, default to
/// empty. Append \n if non-empty (git -m behavior).
fn annotated_tag_content(
    sha: [u8; 20],
    name: &str,
    tagger: &crate::object::Ident,
    message: Option<String>,
) -> Result<Vec<u8>> {
    let msg_bytes = match message {
        Some(m) if !m.is_empty() => format!("{m}\n").into_bytes(),
        _ => Vec::new(),
    };
    let tag = Tag {
        object: sha,
        obj_type: "commit".to_string(),
        name: name.to_string(),
        tagger: tagger.clone(),
        message: msg_bytes,
    };
    tag.serialize()
}

fn delete_tag(refs: &Refs, name: &str) -> Result<()> {
    let sha = match refs.resolve(&format!("refs/tags/{name}"))? {
        Some(s) => s,
        None => return Err(GitError::Invalid(format!("error: tag '{name}' not found."))),
    };
    refs.delete(&format!("refs/tags/{name}"))?;
    let short = &sha[..7.min(sha.len())];
    println!("Deleted tag '{name}' (was {short})");
    Ok(())
}

fn hex(oid: &[u8; 20]) -> String {
    oid.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
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
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_refs() -> (Refs, ObjectStore, std::path::PathBuf) {
        let dir = env::temp_dir().join(format!(
            "git-rs-tag-test-{}-{}",
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
    fn lightweight_tag_roundtrip() {
        let (refs, store, _dir) = temp_refs();
        let root = commit(&store, &[], 1);
        let sha_hex = hex(&root);
        refs.update("refs/heads/main", &sha_hex, "initial").unwrap();
        refs.update("HEAD", &sha_hex, "init").unwrap();

        create_lightweight_tag(&refs, "v1.0", root).unwrap();
        let resolved = refs.resolve("refs/tags/v1.0").unwrap().unwrap();
        assert_eq!(resolved, sha_hex);
    }

    #[test]
    fn tag_list_sort_matches_git() {
        // Plain byte/lexicographic order, matching `git tag -l` (probed).
        let tags = vec!["v1.10", "v1.2", "v1.9", "v0.5", "foo", "v1.0.1", "v1.02"];
        let mut sorted = tags.clone();
        sorted.sort();
        assert_eq!(
            sorted,
            vec!["foo", "v0.5", "v1.0.1", "v1.02", "v1.10", "v1.2", "v1.9"]
        );
    }

    #[test]
    fn annotated_tag_serializes() {
        let tagger = crate::object::Ident::new("Tester", "t@e", 1700000000, 330).unwrap();
        let content =
            annotated_tag_content(short_oid(1), "v1.0", &tagger, Some("release one".into()))
                .unwrap();

        let tag = Tag::parse(&content).unwrap();
        assert_eq!(tag.name, "v1.0");
        assert_eq!(tag.object, short_oid(1));
        assert_eq!(tag.message, b"release one\n");
        assert_eq!(tag.tagger.name, "Tester");
    }

    #[test]
    fn annotated_tag_empty_message() {
        let tagger = crate::object::Ident::new("Tester", "t@e", 1700000000, 330).unwrap();
        let content = annotated_tag_content(short_oid(1), "v1.0", &tagger, None).unwrap();
        let tag = Tag::parse(&content).unwrap();
        assert!(tag.message.is_empty());
    }

    fn short_oid(first: u8) -> [u8; 20] {
        let mut o = [0u8; 20];
        o[0] = first;
        o
    }
}
