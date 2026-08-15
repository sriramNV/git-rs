# Build Plan

## Core Principle

Build one feature at a time — fully functional, tested, and verified against real git — before moving on. The object store is the foundation; everything else layers on it. Compatibility with real git is the definition of done.

---

## Phase 1 — Scaffold & Core Primitives

### 01 Project Scaffold
- `cargo new`, crate name `git-rs`, minimal deps: `sha1`, `flate2`
- `cli.rs` argument dispatch skeleton, `GitError` enum, `Result` alias
- Verify: `cargo build` clean, `cargo run -- --help` shows usage

### 02 Object Store — Loose Objects
- `store.rs`: read/write loose objects (zlib + sha1), object-type dispatch
- Plumbing helpers: `hash-object -w`, `cat-file`
- Verify: hash a file, `git cat-file` reads it back from a real repo and vice versa

### 03 Config
- `config.rs`: INI parser, `[core]`/`[user]` handling, env var overrides
- Verify: our config parses a real repo's `.git/config` identically

---

## Phase 2 — Objects

### 04 Tree & Commit Objects
- `object/tree.rs`, `object/commit.rs`: parse + serialize
- Verify: our parsed commit matches `git cat-file -p`, our written trees/commits pass `git fsck`

### 05 Refs
- `refs.rs`: HEAD, branches, tags, symrefs, atomic updates, packed-refs read
- Verify: create branch/commit with our code, `git branch` / `git log` on same repo agrees

### 06 Index
- `index.rs`: index v2 read/write with checksum validation
- Verify: `git add` in real repo, we read the index; we write an index, `git status` agrees

---

## Phase 3 — Working Tree & History

### 07 Add & Status
- `commands/add.rs`: hash objects, update index
- `commands/status.rs`: index vs HEAD vs working tree comparison
- Verify: `git status --short` and ours identical on same repo

### 08 Diff
- `diff.rs`: Myers line diff, unified output format matching git's exactly
- `commands/diff.rs`: `git diff`, `git diff --cached`
- Verify: byte-identical output against real `git diff`

### 09 Commit, Log & Show
- `commands/commit.rs`, `commands/log.rs`, `commands/show.rs`
- `revwalk.rs`: traversal, `--oneline`, `--graph`, date ordering
- Verify: identical `git log --oneline` output; commit SHAs match real git for equal input

### 10 Checkout, Branch, Tag, Reset
- `commands/checkout.rs`: tree → working tree materialization
- `commands/branch.rs`, `commands/tag.rs` (lightweight + annotated), `commands/reset.rs`
- Verify: `git status` clean after our checkout; real git can check out our tags

---

## Phase 4 — Merging & History Editing

### 11 Three-Way Merge
- `merge.rs`: merge-base, recursive merge, conflict markers
- `commands/merge.rs`: merge commits, `--no-ff`, branch removal on success
- Verify: real `git merge` produces same result tree on same inputs; conflicts byte-identical

### 12 Rebase
- `commands/rebase.rs`: replay commits onto new base, conflict handling
- Verify: rebased history has same content as real git's (SHAs may differ with same author dates — compare trees)

### 13 Stash
- `commands/stash.rs`: save/pop (commit on refs/stash), list
- Verify: `git stash list` in real git sees our stashes

---

## Phase 5 — Packfiles & Hardening

### 14 Packfiles — Reading
- `pack.rs`: pack/idx parsing, OFS_DELTA/REF_DELTA resolution, verify checksums
- Verify: `git log` over a repo real git packed; `git fsck` clean

### 15 Packfiles — Writing
- `pack.rs`: delta search, deltification, idx writing, `pack-objects`-style CLI (plumbing `git-rs pack-objects`)
- Verify: our pack passes `git verify-pack`; objects readable

### 16 Fsck & Integrity
- `commands/fsck.rs`: full-object walk, hash verification, ref graph validation
- Verify: `git fsck` and ours report same errors on deliberately corrupted repos

### 17 Hardening & Compat Pass
- Cross-check every command against real git on tricky repos: renames, symlinks, empty trees, CRLF files, large files
- Exit codes, error messages, `git status --porcelain` format compatibility

---

## Feature Count

| Phase | Features |
|-------|----------|
| Phase 1 — Scaffold & Core | 3 |
| Phase 2 — Objects | 3 |
| Phase 3 — Working Tree & History | 4 |
| Phase 4 — Merging | 3 |
| Phase 5 — Packfiles & Hardening | 4 |
| **Total** | **17** |