//! Ref reading and updating: loose refs, symrefs, packed-refs, reflogs.
//!
//! Formats (locked in rules.md): a loose ref is plain text `<sha>\n` or a
//! symref `ref: <target>\n`; `packed-refs` holds `<sha> <name>` lines plus
//! `^<sha>` peeled lines; reflogs append
//! `<old> <new> Name <email> <ts> <tz>[<tab><message>]\n`. Updates are
//! atomic: write a sibling temp file, fsync, rename over the target.

use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::error::{GitError, IoContext, Result};

/// The zero sha used in reflogs for unborn refs (matches git's all-zeros).
pub const ZERO_SHA: &str = "0000000000000000000000000000000000000000";

/// Ref state anchored at a git directory (`.git`).
pub struct Refs {
    git_dir: PathBuf,
}

impl Refs {
    /// Refs rooted at an explicit git directory (tests use this).
    pub fn new(git_dir: impl Into<PathBuf>) -> Self {
        Refs {
            git_dir: git_dir.into(),
        }
    }

    /// Resolve the git dir: `GIT_DIR` env, else `<cwd>/.git`.
    /// ponytail: no upward walk in v1, same rule as ObjectStore (D-002).
    pub fn discover() -> Result<Self> {
        let git_dir = match env::var("GIT_DIR") {
            Ok(dir) => PathBuf::from(dir),
            Err(_) => PathBuf::from(".git"),
        };
        Ok(Refs::new(git_dir))
    }

    /// Resolve a ref name to a sha, following symrefs (HEAD → branch).
    /// Unborn refs (symref target missing, or ref absent) resolve to `None`.
    /// A ref file that is neither `<sha>` nor `ref: <target>` is `Corrupt`.
    pub fn resolve(&self, name: &str) -> Result<Option<String>> {
        self.resolve_loop(name, 0)
    }

    fn resolve_loop(&self, name: &str, depth: u8) -> Result<Option<String>> {
        if depth > 10 {
            // ponytail: symref loop guard; real git errors here too.
            return Err(GitError::Corrupt(format!("ref loop detected at '{name}'")));
        }
        if let Some(content) = self.read_loose(name)? {
            if let Some(target) = symref_target(&content) {
                return self.resolve_loop(target, depth + 1);
            }
            return Ok(Some(content));
        }
        if let Some(sha) = self.read_packed(name)? {
            return Ok(Some(sha));
        }
        Ok(None)
    }

    /// Read a loose ref file: `Some("<sha>")`, `Some("ref: <target>")`, or
    /// `None` when absent. Malformed content is `Corrupt`.
    fn read_loose(&self, name: &str) -> Result<Option<String>> {
        let path = self.git_dir.join(name);
        let content = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(GitError::io(path.display().to_string(), "read ref", e)),
        };
        if content.len() > 1024 {
            return Err(GitError::Corrupt(format!(
                "ref '{name}' is unreasonably large"
            )));
        }
        let line = String::from_utf8(content)
            .map_err(|_| GitError::Corrupt(format!("ref '{name}' is not valid UTF-8")))?;
        let line = line.trim();
        if line.is_empty() {
            return Err(GitError::Corrupt(format!("ref '{name}' is empty")));
        }
        if is_sha(line) || symref_target(line).is_some() {
            Ok(Some(line.to_string()))
        } else {
            Err(GitError::Corrupt(format!(
                "ref '{name}' is not a sha or symref"
            )))
        }
    }

    fn read_packed(&self, name: &str) -> Result<Option<String>> {
        for (ref_name, sha, _peeled) in self.read_packed_refs()? {
            if ref_name == name {
                return Ok(Some(sha));
            }
        }
        Ok(None)
    }

    /// Parse `.git/packed-refs`: `#` header, `<sha> <name>`, `^<sha>` peeled.
    /// A missing file is an empty list. Malformed lines are `Corrupt`.
    pub fn read_packed_refs(&self) -> Result<Vec<(String, String, Option<String>)>> {
        let path = self.git_dir.join("packed-refs");
        let content = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(GitError::io(
                    path.display().to_string(),
                    "read packed-refs",
                    e,
                ));
            }
        };
        let mut refs: Vec<(String, String, Option<String>)> = Vec::new();
        let mut peel_next: Option<usize> = None;
        for (i, raw) in content.split(|&b| b == b'\n').enumerate() {
            let line = std::str::from_utf8(raw)
                .map_err(|_| GitError::Corrupt(format!("packed-refs line {} not UTF-8", i + 1)))?
                .trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(peeled) = line.strip_prefix('^') {
                let idx = peel_next.take().ok_or_else(|| {
                    GitError::Corrupt(format!(
                        "packed-refs line {}: '^' without a preceding ref",
                        i + 1
                    ))
                })?;
                if !is_sha(peeled) {
                    return Err(GitError::Corrupt(format!(
                        "packed-refs line {}: bad peeled sha",
                        i + 1
                    )));
                }
                refs[idx].2 = Some(peeled.to_string());
                continue;
            }
            let (sha, name) = line.split_once(' ').ok_or_else(|| {
                GitError::Corrupt(format!(
                    "packed-refs line {}: expected '<sha> <name>'",
                    i + 1
                ))
            })?;
            if !is_sha(sha) || name.is_empty() {
                return Err(GitError::Corrupt(format!(
                    "packed-refs line {}: bad ref line",
                    i + 1
                )));
            }
            refs.push((name.to_string(), sha.to_string(), None));
            peel_next = Some(refs.len() - 1);
        }
        Ok(refs)
    }

    /// Whether a ref name is valid per git's `check-ref-format` rules
    /// (rules.md): no `..`, no leading `.`, no whitespace, `~^:?*[\` or
    /// control chars; plus git's full set — no trailing `.`, no `.lock`
    /// suffix, no `@{`, no `/./`, no `//`.
    pub fn validate_name(name: &str) -> Result<()> {
        let bad = || GitError::Invalid(format!("refusing to update ref with bad name '{name}'"));
        if name.is_empty()
            || (name != "HEAD" && !name.starts_with("refs/"))
            || name == "@"
            || name.contains("..")
            || name.contains("@{")
            || name.starts_with('.')
            || name.ends_with('.')
            || name.ends_with(".lock")
            || name.contains("//")
        {
            return Err(bad());
        }
        for c in name.chars() {
            if c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c) || c == '\u{7f}' {
                return Err(bad());
            }
        }
        for part in name.split('/') {
            if part.is_empty() || part.starts_with('.') {
                return Err(bad());
            }
        }
        Ok(())
    }

    /// Update a ref to `new_sha`, atomically (sibling temp + fsync + rename).
    /// Symrefs are resolved to their target file (`update("HEAD", ...)`
    /// writes the branch). Writes a reflog entry when
    /// `core.logallrefupdates` is unset or true — to `logs/<name>` (the
    /// name as given, matching git's update-ref behavior).
    pub fn update(&self, name: &str, new_sha: &str, message: &str) -> Result<()> {
        let fail = |detail: String| {
            GitError::Fatal(format!("update_ref failed for ref '{name}': {detail}"))
        };

        Self::validate_name(name)
            .map_err(|_| fail(format!("refusing to update ref with bad name '{name}'")))?;
        if !is_sha(new_sha) || new_sha.chars().any(|c| c.is_ascii_uppercase()) {
            // git: "trying to write ref '<name>' with nonexistent object
            // <new>"; exit 128 for any bad object name.
            return Err(fail(format!(
                "trying to write ref '{name}' with nonexistent object {new_sha}"
            )));
        }

        let old_sha = match self.resolve(name)? {
            Some(sha) => sha,
            None => ZERO_SHA.to_string(),
        };

        let target = self.writable_target(name)?;
        let store = crate::store::ObjectStore::new(self.git_dir.join("objects"));
        if !store.object_path(new_sha).exists() {
            // ponytail: loose-only lookup; pack lookup when packs land.
            return Err(fail(format!(
                "trying to write ref '{name}' with nonexistent object {new_sha}"
            )));
        }

        let dir = target
            .parent()
            .ok_or_else(|| fail(format!("ref '{name}' has no parent directory")))?
            .to_path_buf();
        fs::create_dir_all(&dir).context(&dir, "create ref directory")?;
        let tmp = dir.join(format!(".tmp-ref-{}", std::process::id()));
        let mut file = fs::File::create(&tmp).context(&tmp, "create temp ref")?;
        file.write_all(new_sha.as_bytes())
            .context(&tmp, "write temp ref")?;
        file.write_all(b"\n").context(&tmp, "write temp ref")?;
        file.sync_all().context(&tmp, "fsync temp ref")?;
        fs::rename(&tmp, &target).context(&target, "commit ref update")?;

        self.append_reflog(name, &old_sha, new_sha, message)?;
        Ok(())
    }

    /// The file an update to `name` must write: for a symref, the target's
    /// file (recursively); for a plain ref, the ref file itself.
    fn writable_target(&self, name: &str) -> Result<PathBuf> {
        let mut current = name.to_string();
        for _ in 0..10 {
            match self.read_loose(&current)? {
                Some(content) => match symref_target(&content) {
                    Some(target) => current = target.to_string(),
                    None => return Ok(self.git_dir.join(current)),
                },
                None => return Ok(self.git_dir.join(current)),
            }
        }
        Err(GitError::Corrupt(format!("symref loop at '{name}'")))
    }

    /// Append `<old> <new> <ident> <ts> <tz>[tab]<msg>` to `logs/<name>`.
    fn append_reflog(&self, name: &str, old: &str, new: &str, message: &str) -> Result<()> {
        let cfg = Config::load_with(&self.git_dir, None)
            .map_err(|e| GitError::Fatal(format!("update_ref failed for ref '{name}': {e}")))?;
        if !cfg.get_bool("core", "logallrefupdates").unwrap_or(true) {
            return Ok(());
        }
        let Some((ident_name, ident_email)) = cfg.committer_identity() else {
            return Err(GitError::Fatal(format!(
                "update_ref failed for ref '{name}': no user identity configured"
            )));
        };
        let (ts, tz) = now_with_tz();
        let dir = self.git_dir.join("logs").join(name);
        let dir = dir.parent().unwrap_or(&self.git_dir).to_path_buf();
        fs::create_dir_all(&dir).context(&dir, "create reflog directory")?;
        let mut line = format!("{old} {new} {ident_name} <{ident_email}> {ts} {tz}");
        if !message.is_empty() {
            line.push('\t');
            line.push_str(message);
        }
        line.push('\n');
        let path = self.git_dir.join("logs").join(name);
        let mut f = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .context(&path, "open reflog")?;
        f.write_all(line.as_bytes())
            .context(&path, "append reflog")?;
        Ok(())
    }
}

/// The target of a symref line, else `None`.
fn symref_target(line: &str) -> Option<&str> {
    line.strip_prefix("ref: ")
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

/// A 40-lowercase-hex sha, as stored in ref files.
fn is_sha(s: &str) -> bool {
    s.len() == 40
        && s.chars().all(|c| c.is_ascii_hexdigit())
        && !s.chars().any(|c| c.is_ascii_uppercase())
}

/// Unix seconds now, tz from `GIT_COMMITTER_DATE` (`<ts> <tz>`) if set,
/// else UTC.
fn now_with_tz() -> (i64, String) {
    if let Ok(date) = env::var("GIT_COMMITTER_DATE")
        && let Some((ts, tz)) = date.split_once(' ')
        && let Ok(ts) = ts.trim().parse::<i64>()
    {
        return (ts, tz.trim().to_string());
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    (ts, "+0000".to_string()) // ponytail: local tz needs chrono/unsafe, banned
}

#[cfg(test)]
mod tests {
    #![allow(unsafe_code)] // env::set_var is unsafe in edition 2024; tests only
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_git_dir() -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "git-rs-refs-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("refs/heads")).unwrap();
        fs::create_dir_all(dir.join("logs")).unwrap();
        fs::write(dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        dir
    }

    fn sha(n: u8) -> String {
        format!("{n:040x}")
    }

    #[test]
    fn validate_rejects_git_bad_names() {
        for bad in [
            "",
            "@",
            "refs/heads/..",
            "refs/heads/a..b",
            "refs/heads/.a",
            "refs/heads/a b",
            "refs/heads/a~b",
            "refs/heads/a^b",
            "refs/heads/a:b",
            "refs/heads/a?b",
            "refs/heads/a*b",
            "refs/heads/a[b",
            "refs/heads/a\\b",
            "refs/heads/a.lock",
            "refs/heads/a@{b}",
            "refs/heads/a.",
            "refs/heads/a//b",
            "refs/heads/a/.b",
            "refs/heads/\u{7f}b",
            "a/b",
        ] {
            assert!(Refs::validate_name(bad).is_err(), "should reject '{bad}'");
        }
        for good in [
            "refs/heads/main",
            "refs/heads/feature/one",
            "refs/tags/v1.0",
            "refs/remotes/origin/HEAD",
        ] {
            assert!(Refs::validate_name(good).is_ok(), "should accept '{good}'");
        }
    }

    #[test]
    fn resolve_follows_symref_and_loose_over_packed() {
        let dir = temp_git_dir();
        fs::write(dir.join("refs/heads/main"), format!("{}\n", sha(1))).unwrap();
        let refs = Refs::new(&dir);
        assert_eq!(refs.resolve("HEAD").unwrap(), Some(sha(1)));
        assert_eq!(refs.resolve("refs/heads/main").unwrap(), Some(sha(1)));
        assert_eq!(refs.resolve("refs/heads/nope").unwrap(), None);
        // Symref loop → Corrupt.
        fs::write(dir.join("refs/heads/a"), "ref: refs/heads/b\n").unwrap();
        fs::write(dir.join("refs/heads/b"), "ref: refs/heads/a\n").unwrap();
        assert!(refs.resolve("refs/heads/a").is_err());
    }

    #[test]
    fn packed_refs_parses_and_loose_wins() {
        let dir = temp_git_dir();
        fs::write(dir.join("refs/heads/main"), format!("{}\n", sha(1))).unwrap();
        fs::write(
            dir.join("packed-refs"),
            format!(
                "# pack-refs with: peeled fully-peeled sorted \n{} refs/heads/main\n{} refs/tags/v1\n^{}\n",
                sha(2),
                sha(3),
                sha(4)
            ),
        )
        .unwrap();
        let refs = Refs::new(&dir);
        assert_eq!(refs.resolve("refs/heads/main").unwrap(), Some(sha(1)));
        assert_eq!(refs.resolve("refs/tags/v1").unwrap(), Some(sha(3)));
        let packed = refs.read_packed_refs().unwrap();
        assert_eq!(packed.len(), 2);
        assert_eq!(packed[1].1, sha(3));
        assert_eq!(packed[1].2.as_deref(), Some(sha(4).as_str()));
    }

    #[test]
    fn corrupted_packed_refs_is_corrupt() {
        let dir = temp_git_dir();
        fs::write(dir.join("packed-refs"), "not a ref line\n").unwrap();
        let refs = Refs::new(&dir);
        assert!(refs.read_packed_refs().is_err());
    }

    #[test]
    fn update_is_atomic_and_writes_reflog() {
        let dir = temp_git_dir();
        let store = crate::store::ObjectStore::new(dir.join("objects"));
        let id = store.write_blob(b"ref test").unwrap();
        let refs = Refs::new(&dir);
        unsafe { env::set_var("GIT_COMMITTER_NAME", "T") };
        unsafe { env::set_var("GIT_COMMITTER_EMAIL", "t@e.co") };
        unsafe { env::set_var("GIT_COMMITTER_DATE", "1700000000 +0530") };
        // Unborn HEAD update: creates refs/heads/main, reflog in logs/HEAD.
        refs.update("HEAD", &id, "commit (initial): first").unwrap();
        let main = fs::read_to_string(dir.join("refs/heads/main")).unwrap();
        assert_eq!(main.trim(), id);
        let log = fs::read_to_string(dir.join("logs/HEAD")).unwrap();
        assert_eq!(
            log,
            format!("{ZERO_SHA} {id} T <t@e.co> 1700000000 +0530\tcommit (initial): first\n")
        );
        // Direct ref update: reflog goes to logs/refs/heads/<name>.
        refs.update("refs/heads/f2", &id, "branch: created")
            .unwrap();
        let log = fs::read_to_string(dir.join("logs/refs/heads/f2")).unwrap();
        assert_eq!(
            log,
            format!("{ZERO_SHA} {id} T <t@e.co> 1700000000 +0530\tbranch: created\n")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_rejects_bad_name_and_missing_object() {
        let dir = temp_git_dir();
        let refs = Refs::new(&dir);
        let err = refs.update("refs/heads/a..b", &sha(1), "").unwrap_err();
        assert!(matches!(err, GitError::Fatal(_)));
        let err = refs.update("refs/heads/x", &sha(1), "").unwrap_err();
        assert!(matches!(err, GitError::Fatal(_)));
        assert!(matches!(
            refs.update("refs/heads/x", "ZZ", "").unwrap_err(),
            GitError::Fatal(_)
        ));
    }

    #[test]
    fn logallrefupdates_false_skips_reflog() {
        let dir = temp_git_dir();
        fs::create_dir_all(dir.join("objects")).unwrap();
        fs::write(dir.join("config"), "[core]\n\tlogallrefupdates = false\n").unwrap();
        let store = crate::store::ObjectStore::new(dir.join("objects"));
        let id = store.write_blob(b"x").unwrap();
        let refs = Refs::new(&dir);
        refs.update("refs/heads/x", &id, "m").unwrap();
        assert!(!dir.join("logs/refs/heads/x").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
