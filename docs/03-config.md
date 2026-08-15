# 03 — Config

## Why

Configuration is essential for repository identity (user name/email), feature flags (core.repositoryformatversion), and behavior settings (core.filemode, core.ignorecase). The config parser must handle real repo config files and global config, matching real git behavior.

## How

- **INI parser**: `[section]`, `[section "subsection"]`, `key = value`, `#`/`;` comments
- **Load order**: repo `.git/config`, then global `~/.gitconfig` (`GIT_CONFIG_GLOBAL` overrides path), env vars win over both
- **`Config::get(section, key) -> Option<String>`**: typed getters for bool/int parse on demand
- **`repositoryformatversion` check**: accepts 0 and 1, rejects 2+ (see decisions.md D-005)
- **`user_identity()`**: name/email from config or `GIT_AUTHOR_NAME/EMAIL`, `GIT_COMMITTER_NAME/EMAIL`
- **Unknown sections/keys**: ignored silently — same as real git
- **`[core]` keys read by v1**: `repositoryformatversion`, `filemode`, `bare`, `logallrefupdates`, `ignorecase`, `symlinks`; everything else read on demand

## Usage

```bash
# Git-rs reads config automatically
git-rs commit -m "Message"   # Uses identity from config
git-rs status                # Shows tracked config values

# Override via env vars
GIT_AUTHOR_NAME="Sriram" git-rs commit -m "Test"

# Check repo format version
git-rs config repositoryformatversion
# Output: 0 or 1 (git-rs specific)
```

**Verification**: Parse a real repo's `.git/config` and a real `~/.gitconfig`; assert our `get()` values match `git config --get` for the same keys.