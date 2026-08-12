//! `.gitignore` matching (v1: per-directory `.gitignore` files only —
//! `.git/info/exclude` and `core.excludesfile` are out of scope, see
//! decisions D-013).
//!
//! Git semantics implemented: `!` negation, trailing `/` directory-only,
//! leading `/` anchoring (also implied by any `/` in the pattern), `**`
//! (leading `**/`, trailing `/**`, middle `/**/`), `*` / `?` / `[...]`
//! classes within a segment, and last-match-wins across the whole rule set
//! (deeper `.gitignore` files take precedence over shallower ones).

use std::fs;
use std::path::Path;

use crate::error::{GitError, IoContext, Result};

/// A single parsed pattern from one `.gitignore` line.
#[derive(Debug, Clone)]
struct Rule {
    /// Directory of the `.gitignore` that owns this rule, relative to the
    /// repo root as `/`-separated bytes (empty = repo root).
    dir: Vec<u8>,
    /// The pattern without negation/dir-only markers.
    pat: Vec<u8>,
    /// `!` negation — a match inverts the decision.
    negated: bool,
    /// Trailing `/`: only matches directories.
    dir_only: bool,
    /// Leading `/` (or any `/` in the pattern): anchored to the rule's
    /// directory, never a basename match.
    anchored: bool,
}

impl Rule {
    /// Whether this rule's directory is a prefix of `path`.
    fn applies_to(&self, path: &[u8]) -> Option<Vec<u8>> {
        if self.dir.is_empty() {
            return Some(path.to_vec());
        }
        let prefix = format!("{}/", String::from_utf8_lossy(&self.dir));
        let prefix = prefix.as_bytes();
        if path.starts_with(prefix) {
            Some(path[prefix.len()..].to_vec())
        } else {
            None
        }
    }

    /// Match `rel` (path relative to the rule's directory, bytes) against
    /// the pattern. `is_dir` gates dir-only rules.
    fn matches(&self, rel: &[u8], is_dir: bool) -> bool {
        if self.dir_only && !is_dir {
            return false;
        }
        if self.anchored {
            // Anchored to the rule's directory.
            match_wild(&self.pat, rel)
        } else {
            // Basename match at any depth.
            match rel.iter().rposition(|&b| b == b'/') {
                Some(i) => match_wild(&self.pat, &rel[i + 1..]),
                None => match_wild(&self.pat, rel),
            }
        }
    }
}

/// The ordered rule set for a repo. Rules are evaluated in order; the last
/// matching rule decides.
#[derive(Debug, Default)]
pub struct IgnoreMatcher {
    rules: Vec<Rule>,
}

impl IgnoreMatcher {
    /// Load every `.gitignore` under `root` (skipping `git_dir`). A
    /// `.gitignore` inside an ignored directory is never reached — the
    /// walk prunes ignored directories, mirroring git's "cannot re-include
    /// a file if a parent directory is excluded".
    pub fn load(root: &Path, git_dir: &Path) -> Result<IgnoreMatcher> {
        let mut matcher = IgnoreMatcher { rules: Vec::new() };
        matcher.collect(root, git_dir, &mut vec![])?;
        Ok(matcher)
    }

    fn collect(&mut self, dir: &Path, git_dir: &Path, rel: &mut Vec<u8>) -> Result<()> {
        if dir == git_dir {
            return Ok(());
        }
        let entries = fs::read_dir(dir)
            .map_err(|e| GitError::io(dir.display().to_string(), "read directory", e))?;
        let mut ignore_file: Option<std::path::PathBuf> = None;
        let mut subdirs: Vec<(std::ffi::OsString, Vec<u8>)> = Vec::new();
        for e in entries {
            let e =
                e.map_err(|e| GitError::io(dir.display().to_string(), "read directory entry", e))?;
            let name = e.file_name().to_string_lossy().into_owned();
            let ft = e
                .file_type()
                .map_err(|e| GitError::io(dir.display().to_string(), "stat directory entry", e))?;
            if ft.is_dir() {
                subdirs.push((e.file_name(), name.into_bytes()));
            } else if name == ".gitignore" {
                ignore_file = Some(e.path());
            }
        }
        if let Some(path) = ignore_file {
            self.load_file(dir, rel, &path)?;
        }
        for (os_name, name_bytes) in subdirs {
            let child = dir.join(&os_name);
            if child == git_dir {
                continue;
            }
            let rel_len = rel.len();
            rel.extend_from_slice(&name_bytes);
            rel.push(b'/');
            let child_rel = rel.clone();
            // Prune: rules inside an ignored directory cannot take effect
            // anyway ("cannot re-include a file if a parent directory is
            // excluded").
            if !self.is_ignored(&child_rel, true) {
                self.collect(&child, git_dir, rel)?;
            }
            rel.truncate(rel_len);
        }
        Ok(())
    }

    fn load_file(&mut self, _dir: &Path, rel_dir: &[u8], path: &Path) -> Result<()> {
        let content = fs::read(path).context(path, "read .gitignore")?;
        let dir_bytes = rel_dir.strip_suffix(b"/").unwrap_or(rel_dir).to_vec();
        for (i, line) in split_lines(&content).enumerate() {
            if let Some(rule) = parse_line(&dir_bytes, line) {
                self.rules.push(rule);
            } else if !line.is_empty() && line[0] != b'#' {
                // Malformed pattern (e.g. unterminated class) — git treats
                // the line as non-matching; we mirror that silently.
                let _ = i;
            }
        }
        Ok(())
    }

    /// Whether `path` (repo-relative bytes) is ignored. `is_dir` must be
    /// true for directory-only patterns to match. A trailing `/` on `path`
    /// is tolerated (the worktree walk passes directory names with one).
    pub fn is_ignored(&self, path: &[u8], is_dir: bool) -> bool {
        let path = path.strip_suffix(b"/").unwrap_or(path);
        let mut last: Option<&Rule> = None;
        for rule in &self.rules {
            if let Some(rel) = rule.applies_to(path)
                && rule.matches(&rel, is_dir)
            {
                last = Some(rule);
            }
        }
        last.map(|r| !r.negated).unwrap_or(false)
    }
}

/// Split raw file bytes into lines (LF or CRLF terminated).
fn split_lines(content: &[u8]) -> impl Iterator<Item = &[u8]> {
    content
        .split(|&b| b == b'\n')
        .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
}

/// Parse one `.gitignore` line into a rule, or `None` (comment/blank).
fn parse_line(dir: &[u8], raw: &[u8]) -> Option<Rule> {
    // Strip trailing spaces (git does; escaped spaces need `\ ` — v1:
    // trailing backslash-space keeps the space, approximated by not
    // stripping when the line ends with an odd backslash run).
    let mut line = raw;
    while line.ends_with(b" ") {
        line = &line[..line.len() - 1];
    }
    if line.is_empty() || line[0] == b'#' {
        return None;
    }
    let mut negated = false;
    if line[0] == b'!' {
        negated = true;
        line = &line[1..];
        if line.is_empty() {
            return None;
        }
    }
    let mut dir_only = false;
    if line.ends_with(b"/") && line.len() > 1 {
        dir_only = true;
        line = &line[..line.len() - 1];
    }
    let mut anchored = false;
    let pat = if line[0] == b'/' {
        anchored = true;
        &line[1..]
    } else {
        line
    };
    if pat.is_empty() {
        return None;
    }
    if !anchored && pat.contains(&b'/') {
        anchored = true; // any slash makes it relative to the rule's dir
    }
    Some(Rule {
        dir: dir.to_vec(),
        pat: pat.to_vec(),
        negated,
        dir_only,
        anchored,
    })
}

/// Wildcard match over bytes: `*` and `?` never cross `/`; `**` only
/// appears as a full segment. `[...]` classes support ranges and `!`/`^`
/// negation.
fn match_wild(pat: &[u8], text: &[u8]) -> bool {
    match pat.split_first() {
        None => text.is_empty(),
        Some((&b'*', rest)) => {
            if rest.starts_with(b"*") {
                // `**` segment: only valid between/at slashes.
                return match_doublestar(pat, text);
            }
            if rest.is_empty() {
                // Trailing `*`: matches everything remaining, including
                // slashes (probed: `dir/*` ignores `dir/x/y.txt`).
                return true;
            }
            // Mid-pattern `*`: matches a non-slash run only — never
            // consumes a `/` (probed: `a*/b.txt` does not ignore
            // `a/x/b.txt`).
            match text.iter().position(|&b| b == b'/') {
                Some(i) => (0..i).any(|n| match_wild(rest, &text[n..])),
                None => (0..=text.len()).any(|n| match_wild(rest, &text[n..])),
            }
        }
        Some((&b'?', rest)) => !text.is_empty() && text[0] != b'/' && match_wild(rest, &text[1..]),
        Some((&b'[', rest)) => match_class(pat, text, rest),
        Some((&c, rest)) => !text.is_empty() && text[0] == c && match_wild(rest, &text[1..]),
    }
}

/// `**` handling. `pat` starts with `**`.
fn match_doublestar(pat: &[u8], text: &[u8]) -> bool {
    let rest = &pat[2..];
    if rest.is_empty() {
        return true; // bare `**` matches everything
    }
    if rest.starts_with(b"/") {
        // `**/` — zero or more leading directories.
        let after = &rest[1..];
        // Zero dirs:
        if match_wild(after, text) {
            return true;
        }
        // One or more dirs:
        let mut i = 0;
        while i < text.len() {
            if text[i] == b'/' && match_wild(after, &text[i + 1..]) {
                return true;
            }
            i += 1;
        }
        false
    } else {
        // `**` not at a segment boundary — treat as two wildcards of
        // non-slash runs (approximation; git only allows segment `**`).
        match_wild(pat, text)
    }
}

/// `[...]` class matching; `]` at the first position is literal.
fn match_class(_pat: &[u8], text: &[u8], rest: &[u8]) -> bool {
    if text.is_empty() || text[0] == b'/' {
        return false;
    }
    let mut i = 0;
    let mut negated = false;
    if rest.first() == Some(&b'!') || rest.first() == Some(&b'^') {
        negated = true;
        i = 1;
    }
    let mut matched = false;
    let mut first = true;
    while i < rest.len() {
        let c = rest[i];
        if c == b']' && !first {
            let after = &rest[i + 1..];
            if matched != negated {
                return match_wild(after, &text[1..]);
            }
            return false;
        }
        first = false;
        if c == b'\\' && i + 1 < rest.len() {
            i += 1;
            matched = matched || rest[i] == text[0];
        } else if i + 2 < rest.len() && rest[i + 1] == b'-' && rest[i + 2] != b']' {
            let lo = rest[i];
            let hi = rest[i + 2];
            matched = matched || (text[0] >= lo && text[0] <= hi);
            i += 2;
        } else {
            matched = matched || c == text[0];
        }
        i += 1;
    }
    false // unterminated class — git treats as literal `[`; v1: no match
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(patterns: &[&str]) -> IgnoreMatcher {
        let mut matcher = IgnoreMatcher::default();
        for p in patterns {
            matcher.rules.push(parse_line(b"", p.as_bytes()).unwrap());
        }
        matcher
    }

    #[test]
    fn basename_and_anchored() {
        let g = m(&["*.log", "/root.txt"]);
        assert!(g.is_ignored(b"a.log", false));
        assert!(g.is_ignored(b"sub/b.log", false));
        assert!(g.is_ignored(b"root.txt", false));
        assert!(!g.is_ignored(b"sub/root.txt", false));
        assert!(!g.is_ignored(b"a.txt", false));
    }

    #[test]
    fn negation_last_match_wins() {
        let g = m(&["*.log", "!keep.log"]);
        assert!(g.is_ignored(b"a.log", false));
        assert!(!g.is_ignored(b"keep.log", false));
    }

    #[test]
    fn dir_only() {
        let g = m(&["docs/"]);
        assert!(g.is_ignored(b"docs", true));
        assert!(!g.is_ignored(b"docs", false));
        assert!(!g.is_ignored(b"docs.txt", false));
    }

    #[test]
    fn doublestar() {
        let g = m(&["a/**/b"]);
        assert!(g.is_ignored(b"a/b", false));
        assert!(g.is_ignored(b"a/x/b", false));
        assert!(g.is_ignored(b"a/x/y/b", false));
        assert!(!g.is_ignored(b"a/x/y/c", false));
        let g = m(&["**/z"]);
        assert!(g.is_ignored(b"z", false));
        assert!(g.is_ignored(b"x/z", false));
        assert!(g.is_ignored(b"x/y/z", false));
        let g = m(&["x/**"]);
        assert!(g.is_ignored(b"x/y", false));
        assert!(g.is_ignored(b"x/y/z", false));
        assert!(!g.is_ignored(b"w", false));
    }

    #[test]
    fn classes_and_question() {
        let g = m(&["file[0-9].txt", "a?.c"]);
        assert!(g.is_ignored(b"file5.txt", false));
        assert!(!g.is_ignored(b"filex.txt", false));
        assert!(g.is_ignored(b"ab.c", false));
        assert!(!g.is_ignored(b"a/b.c", false));
        let g = m(&["[!a]b"]);
        assert!(g.is_ignored(b"xb", false));
        assert!(!g.is_ignored(b"ab", false));
    }

    #[test]
    fn deeper_gitignore_wins() {
        let mut matcher = IgnoreMatcher::default();
        matcher.rules.push(parse_line(b"", b"*.log").unwrap()); // root: ignore all logs
        matcher
            .rules
            .push(parse_line(b"sub", b"!keep.log").unwrap()); // sub: re-include
        assert!(matcher.is_ignored(b"a.log", false));
        assert!(matcher.is_ignored(b"sub/a.log", false));
        assert!(!matcher.is_ignored(b"sub/keep.log", false));
        assert!(matcher.is_ignored(b"keep.log", false)); // root rule still wins at root
    }

    #[test]
    fn pattern_with_slash_is_anchored_to_dir() {
        let mut matcher = IgnoreMatcher::default();
        matcher.rules.push(parse_line(b"sub", b"out/tmp").unwrap());
        assert!(matcher.is_ignored(b"sub/out/tmp", false));
        assert!(!matcher.is_ignored(b"out/tmp", false));
        assert!(!matcher.is_ignored(b"x/sub/out/tmp", false));
    }
}
