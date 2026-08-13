//! `git-rs commit -m <msg> [-m <msg>] [-a]` — record the index as a commit.
//!
//! Byte-parity goals (probed against git 2.55): the commit object hashes
//! identically to real git for the same tree/identity/dates, the reflog
//! message is `commit (initial): <subject>` for the root commit and
//! `commit: <subject>` after, empty-commit messages and exit codes match
//! (v1 prints only the final line — git prints a full status block first),
//! and a missing email reproduces git's hint block byte for byte.
//!
//! v1: no hooks, no editor, no `-F`, no pathspec, no `--amend`. Success is
//! silent (like `git commit -q`; git's `[branch sha] subject` + stat
//! summary output is not printed — D-015). `GIT_AUTHOR_DATE` /
//! `GIT_COMMITTER_DATE` take git's internal `<unix-ts> <tz>` form; the
//! ISO/RFC forms git also accepts are not parsed (D-015).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::{GitError, Result};
use crate::index::{Index, IndexEntry};
use crate::object::Commit;
use crate::object::tree::{Tree, TreeEntry};
use crate::refs::Refs;
use crate::store::{Kind, ObjectStore};
use crate::worktree::{abs_git_dir, hash_entry, index_path, repo_root, walk_worktree};

/// `git-rs commit -m <msg> [-m <msg>] [-a]`
pub fn run_commit(args: &[String]) -> Result<()> {
    let mut messages: Vec<String> = Vec::new();
    let mut all = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-m" | "--message" => {
                i += 1;
                let Some(msg) = args.get(i) else {
                    return Err(GitError::Invalid(
                        "commit: option '-m' requires a message".into(),
                    ));
                };
                messages.push(msg.clone());
            }
            "-a" | "--all" => all = true,
            "-q" | "--quiet" => {}
            s if s.starts_with('-') => {
                return Err(GitError::Invalid(format!("commit: unknown option '{s}'")));
            }
            s => {
                return Err(GitError::Invalid(format!(
                    "commit: unexpected argument '{s}'"
                )));
            }
        }
        i += 1;
    }
    if messages.is_empty() {
        return Err(GitError::Invalid(
            "commit: no message given; use -m (editor not implemented in v1)".into(),
        ));
    }

    let message_clean = clean_message(&messages);

    let refs = Refs::discover()?;
    let git_dir = abs_git_dir(refs.git_dir())?;
    let root = repo_root(&git_dir)?;
    let ipath = index_path(&git_dir);
    let mut idx = if ipath.exists() {
        Index::read(&ipath)?
    } else {
        Index::new()
    };
    let store = ObjectStore::discover()?;
    let config = Config::load()?;

    // Identity: env > config, git's fallback chain (probed). A missing
    // name falls back to the OS username; a missing email is fatal with
    // git's hint block and the auto-detect guess.
    let author = resolve_identity(
        env::var("GIT_AUTHOR_NAME").ok(),
        env::var("GIT_AUTHOR_EMAIL").ok(),
        config.get("user", "name").map(String::from),
        config.get("user", "email").map(String::from),
        "Author",
    )?;
    // Committer chain (probed): GIT_COMMITTER_* > user config > the author
    // env pair — author env alone is enough for a successful commit.
    let committer = resolve_identity(
        env::var("GIT_COMMITTER_NAME").ok(),
        env::var("GIT_COMMITTER_EMAIL").ok(),
        env::var("GIT_AUTHOR_NAME")
            .ok()
            .or_else(|| config.get("user", "name").map(String::from)),
        env::var("GIT_AUTHOR_EMAIL")
            .ok()
            .or_else(|| config.get("user", "email").map(String::from)),
        "Committer",
    )?;

    if all {
        restage_all(&root, &store, &mut idx, &ipath)?;
    }

    let tree = tree_from_index(&store, idx.entries())?;
    let head = refs.resolve("HEAD")?;

    // Empty-commit checks (probed messages; git prints a status block
    // first, v1 prints only the final line — D-015).
    if let Some(h) = &head {
        if head_tree(&store, h)? == Some(tree.clone()) {
            let dirty = worktree_dirty(&root, &store, idx.entries());
            let msg = if !all && dirty {
                "no changes added to commit (use \"git add\" and/or \"git commit -a\")"
            } else {
                "nothing to commit, working tree clean"
            };
            println!("{msg}");
            return Err(GitError::Invalid(String::new()));
        }
    } else if idx.entries().iter().all(|e| e.stage() != 0) {
        // Unborn HEAD with nothing staged: distinguish untracked files
        // present (probed: different message).
        let matcher = crate::ignore::IgnoreMatcher::load(&root, &git_dir)?;
        let items = walk_worktree(&root, &git_dir, &matcher)?;
        let untracked = items.iter().any(|it| {
            !it.is_dir
                && !idx.entries().iter().any(|e| e.path == it.path)
                && !matcher.is_ignored(&it.path, false)
        });
        let msg = if untracked {
            "nothing added to commit but untracked files present (use \"git add\" to track)"
        } else {
            "nothing to commit (create/copy files and use \"git add\" to track)"
        };
        println!("{msg}");
        return Err(GitError::Invalid(String::new()));
    }

    let message = if message_clean.is_empty() {
        // stderr, exit 1 — but only after the nothing-to-commit checks
        // (probed: git reports an empty index before an empty message).
        return Err(GitError::Invalid(
            "Aborting commit due to empty commit message.".into(),
        ));
    } else {
        message_clean
    };

    let (author_ts, author_tz) = commit_dates("GIT_AUTHOR_DATE", "GIT_COMMITTER_DATE")?;
    let (committer_ts, committer_tz) = commit_dates("GIT_COMMITTER_DATE", "GIT_AUTHOR_DATE")?;
    let author = crate::object::Ident::new(author.0, author.1, author_ts, author_tz)?;
    let committer =
        crate::object::Ident::new(committer.0, committer.1, committer_ts, committer_tz)?;

    let parents: Vec<[u8; 20]> = match &head {
        Some(h) => vec![hex_to_oid(h)?],
        None => Vec::new(),
    };
    let commit = Commit {
        tree: hex_to_oid(&tree)?,
        parents,
        author,
        committer,
        message: message.as_bytes().to_vec(),
    };
    let bytes = commit.serialize()?;
    let id = store.write_object(Kind::Commit, &bytes)?;

    let subject = message.split('\n').next().unwrap_or("");
    let reflog = if head.is_none() {
        format!("commit (initial): {subject}")
    } else {
        format!("commit: {subject}")
    };
    refs.update("HEAD", &id, &reflog)?;
    Ok(())
}

/// Join `-m` messages with a single `\n` between them, then apply git's
/// default cleanup (probed): trailing whitespace stripped per line, trailing
/// blank lines dropped; internal blank lines and leading whitespace kept.
/// An empty `-m` becomes an empty paragraph (a blank line in the middle).
pub(crate) fn clean_message(messages: &[String]) -> String {
    let mut lines: Vec<String> = messages
        .join("\n")
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect();
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let msg = lines.join("\n");
    if msg.is_empty() {
        msg
    } else {
        format!("{msg}\n")
    }
}

/// Resolve name/email for an identity slot, applying git's fallbacks:
/// env > config; missing name falls back to the OS username; missing email
/// is fatal with git's exact hint block and `unable to auto-detect email
/// address (got '<guess>')` (probed, git 2.55).
fn resolve_identity(
    env_name: Option<String>,
    env_email: Option<String>,
    cfg_name: Option<String>,
    cfg_email: Option<String>,
    who: &str,
) -> Result<(String, String)> {
    let name = env_name.or(cfg_name);
    let email = env_email.or(cfg_email);
    let Some(email) = email else {
        let host = env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".to_string());
        let guess = format!("{}@{}. (none)", os_username(), host).replace(". ", ".");
        eprintln!(
            "{who} identity unknown\n\
             \n\
             *** Please tell me who you are.\n\
             \n\
             Run\n\
             \n\
             \x20\x20git config --global user.email \"you@example.com\"\n\
             \x20\x20git config --global user.name \"Your Name\"\n\
             \n\
             to set your account's default identity.\n\
             Omit --global to set the identity only in this repository.\n"
        );
        return Err(GitError::Fatal(format!(
            "unable to auto-detect email address (got '{guess}')"
        )));
    };
    let name = name.unwrap_or_else(os_username);
    Ok((name, email))
}

fn os_username() -> String {
    env::var("USERNAME")
        .or_else(|_| env::var("USER"))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Author date: `GIT_AUTHOR_DATE`, else `GIT_COMMITTER_DATE`, else now.
/// Committer date: `GIT_COMMITTER_DATE`, else `GIT_AUTHOR_DATE`, else now.
fn commit_dates(primary: &str, fallback: &str) -> Result<(i64, i32)> {
    for var in [primary, fallback] {
        if let Ok(raw) = env::var(var) {
            return parse_date(&raw);
        }
    }
    let (ts, tz) = crate::refs::now_with_tz()?;
    Ok((ts, parse_date(&format!("{ts} {tz}")).unwrap().1))
}

/// Parse `<unix-ts> <tz>` where tz is `±HHMM`, validating git's range.
fn parse_date(raw: &str) -> Result<(i64, i32)> {
    let (ts, tz) = raw
        .split_once(' ')
        .ok_or_else(|| GitError::Fatal(format!("invalid date format: {raw}")))?;
    let ts: i64 = ts
        .trim()
        .parse()
        .map_err(|_| GitError::Fatal(format!("invalid date format: {raw}")))?;
    let (sign, digits) = match tz.trim().strip_prefix('-') {
        Some(d) => (-1, d),
        None => match tz.trim().strip_prefix('+') {
            Some(d) => (1, d),
            None => {
                return Err(GitError::Fatal(format!("invalid date format: {raw}")));
            }
        },
    };
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(GitError::Fatal(format!("invalid date format: {raw}")));
    }
    let value: i32 = digits
        .parse()
        .map_err(|_| GitError::Fatal(format!("invalid date format: {raw}")))?;
    if !(-1200..=1400).contains(&value) {
        return Err(GitError::Fatal(format!("invalid date format: {raw}")));
    }
    Ok((ts, sign * value))
}

/// Stage every modified/deleted tracked file (`commit -a`): hash the
/// worktree version of each stage-0 entry, replacing or unstaging it.
/// Untracked files are untouched.
fn restage_all(root: &Path, store: &ObjectStore, idx: &mut Index, ipath: &Path) -> Result<()> {
    let paths: Vec<Vec<u8>> = idx
        .entries()
        .iter()
        .filter(|e| e.stage() == 0)
        .map(|e| e.path.clone())
        .collect();
    let mut dirty = false;
    for rel in paths {
        let abs = root.join(rel_os_path(&rel));
        match fs::symlink_metadata(&abs) {
            Ok(_) => {
                let e = crate::commands::add::build_entry(root, store, &rel)?;
                let changed = idx
                    .entries()
                    .iter()
                    .any(|old| old.path == e.path && (old.oid != e.oid || old.mode != e.mode));
                idx.stage(e);
                dirty |= changed;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let before = idx.entries().len();
                idx.unstage(&rel);
                dirty |= idx.entries().len() != before;
            }
            Err(e) => return Err(GitError::io(abs.display().to_string(), "stat path", e)),
        }
    }
    if dirty {
        idx.write(ipath)?;
    }
    Ok(())
}

/// Build a tree object from the stage-0 index entries, returning its sha.
/// Entries are grouped by directory; subtrees get mode `040000`. A path
/// that is both a file and a directory prefix errors with git's
/// `write-tree` message (probed: `invalid object <mode> <sha> for '<name>'`).
pub(crate) fn tree_from_index(store: &ObjectStore, entries: &[IndexEntry]) -> Result<String> {
    let mut files: Vec<&IndexEntry> = entries.iter().filter(|e| e.stage() == 0).collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    build_tree(store, &files, 0)
}

/// Recursively build the subtree for the shared prefix `skip` of `files`.
fn build_tree(store: &ObjectStore, files: &[&IndexEntry], skip: usize) -> Result<String> {
    let mut tree = Vec::new();
    let mut i = 0;
    while i < files.len() {
        let rest = &files[i].path[skip..];
        let seg_len = rest.iter().position(|&b| b == b'/').unwrap_or(rest.len());
        let seg = &rest[..seg_len];
        let is_dir = files[i].path.len() > skip + seg_len;
        if is_dir {
            let mut j = i + 1;
            while j < files.len()
                && files[j].path.len() > skip + seg_len
                && &files[j].path[skip..skip + seg_len] == seg
            {
                j += 1;
            }
            let sub = build_tree(store, &files[i..j], skip + seg_len + 1)?;
            tree.push(TreeEntry {
                mode: 0o040000,
                name: seg.to_vec(),
                oid: hex_to_oid(&sub)?,
            });
            i = j;
        } else {
            // A file whose name is also a directory prefix is a conflict.
            if i + 1 < files.len()
                && files[i + 1].path.len() > skip + seg_len
                && &files[i + 1].path[skip..skip + seg_len] == seg
            {
                eprintln!(
                    "error: invalid object {:o} {} for '{}'",
                    files[i].mode,
                    crate::commands::status::hex(&files[i].oid),
                    String::from_utf8_lossy(seg)
                );
                return Err(GitError::Fatal(
                    "git-write-tree: error building trees".into(),
                ));
            }
            tree.push(TreeEntry {
                mode: files[i].mode,
                name: seg.to_vec(),
                oid: files[i].oid,
            });
            i += 1;
        }
    }
    let t = Tree { entries: tree };
    let bytes = t.serialize()?;
    store.write_object(Kind::Tree, &bytes)
}

/// The tree sha of a commit, or `None` when the object is not a commit.
fn head_tree(store: &ObjectStore, sha: &str) -> Result<Option<String>> {
    let (kind, content) = match store.read_object(sha) {
        Ok(v) => v,
        Err(GitError::NotFound(_)) => return Ok(None),
        Err(e) => return Err(e),
    };
    if kind != Kind::Commit {
        return Ok(None);
    }
    Ok(Some(crate::commands::status::hex(
        &Commit::parse(&content)?.tree,
    )))
}

/// Whether any stage-0 file differs from the worktree (missing or changed
/// content) — the difference between "working tree clean" and "no changes
/// added to commit".
fn worktree_dirty(root: &Path, store: &ObjectStore, entries: &[IndexEntry]) -> bool {
    entries.iter().filter(|e| e.stage() == 0).any(|e| {
        let abs = root.join(rel_os_path(&e.path));
        match hash_entry(store, &abs, false) {
            Ok(h) => h != e.oid,
            Err(_) => true,
        }
    })
}

fn rel_os_path(rel: &[u8]) -> PathBuf {
    let s = String::from_utf8_lossy(rel);
    PathBuf::from(s.replace('/', std::path::MAIN_SEPARATOR_STR))
}

/// Parse a 40-hex sha to raw oid bytes.
fn hex_to_oid(sha: &str) -> Result<[u8; 20]> {
    if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(GitError::Corrupt(format!("bad sha '{sha}'")));
    }
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

    #[test]
    fn clean_message_strips_trailing_whitespace_per_line() {
        assert_eq!(clean_message(&["title line   ".into()]), "title line\n");
        assert_eq!(
            clean_message(&[" body with trailing ws   ".into()]),
            " body with trailing ws\n"
        );
        assert_eq!(clean_message(&["a\tb \t".into()]), "a\tb\n");
    }

    #[test]
    fn clean_message_joins_with_single_newline() {
        assert_eq!(clean_message(&["one".into(), "two".into()]), "one\ntwo\n");
        assert_eq!(
            clean_message(&["one".into(), "".into(), "two".into()]),
            "one\n\ntwo\n"
        );
    }

    #[test]
    fn clean_message_drops_trailing_blank_lines_only() {
        assert_eq!(clean_message(&["a\n\n".into()]), "a\n");
        assert_eq!(clean_message(&["a\n\nb\n".into()]), "a\n\nb\n");
        // Internal blank lines survive verbatim, including ones that fall
        // between -m messages (git's stripspace keeps them).
        assert_eq!(clean_message(&["a\n\n".into(), "b".into()]), "a\n\n\nb\n");
        assert_eq!(clean_message(&["\n\n".into()]), "");
        assert_eq!(clean_message(&["".into()]), "");
        assert_eq!(clean_message(&["   ".into()]), "");
    }

    #[test]
    fn clean_message_keeps_leading_whitespace_and_internal_blanks() {
        assert_eq!(
            clean_message(&["  # not a comment\n\nbody".into()]),
            "  # not a comment\n\nbody\n"
        );
        // Empty paragraph in the middle survives as a blank line.
        assert_eq!(
            clean_message(&["title".into(), "".into(), "final".into()]),
            "title\n\nfinal\n"
        );
    }

    #[test]
    fn parse_date_validates_git_internal_form() {
        assert_eq!(parse_date("1786610047 +0530").unwrap(), (1786610047, 530));
        assert_eq!(parse_date("1 -0700").unwrap(), (1, -700));
        assert_eq!(parse_date("1 +0000").unwrap(), (1, 0));
        assert!(parse_date("1786610047").is_err());
        assert!(parse_date("garbage").is_err());
        assert!(parse_date("1 +2500").is_err());
        assert!(parse_date("1 +053").is_err());
    }
}
