# 04 — Tree & Commit Objects

## Why

Tree and commit objects are the core data structures in git. Trees represent directory entries; commits tie together trees, authorship, and parent relationships. These must parse and serialize correctly to match real git — `git cat-file -p` and `git commit-tree` must produce identical output.

## How

- **Tree entries**: sorted by `base_name_compare` exactly — compare names bytewise, trailing `/` appended as tiebreaker, directory flag (bit 0x4000) is the tiebreaker when names are otherwise equal
- **Tree modes**: `100644` regular, `100755` executable, `120000` symlink, `040000` subtree, `160000` gitlink (parse-only in v1)
- **Commit parse**: strict order — `tree` line first, then `parent` lines (0+), then `author`, then `committer`, then blank line, then message. Reject anything violating this as `Corrupt`
- **Timestamps**: unix seconds + tz offset (`+0530`, `-0700`), validate tz range; invalid dates are `Invalid` (real git rejects them)
- **Tag**: `object <sha>\ntype <type>\ntag <name>\ntagger <ident>\n\n<message>`
- **cat-file -p**: pretty-prints trees in ls-tree format — `<6-digit octal mode> <type> <sha>\t<name>` per entry, entries joined by `\n` with NO trailing newline — while blobs/commits/tags print raw content

## Usage

```bash
# Create a commit object
git-rs commit -m "My commit message"

# Cat-file to verify
git-rs cat-file -p <sha>
git-rs cat-file -t <sha>    # Returns: commit, tree, blob, or tag
git-rs cat-file -s <sha>    # Returns byte size of content

# Verify commit matches real git
# For identical input (tree, identity, dates), our commit sha equals real git commit-tree's sha
```

**Verification**: For identical input, our commit sha matches real `git commit-tree`'s sha; `git cat-file -p` output matches our parse of a real repo's objects; `git fsck` clean on trees/commits we write.