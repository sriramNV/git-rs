# Module Registry

Living document. Updated after every module is built. Read before building — match existing patterns before inventing new ones.

---

## Baseline — Established 2026-08-12

No modules built yet. Registry will fill in as features land.

---

## Core Modules

### ObjectStore
File: `src/store.rs`

| Property | Value |
|----------|-------|
| Purpose | Loose object read/write, hashing, storage layout |
| Format | zlib(header + content), `.git/objects/xx/38hex` |
| Depends on | `sha1`, `flate2` (ZlibEncoder/Decoder) |
| Key methods | `write_blob(&[u8])`, `read_object(&str)`, `object_path(&str)`, `hash(kind, content)` |

### GitError
File: `src/error.rs`

| Property | Value |
|----------|-------|
| Purpose | Single error enum: `NotFound`, `Corrupt`, `Invalid`, `Io` |
| Key ids | Associated `Result<T>` alias used everywhere |

---

## Object Types (`src/object/`)

### Blob / Tree / Commit / Tag
Files: `blob.rs`, `tree.rs`, `commit.rs`, `tag.rs`

| Property | Value |
|----------|-------|
| Purpose | Parse + serialize each object kind |
| Tree entries | `<mode> <name>\0<20-byte-sha>`, sorted per `base_name_compare` |
| Commit | tree + parents + author/committer + message |
| Key methods | `parse(bytes)`, `serialize()`, per-type accessors |

---

## State Modules

### Refs
File: `src/refs.rs`

| Property | Value |
|----------|-------|
| Purpose | HEAD, branch/tag tips, symrefs, packed-refs, atomic updates |
| Update | temp file + rename, never in-place |
| Key methods | `resolve(HEAD)`, `update(name, sha)`, `list()`, `read_packed_refs()` |

### IndexStore
File: `src/index.rs`

| Property | Value |
|----------|-------|
| Purpose | Index v2 read/write, checksum |
| Format | `DIRC` + v2 + entries (62-byte + path, 8-byte aligned) + sha1 |
| Key methods | `read()`, `write()`, `stage(path, sha, mode)`, `entries()` |

### Config
File: `src/config.rs`

| Property | Value |
|----------|-------|
| Purpose | INI-style config parsing: repo + global + env overrides |
| Key methods | `load(dir)`, `get(section, key)`, `user_identity()` |

---

## Algorithm Modules

### DiffEngine
File: `src/diff.rs`

| Property | Value |
|----------|-------|
| Purpose | Myers line diff, unified output matching git's format |
| Key methods | `diff(a, b)`, `render_unified(..., context)`, `detect_renames` |

### ThreeWayMerge
File: `src/merge.rs`

| Property | Value |
|----------|-------|
| Purpose | Merge-base, recursive merge, conflict markers |
| Key methods | `merge_base(a, b)`, `merge_trees(base, ours, theirs)`, `write_conflict_markers` |

### Checkout
File: `src/checkout.rs`

| Property | Value |
|----------|-------|
| Purpose | Materialize index/tree into working tree |
| Key methods | `checkout_tree(tree_sha)`, `reset_hard(tree_sha)`, `unchanged(path)` |

### RevWalk
File: `src/revwalk.rs`

| Property | Value |
|----------|-------|
| Purpose | Commit graph traversal, ordering, graph rendering |
| Key methods | `walk(from_commits, opts)`, `oneline()`, `graph()` |

### PackStore
File: `src/pack.rs`

| Property | Value |
|----------|-------|
| Purpose | Pack/idx read+write, OFS_DELTA/REF_DELTA |
| Key methods | `read_pack()`, `resolve_delta()`, `write_pack(objs)`, `write_idx()` |

---

## Command Modules (`src/commands/`)

One file per command, thin wrappers over state/algorithm modules.

| Command | File | Behavior |
|---------|------|----------|
| `init` | `init.rs` | Create `.git` skeleton, default branch |
| `add` | `add.rs` | Hash + stage files |
| `commit` | `commit.rs` | Tree from index → commit → ref update |
| `status` | `status.rs` | index vs HEAD vs worktree |
| `log` / `show` | `log.rs` | History walk + diff display |
| `diff` | `diff.rs` | Worktree/cached/commit diffs |
| `branch` / `tag` | `branch.rs` | Ref listing/creation |
| `checkout` | `checkout.rs` | Switch branch/tree, materialize |
| `config` | `config.rs` | Read/write config values |
| `merge` / `rebase` / `stash` | `merge.rs` | 3-way merge, replay, stash |
| `reset` | `reset.rs` | Move ref, optionally reset worktree |
| plumbing | `hash_object.rs` | hash-object, cat-file, ls-tree, update-ref, etc. |
| `fsck` | `fsck.rs` | Full integrity walk |