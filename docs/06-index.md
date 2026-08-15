# 06 — Index

## Why

The index (staging area) is git's mechanism for building the next commit. It tracks which files are staged, their modes, and their sha1 hashes. Correct index read/write is essential for `git add`, `git status`, `git diff --cached`, and `git reset`.

## How

- **`IndexEntry`**: `ctime, mtime, dev, ino, mode, uid, gid, size, oid, flags, path` (stage-aware)
- **Read**: `DIRC` magic, version `2` (reject others), entry count, parse entries (62-byte fixed + NUL-terminated path, 8-byte aligned), verify trailing sha1 checksum
- **Write**: emit header + entries + sha1 of all preceding bytes
- **Stage/unstage helpers** for `add`/`reset`
- **Path handling**: paths stored as-is (no case folding, no normalization) — match real git's index paths exactly (slash separators, no leading `./`)
- **Entry fixed part (62 bytes)**: ctime sec+nsec (i32/i32), mtime sec+nsec, dev, ino, mode (u32), uid, gid, size (u32), oid (20 raw bytes), flags (u16) — flags: 1<<15 assume-valid, 1<<14 extended, stage in bits 12-13, path length in bits 0-11 (0x0FFF; longer paths use extended)
- **Padding**: entries padded with NULs so next entry starts at 8-byte-aligned offset (up to 8 NULs after path's NUL)
- **Preserve stat data and unknown flags on rewrite** — zeroing them breaks real git's racy-index detection; when we rewrite the index we round-trip entries we didn't touch verbatim
- **Checksum**: sha1 over header+entries appended as final 20 bytes; on read, verify it and report `Corrupt` on mismatch
- **Extensions**: version 3+ and extensions (TREE, REUC, link, sdir): rejected version > 2 in v1; extensions skipped by length field (4-byte signature + 4-byte length + data, loop until final 20-byte checksum); signatures validated as alphabetic; overruns are `Corrupt` (decision D-011)

## Usage

```bash
# Stage files
git-rs add file1.txt file2.txt

# Check staged status
git-rs status --short
# Output: XY PATH (staged vs worktree)

# Diff against HEAD
git-rs diff --cached

# Reset to a previous state
git-rs reset --hard HEAD
```

**Verification**: After real `git add`, we read the index and our `status` sees the same staged entries; after we write an index, real `git status` and `git diff --cached` agree with ours.