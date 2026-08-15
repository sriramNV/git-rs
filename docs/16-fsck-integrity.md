# 16 — Fsck & Integrity

## Why

`git fsck` checks repository integrity — finding corrupt objects, dangling references (unreachable objects), missing refs, and other inconsistencies. It's the git-equivalent of a filesystem check.

## How

- **Roots**: every ref tip (05), reflog entries, index entries (06)
- **Reachability walk over commits/trees/blobs** (09 revwalk extended to trees): walk from ref tips, marking all reachable objects
- **Per-object**: read + verify hash (02 read path), report corrupt objects with path
- **Output**: corrupt objects, dangling objects (unreachable), missing refs; exit code 1 when anything is wrong, 0 when clean
- **Mirror real `git fsck` behavior**: report each issue on its own line, e.g. `error: object <sha>: corrupt` / `dangling commit <sha>`; exit 1 on errors
- **v1 scope**: no `--strict`, no fsck.<msg-id> machinery, no connectivity check across packs beyond resolution (that's 14's job)
- **Report order**: traversal order (deterministic — walk commits, then their trees)

## Usage

```bash
# Check repository integrity
git-rs fsck

# Check with --no-dangling (only report errors, not dangling objects)
git-rs fsck --no-dangling

# Expected output on a clean repo:
# (no output, exit 0)

# Expected output on a corrupted repo:
# error: object <sha>: corrupt
# dangling blob <sha>
# missing ref refs/heads/nonexistent
# exit code: 1
```

**Verification**: Deliberately corrupt a repo (flip a byte in an object file, truncate one, break a ref) → our fsck and real `git fsck` report the same findings; clean repo → both exit 0.

**Known test**: `fsck_stays_clean_after_our_writes` — verifies our fsck is clean after writing objects.