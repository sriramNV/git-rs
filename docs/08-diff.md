# 08 — Diff

## Why

`git diff` shows changes between different tree states (HEAD, index, worktree). A correct Myers diff engine and unified output format are essential for developers to understand what changed.

## How

- **Myers O(ND) line diff**: content-valid, edit-distance-optimal difference algorithm
- **Line splitting**: split on `\n`, keep `\n` in line content — git's line diff operates on raw bytes, no CRLF stripping
- **Common prefix/suffix trim**: happens on the line arrays before running Myers (matches git's behavior, big speedup)
- **Unified renderer**: matches git's output byte-for-byte
  - `diff --git a/<path> b/<path>`
  - `index <sha>..<sha> <mode>`
  - `--- a/<path>`, `+++ b/<path>`
  - Hunks `@@ -s,c +s,c @@` — starting line (1-based), count; omit `,c` when count==1
  - New file: `index 0000000..<sha> <mode>` and `--- /dev/null`
  - Deleted: `index <sha>..0000000` and `+++ /dev/null`
- **Rename detection**: not in v1 diff output — renames show as delete+add unless `-M` given (v1: `-M` unsupported, flag prints "not supported" and exits 1)
- **Binary file detection**: if a blob contains NUL in first 8000 bytes → emit `Binary files a/x and b/x differ`, no diff body
- **No color** in v1 — plain text only
- **`-- funcname` suffix**: `@@ ... @@ <line>` with sticky like git's `def_ff`

## Usage

```bash
# Worktree vs index (unstaged changes)
git-rs diff

# Index vs HEAD (staged changes)
git-rs diff --cached

# With pathspec
git-rs diff -- path/to/file.txt

# Renames (v1 unsupported, exits 1)
git-rs diff -M 2>&1; echo "exit code: $?"

# Binary file handling
git-rs diff --binary-file-handling 2>&1
```

**Verification**: Byte-identical output vs real `git diff` / `git diff --cached` on: modified lines, insertions, deletions, hunks split across context, new/deleted files, binary files, files with trailing-newline differences — locked in by tests/diff.rs (distinct-line fixtures; identical-run boundary placement is D-014).