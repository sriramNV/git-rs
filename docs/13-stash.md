# 13 — Stash

## Why

`git stash` saves the current worktree/index state to be restored later, typically before switching branches. It's essential for interrupting work without committing unfinished changes.

## How

- **`git stash` (save)**: 
  - Index commit (tree = current index, parent = HEAD)
  - Worktree commit (tree = worktree state, parent = index commit)
  - Write `refs/stash` + reflog `stash@{0}` semantics
  - Stash commit message: `WIP on <branch>: <short-head-sha> <head-subject>` — real git parses this for `stash show` display
  - Reflog for `refs/stash`: `stash@{0}` shows in `git reflog` with action message like `WIP on <branch>: ...`
  - Untracked files: not included in v1 (plain `git stash` — no `-u`)

- **`git stash list`**: shows stash entries from reflog

- **`git stash pop`**: 
  - 3-way restore: worktree changes from worktree commit (11), index from index commit, then drop
  - On conflict: reuse 11's conflict markers; on conflict, keep the stash (do not drop) — `git stash pop` drops only on clean apply
  - After pop, index restore: stage the index commit's tree entries onto current index (paths from index commit tree)

## Usage

```bash
# Stash current changes
git-rs stash

# Stash with a message
git-rs stash save "WIP: implementing feature"

# List stashes
git-rs stash list

# Pop the most recent stash
git-rs stash pop

# Show stash diff
git-rs stash show -p

# Drop a stash
git-rs stash drop stash@{0}
```

**Verification**: real `git stash list` shows our stash; real `git stash show -p` prints the right diff; `git stash pop` (ours) restores worktree + index; dropping updates reflog.