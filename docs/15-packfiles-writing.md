# 15 — Packfiles — Writing

## Why

Writing pack files enables `git repack`, `git gc`, and creating packs for efficient object storage. The pack format must be correct so that `git verify-pack` accepts our packs and `git cat-file` reads any object back identically.

## How

- **Object selection**: walk refs, collect reachable loose objects (reuse revwalk from 09), plus reflog-referenced objects (v1: skip reflog refs)
- **Sort**: by type then by oid — git's pack order: commit, tree, blob, tag... **locked choice: sort commits first, then trees, then blobs, each by oid**
- **Delta search**: for each blob, compare against up to window=10 previously-serialized blobs; keep best delta (smallest size); cap delta chain depth at 50
- **Write pack**: entries in sort order, delta entries for chosen pairs, 20-byte trailer = sha1 of all preceding bytes
- **Write idx v2 matching the pack**: fanout table, oid table, offset table, large-offset support (>2^31)
- **Thin pack**: no (no remote) — every delta base must be inside the pack
- **Entry data for non-delta**: raw deflate (`DeflateEncoder`) of header+content — NOT zlib (rules.md)
- **Sort order**: deterministic across runs (stable sort by (type-rank, oid))

## Usage

```bash
# Verify pack objects are readable
# Pack writing is primarily an internal operation,
# but objects written to pack are readable via:

git-rs cat-file -p <sha>
git-rs cat-file -t <sha>

# The pack objects are integrated into the object store
# and accessible through normal git-rs commands

# Example workflow (internal):
# git-rs repack-equivalent-flow
# git-rs verify-pack <pack-file>    # Verify the pack
```

**Verification**: `git verify-pack -v` passes on our pack; real `git fsck` clean after `git repack`-equivalent flow; every object read back from our pack matches its loose original sha.