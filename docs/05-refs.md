# 05 — Refs

## Why

Refs (references) are git's way of pointing to objects (usually commits). HEAD, branch refs, tags, and reflog entries must all work correctly for the rest of the system to function — branch creation, checkout, tag listing, and commit history all depend on refs.

## How

- **Ref name validation**: reject names containing `..`, starting with `.`, containing whitespace or `~^:?*[\\` or control chars
- **Atomicity**: temp file in the same directory as the target + `fs::rename` — never truncate a ref file in place
- **Ref paths**: come from ref names joined to `.git/refs/` — validate the name first to prevent path traversal
- **HEAD**: symref `ref: refs/heads/<branch>`; unborn HEAD resolves to `None`
- **Packed-refs reading**: header line, `<sha> <name>` lines, `^<sha>` peeled tags; loose ref wins over packed
- **Reflog line format**: `<old-sha> <new-sha> <name> <email> <ts> <tz>\t<message>` — message from the invoking command, e.g. `commit: <subject>`
- **`logallrefupdates=true`** (default): update reflog on every ref change
- **`Refs::update` skips reflog for `refs/tags/`** — real git never reflogs tags
- **`Refs::delete`**: removes reflog too

## Usage

```bash
# Create a branch
git-rs branch feature
git-rs checkout feature

# Create an annotated tag
git-rs tag -a -m "Version 1.0" v1.0

# Check reflog
git-rs log --all

# Update a ref
git-rs update-ref refs/heads/new-branch <sha>

# Delete a branch
git-rs branch -d feature
```

**Verification**: Create a branch + commit via our code, then real `git branch -v`, `git log`, `git reflog` show identical results.