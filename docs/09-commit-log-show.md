# 09 — Commit, Log & Show

## Why

`git commit` creates new commits, `git log` displays history, and `git show` displays object contents. These commands must work identically to real git for the system to be usable as a git replacement.

## How

- **`git commit`**: `-m <msg>`, `-a` (stage modified tracked files first), empty-commit check (`nothing to commit`), author/committer from config/env, update ref + reflog
- **Identity**: author and committer are separate — both default to `user.name`/`user.email` (env overrides separately); `-a` only stages modified tracked files, never new untracked files
- **Empty commit**: if new tree == parent tree and no `--allow-empty`, print `nothing to commit, working tree clean` (exit 0, no commit)
- **`git log`**: traversal from HEAD, commit-date order (newest first), `--oneline` = `<short-sha> <subject>` (subject = first line of message), `--all` seeds from every ref, `--graph` v1: simple left-column pipe rendering, `*` per commit — no merge-corner glyphs yet
- **`git show`**: commit summary + `--stat`-style patch
  - Header: `commit <sha>`, `Author:`, `Date:` in the ident's tz via hand-rolled `civil_from_days`, indented message
  - Stat block: per-file `<name> | <n> <+->` + summary with singular/plural
  - Patch body: skipped in v1
- **Reflog message**: `commit: <subject>` (or `commit (amend): ...` later)

## Usage

```bash
# Commit changes
git-rs commit -m "My commit message"
git-rs commit -a -m "Message with auto-staged files"

# View commit history
git-rs log --oneline
# Output: <sha> <subject> (one line per commit)

# View log with graph
git-rs log --graph --oneline

# Show commit details
git-rs show <sha>

# Show stats only
git-rs show --stat <sha>
```

**Verification**: Same repo + same identity/timestamps → our commit sha matches real `git commit`; `git log --oneline` output identical; real `git log --all` traverses our commits correctly — locked in by tests/log_commit.rs.