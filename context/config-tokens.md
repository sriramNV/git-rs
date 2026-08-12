# Configuration Tokens

Registry of config keys, env vars, and repository layout constants. Never hardcode tunables in module code — read through `config.rs`.

## Repository Layout (`.git/`)

| Path | Purpose |
|------|---------|
| `HEAD` | Symref to current branch: `ref: refs/heads/main` |
| `config` | Repo config INI |
| `index` | Staging area (index v2) |
| `objects/` | Loose objects + `packs/` |
| `refs/heads/`, `refs/tags/` | Branch / tag tip files |
| `packed-refs` | Packed refs (optional) |
| `logs/` | Reflogs |
| `hooks/` | Sample hooks (copied at init) |
| `info/` | `exclude`, `refs` |
| `description` | gitweb description |

## Config Keys (`config` / `~/.gitconfig`)

```ini
[core]
    repositoryformatversion = 0
    filemode = true
    bare = false
    logallrefupdates = true
    ignorecase = false
    precomposeunicode = false
    symlinks = true
[user]
    name = ...
    email = ...
[init]
    defaultBranch = main
[diff]
    renames = true    # interpreted, not stored
[core]
    excludesfile = ~/.gitignore_global
```

**Rules:** `repositoryformatversion != 0` → refuse to operate. Unknown `[section]` keys are ignored like real git does.

## Gitignore

- `.gitignore` files: one per directory, `!` negation, trailing `/` = directory-only, `**` patterns, last-match-wins within file
- Global ignore: `core.excludesFile`
- `.git/info/exclude` applies to repo
- Only tracked-file rules matter; untracked rules come from all three levels

## Environment Variables

| Variable | Overrides |
|----------|-----------|
| `GIT_DIR` | Repo directory (default `.git`) |
| `GIT_OBJECT_DIRECTORY` | Object store location |
| `GIT_INDEX_FILE` | Index path |
| `GIT_AUTHOR_NAME` / `GIT_AUTHOR_EMAIL` | Author identity |
| `GIT_COMMITTER_NAME` / `GIT_COMMITTER_EMAIL` | Committer identity |
| `GIT_AUTHOR_DATE` / `GIT_COMMITTER_DATE` | Commit timestamps |
| `GIT_CONFIG_GLOBAL` / `GIT_CONFIG_SYSTEM` | Config file paths |
| `GIT_EDITOR` | Editor for messages |
| `GIT_REFLOG_ACTION` | Reflog action prefix |

Env vars always take priority over file config. Config is read once per command invocation — never per operation.

## Invariants

- All config parsed through `config.rs` — no module reads config files directly
- Every new config key must be added here with a default before being referenced in code
- No hardcoded paths or magic numbers in commands — reference via constants here or in `store.rs`