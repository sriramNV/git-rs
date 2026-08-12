# Architecture

## Stack

| Layer | Choice | Notes |
|-------|--------|-------|
| Language | Rust, edition 2024 | |
| CLI parsing | Hand-written | No `clap` — manual arg dispatch in `cli.rs` |
| Hashing | `sha1` crate | Object IDs are SHA-1 hex |
| Compression | `flate2` | zlib for loose objects, raw deflate for packfiles |
| Errors | Hand-written error enum | No `thiserror`/`anyhow` — std only |

## Crate Layout

```
src/
├── main.rs               → parse args, dispatch to commands
├── cli.rs                → hand-written arg parsing, command table
├── error.rs              → GitError enum + Result alias
├── object/
│   ├── mod.rs            → Object enum, type dispatch
│   ├── blob.rs           → (trivial) blob parse/serialize
│   ├── tree.rs           → tree entry parse/serialize, mode handling
│   ├── commit.rs         → commit parse/serialize, author/committer dates
│   └── tag.rs            → annotated tag parse/serialize
├── store.rs              → loose object store: read/write via zlib, sha1
├── refs.rs               → refs, packed-refs, HEAD symrefs, atomic updates
├── index.rs              → index v2 read/write, checksum
├── config.rs             → .git/config + global config (ini-style parser)
├── diff.rs               → diff engine (Myers), rename detection
├── merge.rs              → 3-way merge, conflict markers
├── checkout.rs           → index → working tree materialization
├── pack.rs               → packfile/idx read+write, delta decode/encode
├── revwalk.rs            → commit graph walking, traversal options
└── commands/
    ├── mod.rs            → command dispatch table
    ├── init.rs
    ├── add.rs
    ├── commit.rs
    ├── status.rs
    ├── log.rs
    ├── diff.rs
    ├── branch.rs
    ├── checkout.rs
    ├── config.rs
    ├── merge.rs
    ├── rebase.rs
    ├── stash.rs
    ├── tag.rs
    ├── reset.rs
    ├── hash_object.rs    → plumbing: hash-object, cat-file, ls-tree, etc.
    └── fsck.rs
tests/
└── integration/          → end-to-end against real git
```

## Repository Layout (`.git/`)

```
.git/
├── HEAD                 → "ref: refs/heads/main"
├── config               → [core] repositoryformatversion=0, [user], ...
├── index                → staging area (v2)
├── objects/             → loose objects (xx/38hex) + packs/
│   └── packs/           → pack-*.pack + pack-*.idx
├── refs/
│   ├── heads/           → branch tips
│   ├── tags/            → tags
│   └── remotes/         → (unused until remote support)
├── packed-refs          → packed refs (optional)
├── logs/                → reflogs
├── hooks/               → sample hooks (copied at init)
├── info/                → exclude, refs
└── description          → for gitweb
```

## Data Flow

### Object write (add → commit)

```
working tree file
   │  hash-object / add
   ▼
blob: header "blob <size>\0" + content → sha1 → id
   │
   ▼
zlib-compressed → .git/objects/xx/38hex   (loose object write)
   │
   ▼
tree entries collected from index → tree object (recursive) → tree id
   │
   ▼
commit object: tree id + parent(s) + author/committer + message → commit id
   │
   ▼
ref update: .git/refs/heads/<branch> ← commit id (atomic tmp+rename)
```

### Read path (log, checkout, diff)

```
commit id → refs.rs resolves → loose object read (zlib decompress + sha1 verify)
   ▼
commit tree id → tree objects → blob(s)
   ▼
index vs tree diff → working-tree writes (checkout) or text diffs (log/diff)
```

## System Boundaries

| Owns | Module |
|------|--------|
| Object integrity, hashing, storage layout | `store.rs`, `object/` |
| Branch/tag state, HEAD | `refs.rs` |
| Staging state | `index.rs` |
| All config reading | `config.rs` |
| Text-level file changes | `diff.rs`, `merge.rs` |
| Compressed object batch storage | `pack.rs` |
| Command interpretation | `commands/`, `cli.rs` |

Nothing outside `store.rs` touches `.git/objects` directly. Nothing outside `index.rs` touches `.git/index`.