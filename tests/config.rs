//! Config integration tests: our parser vs real `git config` on the same
//! files.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use git_rs::config::Config;

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn scratch_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "git-rs-cfg-{}-{name}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

fn git_config(dir: &PathBuf, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["config"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run real git config");
    assert!(
        out.status.success(),
        "git config {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A real git repo whose config we both read and compare against.
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

    fn set(&self, key: &str, value: &str) {
        git_config(&self.dir, &[key, value]);
    }

    fn get(&self, key: &str) -> String {
        git_config(&self.dir, &["--get", key])
    }

    fn config(&self) -> Config {
        Config::load_with(&self.dir.join(".git"), None).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn our_values_match_git_config_get() {
    let fixture = Fixture::new();
    fixture.set("user.name", "Test User");
    fixture.set("user.email", "test@example.com");
    fixture.set("init.defaultBranch", "main");
    fixture.set("core.filemode", "false");
    fixture.set("core.logallrefupdates", "true");

    let config = fixture.config();
    assert_eq!(config.get("user", "name"), Some("Test User"));
    assert_eq!(fixture.get("user.name"), "Test User");
    assert_eq!(config.get("user", "email"), Some("test@example.com"));
    assert_eq!(fixture.get("user.email"), "test@example.com");
    assert_eq!(config.get("init", "defaultbranch"), Some("main"));
    assert_eq!(fixture.get("init.defaultBranch"), "main");
    assert_eq!(config.get_bool("core", "filemode"), Some(false));
    assert_eq!(config.get_bool("core", "logallrefupdates"), Some(true));
    assert_eq!(fixture.get("core.filemode"), "false");
    assert_eq!(config.get("core", "repositoryformatversion"), Some("0"));
}

#[test]
fn global_layer_matches_git_config_file() {
    let fixture = Fixture::new();
    // Global file: user.name only. Repo config must win when set, global
    // must provide the fallback.
    let global_dir = scratch_dir("global");
    fs::create_dir_all(&global_dir).unwrap();
    let global_path = global_dir.join("gitconfig");
    fs::write(
        &global_path,
        "[user]\n\tname = Global User\n\temail = global@example.com\n",
    )
    .unwrap();

    let config = Config::load_with(&fixture.dir.join(".git"), Some(&global_path)).unwrap();
    assert_eq!(config.get("user", "name"), Some("Global User"));

    // Real git agrees when pointed at the same file.
    let real = git_config(
        &fixture.dir,
        &[
            "--file",
            global_path.to_str().unwrap(),
            "--get",
            "user.name",
        ],
    );
    assert_eq!(real, "Global User");

    // Repo override wins over global in ours and in git.
    fixture.set("user.name", "Repo User");
    let config = Config::load_with(&fixture.dir.join(".git"), Some(&global_path)).unwrap();
    assert_eq!(config.get("user", "name"), Some("Repo User"));
    let _ = fs::remove_file(&global_path);
}

#[test]
fn backslash_continuation_matches_git() {
    let fixture = Fixture::new();
    fs::write(
        fixture.dir.join(".git/config"),
        "[user]\n\tname = First\\\n\tSecond\n\temail = a@b.c\n",
    )
    .unwrap();
    let config = fixture.config();
    assert_eq!(config.get("user", "name"), Some("First\tSecond"));
    let real = git_config(&fixture.dir, &["--get", "user.name"]);
    assert_eq!(real, "First\tSecond");
}

#[test]
fn version_guard_rejects_upgraded_repo_like_git() {
    let fixture = Fixture::new();

    // Version 1 is accepted by real git 2.55; version 2 is not.
    fixture.set("core.repositoryformatversion", "1");
    fixture.config().check_repository_version().unwrap();

    fixture.set("core.repositoryformatversion", "2");
    let config = fixture.config();
    let err = config.check_repository_version().unwrap_err();
    assert!(
        err.to_string()
            .contains("Expected git repo version <= 1, found 2")
    );

    // Real git also refuses on commands that read config (git config itself
    // skips the check; git log does not):
    // "fatal: Expected git repo version <= 1, found 2"
    let out = Command::new("git")
        .args(["log", "--oneline"])
        .current_dir(&fixture.dir)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(128));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("Expected git repo version <= 1, found 2")
    );
}
