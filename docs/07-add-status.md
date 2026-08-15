# 07 — Add & Status

## Why

`git add` updates the index with new or modified blobs, and `git status` shows the comparison between HEAD, the index, and the working tree. Together they form the core workflow for tracking changes.

## How

- **`git add`**: For each path, hash blob (or symlink target as mode `120000`), stage entry in index
- **`.gitignore` v1 matcher**: per-dir `.gitignore`, `!` negation, trailing `/` dirs, `**`, last-match-wins (rules in config-tokens.md)
- **`git status`**: three-way compare — HEAD tree vs index vs worktree
- **Porcelain short format** (`git status --short`): `XY PATH`
  - X = staged vs HEAD (`A` added, `M` modified, `D` deleted, `R` rename)
  - Y = worktree vs index (`M`, `D`, `?` untracked, ` ` clean)
  - Untracked shows as `?? PATH`
- **Renames in status**: reported only when both delete+add are detected as exact-content matches
- **Ignore rules**: apply to untracked files only — never to tracked file modification detection
- **`add`**: re-stage only what changed; keep index entries for unstaged paths untouched (round-trip them verbatim)

## Usage

```bash
# Add files to the index
git-rs add .
git-rs add file1.txt file2.txt

# Check status
git-rs status --short
# Output examples:
# M  modified.txt      # staged modification
# D  deleted.txt       # staged deletion
# A  new_file.txt      # staged new file
# ?? untracked.txt    # untracked file

# With rename detection (not in v1 default)
git-rs status -M
```

**Verification**: On the same repo, our `status --short` output is byte-identical to real `git status --short` across: new file, modified, deleted, staged-then-modified, untracked, ignored, symlink, subdir paths — locked in by tests/add_status.rs.