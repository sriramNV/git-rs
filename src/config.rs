//! Config file parsing: INI-style repository and global configuration.
//!
//! Loaded once per command invocation and passed around as `&Config` —
//! never re-read per operation. Precedence: env vars (in the identity
//! getters) > repo `.git/config` > global `~/.gitconfig`.
//!
//! ponytail: subsections are parsed but collapsed into their section name
//! (`[remote "origin"]` and `[remote "upstream"]` share one slot, last
//! wins) — fine until we read per-subsection keys (decisions.md D-004).
//! Multi-line values, `include` directives, and system-level config are
//! not read in v1.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{GitError, Result};

/// Parsed configuration: repo layer and global layer, repo wins.
#[derive(Debug, Clone, Default)]
pub struct Config {
    repo: HashMap<(String, String), String>,
    global: HashMap<(String, String), String>,
}

impl Config {
    /// Load repo + global config from the environment.
    ///
    /// Repo config lives at `<GIT_DIR>/config` (or `.git/config`); the
    /// global file is `GIT_CONFIG_GLOBAL` (or `$HOME/.gitconfig`, falling
    /// back to `%USERPROFILE%\.gitconfig` on Windows). Missing files are
    /// empty layers, not errors. Malformed lines are errors.
    pub fn load() -> Result<Config> {
        let git_dir = match env::var("GIT_DIR") {
            Ok(dir) => PathBuf::from(dir),
            Err(_) => PathBuf::from(".git"),
        };
        let global = match global_config_path() {
            Some(path) => Some(path),
            None => None,
        };
        Self::load_with(&git_dir, global.as_deref())
    }

    /// Load config from explicit paths (tests use this; the environment is
    /// not consulted).
    pub fn load_with(git_dir: &Path, global: Option<&Path>) -> Result<Config> {
        let repo = read_layer(&git_dir.join("config"), "repo config")?;
        let global = match global {
            Some(path) => read_layer(path, "global config")?,
            None => HashMap::new(),
        };
        Ok(Config { repo, global })
    }

    /// Look up a config value: repo layer wins over global. Section and key
    /// are matched case-insensitively; values are returned verbatim.
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        let section = section.to_ascii_lowercase();
        let key = key.to_ascii_lowercase();
        self.repo
            .get(&(section.clone(), key.clone()))
            .or_else(|| self.global.get(&(section, key)))
            .map(|v| v.as_str())
    }

    /// Typed bool: `true/yes/on/1` and `false/no/off/0` (case-insensitive).
    pub fn get_bool(&self, section: &str, key: &str) -> Option<bool> {
        self.get(section, key).and_then(parse_bool)
    }

    /// Typed integer. ponytail: plain decimal only — git's `k/m/g`
    /// suffixes are not parsed in v1.
    pub fn get_int(&self, section: &str, key: &str) -> Option<i64> {
        self.get(section, key).and_then(|v| v.trim().parse().ok())
    }

    /// Refuse to operate on repositories with `core.repositoryformatversion`
    /// above 1, matching real git 2.55 (which accepts 0 and 1, rejects 2+).
    pub fn check_repository_version(&self) -> Result<()> {
        let version: i64 = self
            .get("core", "repositoryformatversion")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        if version > 1 {
            return Err(GitError::Invalid(format!(
                "Expected git repo version <= 1, found {version}"
            )));
        }
        Ok(())
    }

    /// Author identity: `GIT_AUTHOR_NAME`/`GIT_AUTHOR_EMAIL` env, else
    /// `user.name`/`user.email`.
    pub fn author_identity(&self) -> Option<(String, String)> {
        let name = env::var("GIT_AUTHOR_NAME")
            .ok()
            .or_else(|| self.get("user", "name").map(String::from))?;
        let email = env::var("GIT_AUTHOR_EMAIL")
            .ok()
            .or_else(|| self.get("user", "email").map(String::from))?;
        Some((name, email))
    }

    /// Committer identity: `GIT_COMMITTER_NAME`/`GIT_COMMITTER_EMAIL` env,
    /// else `user.name`/`user.email`.
    pub fn committer_identity(&self) -> Option<(String, String)> {
        let name = env::var("GIT_COMMITTER_NAME")
            .ok()
            .or_else(|| self.get("user", "name").map(String::from))?;
        let email = env::var("GIT_COMMITTER_EMAIL")
            .ok()
            .or_else(|| self.get("user", "email").map(String::from))?;
        Some((name, email))
    }

    /// Default identity (what commit uses when it does not distinguish
    /// author from committer) — the committer chain.
    pub fn user_identity(&self) -> Option<(String, String)> {
        self.committer_identity()
    }
}

/// Resolve the global config file path, if any.
fn global_config_path() -> Option<PathBuf> {
    if let Ok(path) = env::var("GIT_CONFIG_GLOBAL") {
        return Some(PathBuf::from(path));
    }
    let home = env::var("HOME").or_else(|_| env::var("USERPROFILE")).ok()?;
    Some(PathBuf::from(home).join(".gitconfig"))
}

/// Read one config layer. A missing file is an empty layer.
fn read_layer(path: &Path, what: &str) -> Result<HashMap<(String, String), String>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(e) => return Err(GitError::io(path.display().to_string(), what, e)),
    };
    // git tolerates non-UTF-8 bytes in config values; lossy conversion keeps
    // the parse going instead of failing the whole file.
    let text = String::from_utf8_lossy(&bytes);
    parse(&text, &path.display().to_string())
}

/// Parse INI-style config text into a `(section, key) -> value` map.
///
/// Sections: `[core]` or `[section "sub"]` (subsection case preserved but
/// collapsed into the section slot). Keys: `key = value`; a bare key is a
/// boolean true. Comments are `#` or `;`. Section and key names are
/// lowercased; values keep their case.
fn parse(text: &str, path: &str) -> Result<HashMap<(String, String), String>> {
    let mut map = HashMap::new();
    let mut section = String::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let n = i + 1;
        if line.starts_with('[') {
            section = parse_section(line, path, n)?;
            continue;
        }
        if section.is_empty() {
            return Err(bad_line(path, n, "key without a section"));
        }
        let (key, value) = match line.split_once('=') {
            Some((key, value)) => (key.trim(), value.trim()),
            None => (line, "true"),
        };
        map.insert((section.clone(), key.to_ascii_lowercase()), value.to_string());
    }
    Ok(map)
}

/// Parse a `[section]` / `[section "sub"]` line; returns the section name
/// lowercased. Missing `]` or an empty name is a bad line.
fn parse_section(line: &str, path: &str, n: usize) -> Result<String> {
    let inner = line
        .strip_prefix('[')
        .and_then(|l| l.strip_suffix(']'))
        .ok_or_else(|| bad_line(path, n, "missing ']'"))?;
    let name = inner.split_whitespace().next().unwrap_or("");
    if name.is_empty() {
        return Err(bad_line(path, n, "empty section name"));
    }
    Ok(name.to_ascii_lowercase())
}

fn bad_line(path: &str, n: usize, why: &str) -> GitError {
    GitError::Invalid(format!("bad config line {n} in {path}: {why}"))
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// Write a temp file and load it as both layers so precedence can be
    /// exercised. Returns the config and the paths.
    fn load_two(repo_text: &str, global_text: &str) -> (Config, PathBuf, PathBuf) {
        let base = env::temp_dir().join(format!(
            "git-rs-config-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let git_dir = base.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        let repo_path = git_dir.join("config");
        let global_path = base.join("gitconfig");
        fs::write(&repo_path, repo_text).unwrap();
        fs::write(&global_path, global_text).unwrap();
        let config = Config::load_with(&git_dir, Some(&global_path)).unwrap();
        (config, repo_path, global_path)
    }

    fn cleanup(paths: (Config, PathBuf, PathBuf)) {
        let (_, repo, global) = paths;
        let _ = fs::remove_file(repo);
        let _ = fs::remove_file(global);
    }

    #[test]
    fn parses_sections_keys_comments_and_blanks() {
        let text = "\n# a comment\n; another comment\n[core]\nfilemode = true\n\tsymlinks = true\n[user]\nname = Test User\nemail=test@example.com\n";
        let (config, _repo, _global) = load_two(text, "");
        assert_eq!(config.get("core", "filemode"), Some("true"));
        assert_eq!(config.get("Core", "FileMode"), Some("true"));
        assert_eq!(config.get("user", "name"), Some("Test User"));
        assert_eq!(config.get("user", "email"), Some("test@example.com"));
        assert_eq!(config.get("user", "email"), Some("test@example.com"));
        assert_eq!(config.get("user", "missing"), None);
        cleanup((config, _repo, _global));
    }

    #[test]
    fn values_keep_case_and_trim_whitespace() {
        let (config, _repo, _global) = load_two("[user]\nname =  Sriram N V \n", "");
        assert_eq!(config.get("user", "name"), Some("Sriram N V"));
        cleanup((config, _repo, _global));
    }

    #[test]
    fn bare_key_means_true() {
        let (config, _repo, _global) = load_two("[core]\nbare\nfilemode = false\n", "");
        assert_eq!(config.get_bool("core", "bare"), Some(true));
        assert_eq!(config.get_bool("core", "filemode"), Some(false));
        cleanup((config, _repo, _global));
    }

    #[test]
    fn subsection_is_collapsed_into_section() {
        let (config, _repo, _global) = load_two("[remote \"origin\"]\nurl = https://example.com\n", "");
        assert_eq!(config.get("remote", "url"), Some("https://example.com"));
        cleanup((config, _repo, _global));
    }

    #[test]
    fn repo_wins_over_global() {
        let (config, _repo, _global) = load_two("[user]\nname = Repo User\n", "[user]\nname = Global User\n");
        assert_eq!(config.get("user", "name"), Some("Repo User"));
        cleanup((config, _repo, _global));
    }

    #[test]
    fn global_is_fallback() {
        let (config, _repo, _global) = load_two("", "[user]\nname = Global User\n");
        assert_eq!(config.get("user", "name"), Some("Global User"));
        cleanup((config, _repo, _global));
    }

    #[test]
    fn bool_variants_parse() {
        let (config, _repo, _global) = load_two(
            "[core]\na = yes\nb = NO\nc = on\nd = 0\ne = 1\nf = maybe\n",
            "",
        );
        assert_eq!(config.get_bool("core", "a"), Some(true));
        assert_eq!(config.get_bool("core", "b"), Some(false));
        assert_eq!(config.get_bool("core", "c"), Some(true));
        assert_eq!(config.get_bool("core", "d"), Some(false));
        assert_eq!(config.get_bool("core", "e"), Some(true));
        assert_eq!(config.get_bool("core", "f"), None);
        cleanup((config, _repo, _global));
    }

    #[test]
    fn missing_file_is_an_empty_layer() {
        let base = env::temp_dir().join(format!(
            "git-rs-config-missing-{}",
            std::process::id()
        ));
        let git_dir = base.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        let config = Config::load_with(&git_dir, Some(&base.join("nope"))).unwrap();
        assert_eq!(config.get("user", "name"), None);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn malformed_lines_are_errors() {
        let base = env::temp_dir().join(format!(
            "git-rs-config-bad-{}",
            std::process::id()
        ));
        let git_dir = base.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(git_dir.join("config"), "[core\nfilemode = true\n").unwrap();
        let err = Config::load_with(&git_dir, None).unwrap_err();
        assert!(err.to_string().contains("bad config line 1"));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn key_without_section_is_an_error() {
        let base = env::temp_dir().join(format!(
            "git-rs-config-orphan-{}",
            std::process::id()
        ));
        let git_dir = base.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(git_dir.join("config"), "name = orphan\n").unwrap();
        let err = Config::load_with(&git_dir, None).unwrap_err();
        assert!(err.to_string().contains("key without a section"));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn version_guard_accepts_zero_and_absent() {
        let (config, _repo, _global) = load_two("[core]\nrepositoryformatversion = 0\n", "");
        config.check_repository_version().unwrap();
        cleanup((config, _repo, _global));

        let (config, _repo, _global) = load_two("[core]\nrepositoryformatversion = 1\n", "");
        config.check_repository_version().unwrap();
        cleanup((config, _repo, _global));

        let (config, _repo, _global) = load_two("", "");
        config.check_repository_version().unwrap();
        cleanup((config, _repo, _global));
    }

    #[test]
    fn version_guard_rejects_above_one() {
        let (config, _repo, _global) = load_two("[core]\nrepositoryformatversion = 2\n", "");
        let err = config.check_repository_version().unwrap_err();
        assert!(err.to_string().contains("Expected git repo version <= 1, found 2"));
        cleanup((config, _repo, _global));
    }

    #[test]
    fn identity_comes_from_config() {
        let (config, _repo, _global) = load_two(
            "[user]\nname = Test User\nemail = test@example.com\n",
            "",
        );
        assert_eq!(
            config.user_identity(),
            Some(("Test User".to_string(), "test@example.com".to_string()))
        );
        cleanup((config, _repo, _global));
    }

    #[test]
    fn identity_is_none_without_config() {
        let (config, _repo, _global) = load_two("", "");
        assert_eq!(config.user_identity(), None);
        cleanup((config, _repo, _global));
    }
}