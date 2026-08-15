# 10 — Checkout, Branch, Tag, Reset

## Why

Checkout switches branches, branches create/manage lines of development, tags mark specific points, and reset moves HEAD and index/worktree to different states. These are core daily-driver commands.

## How

- **`git branch`**: `branch <name>` creates at HEAD or given rev; `-d` delete refuses current branch + unmerged; `-D` force; `-a`/`-l` list with `*`/`(HEAD detached at ...)` markers
- **`git checkout`**: `checkout <branch|tag|sha|rev>`, `-b <name>` create+switch, `-f` force, `-q` quiet
  - Materializes tree: `worktree::sync_worktree(store, root, old_tree, new_tree)` — write/overwrite changed files (temp+rename), delete gone files, prune empty dirs, leave untracked files alone
  - `force_sync_worktree` additionally overwrites tracked files whose content differs from the target oid (discard local edits) — used by `checkout -f` and `reset --hard`
  - **File-before-index ordering**: write worktree files FIRST, then rewrite the index (probed, must hold — stats stamped before overwrite make real `git status` report leftover ` M` on Windows, racy-mtime trap)
  - Checkout target resolution order: unresolved `-b <name>` → branch (refs/heads) → tag (refs/tags, peeled to commit) → revision-like via `resolve_rev`; `-b` with no target defaults to `HEAD`
  - **Dirty gate**: refuse the switch (unless `-f`) when the checked-out file set would be overwritten by local/index changes — same-tree switch while dirty IS allowed (probed); error text includes per-file paths + standard message (exit 1)
  - Unknown target → `error: pathspec 'x' did not match any file(s) known to git` (exit 1)

- **`git branch`**: list, create, delete branches
- **`git tag`**: lightweight `tag <name>`, annotated `-a -m`, `-l` list, `-d` delete; tag-dates/identity use committer chain + `commit::commit_dates`; tag name validation reuses `Refs::validate_name`; `tag -l` sorts plain lexicographic
- **`git reset`**: `--soft` (move ref only), `--mixed` (default, + reset index, `Unstaged changes after reset:` block), `--hard` (+ reset worktree), `-q`
  - `--mixed`: rewrite index to target tree with fresh stat
  - `--hard`: + `force_sync_worktree` then index
  - HEAD line: `HEAD is now at <7-sha> <subject>` (stdout, suppressed by `-q`)

## Usage

```bash
# Create and switch to a branch
git-rs checkout -b feature

# Switch branches
git-rs checkout main

# Create a tag
git-rs tag v1.0
git-rs tag -a -m "Version 1.0" v1.0

# List tags
git-rs tag -l

# Reset to a previous commit
git-rs reset --hard HEAD~1

# Reset mixed (unstage but keep worktree)
git-rs reset --mixed HEAD~1

# Reset soft (move ref only)
git-rs reset --soft HEAD~1

# Checkout a specific commit
git-rs checkout <sha>
```

**Verification**: `tests/checkout_branch_tag_reset.rs` (5 integration tests, all green): byte-equal command output vs real git on twin repos with fixed dates; checkout messages incl. detached `HEAD is now at ...`, `-b`, `-f` and interop — real git fsck-clean on repos we touched; branch lifecycle incl. merged/unmerged/current deletes; tag lifecycle incl. annotated object byte-compare; reset soft/mixed/hard incl. `Unstaged changes after reset:` block and `--hard` full-state parity.