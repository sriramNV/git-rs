# git-rs — A Git Reimplementation in Rust

**git-rs** is a pure CLI git reimplementation built with Rust, using only minimal dependencies:
- `sha1` — for object hashing
- `flate2` — for zlib compression

All other functionality is hand-written with the standard library. The goal is full local git compatibility — supporting the same commands, formats, and behaviors as real git for local repository operations.

## Status

| Metric | Value |
|--------|-------|
| **Total Steps** | 17 |
| **Tests Passing** | 197/197 (124 unit + 73 integration) |
| **Build** | `cargo build` — clean |
| **Real Git Compatibility** | Verified: `git fsck`, `git stash`, `git log`, `git status`, `git diff`, `git merge`, `git rebase` all work |

## Quick Start

```bash
# Clone and build
git clone <repo>
cd git-rs
cargo build            # Build the project
cargo test             # Run all 197 tests

# Basic usage
git-rs --help          # Show usage
git-rs <command>       # Run a command
```

## Documentation Structure

This guide is split into separate topic files in the `docs/` folder. Each file covers:
- **Why** — the design rationale and compatibility considerations
- **How** — the implementation approach
- **Usage** — how to use the command or feature

### Available Docs

| File | Feature |
|------|---------|
| `01-scaffold.md` | Project scaffold, CLI, error handling |
| `02-object-store.md` | Loose objects, hash-object, cat-file |
| `03-config.md` | INI config parser, repository format version |
| `04-tree-commit.md` | Tree and commit objects, parsing, serialization |
| `05-refs.md` | Ref name validation, HEAD, branches, tags, reflog |
| `06-index.md` | Index v2 read/write, stage/unstage helpers |
| `07-add-status.md` | git add, git status --short |
| `08-diff.md` | Myers line diff, unified output format |
| `09-commit-log-show.md` | git commit, git log, git show |
| `10-checkout-branch-tag.md` | git checkout, git branch, git tag, git reset |
| `11-merge.md` | Three-way merge, conflict resolution |
| `12-rebase.md` | Git rebase implementation |
| `13-stash.md` | git stash save/list/pop/drop |
| `14-packfiles-reading.md` | Pack idx v2 parsing, delta resolution |
| `15-packfiles-writing.md` | Pack object writing, idx v2 construction |
| `16-fsck-integrity.md` | git fsck — repository integrity check |
| `17-hardening.md` | Hardening & compatibility pass |

## Phase Overview

The project is built in 5 phases:

**Phase 1 — Scaffold & Core Primitives** (Steps 01-03)
- Project setup, CLI argument parsing, error handling, config

**Phase 2 — Objects** (Steps 04-06)
- Tree and commit objects, refs, index

**Phase 3 — Working Tree & History** (Steps 07-09)
- add/status, diff, commit/log/show

**Phase 4 — Merging & History Editing** (Steps 10-12)
- checkout/branch/tag/reset, merge, rebase

**Phase 5 — Packfiles & Hardening** (Steps 13-17)
- stash, packfiles reading/writing, fsck, hardening

## Philosophy

- **Compatibility first** — real git is the success criterion, not self-consistency
- **Minimal dependencies** — only `sha1` and `flate2`; everything else is hand-written
- **One feature at a time** — fully functional and tested before moving on
- **Ponytail development** — lazy, efficient solutions; stdlib and native features first; no speculative abstractions

## Building & Testing

```bash
cargo build      # Build git-rs
cargo test       # Run 197 tests (124 unit + 73 integration)
cargo run -- --help  # Show command usage
```

## Notes

- Commands run from the repository root (`.git/` discovered from `GIT_OBJECT_DIRECTORY` env or `GIT_DIR` + `objects/`)
- Windows path compatibility; some decisions documented in `context/decisions.md`
- Integration tests use fixed dates for byte-identical output against real git 2.55
- Deviations from real git are documented in `context/decisions.md` with reasons

---

*git-rs: Git reimplemented from scratch in Rust. Pure CLI, minimal dependencies (`sha1`, `flate2` only). Full local git; remote/wire protocol is explicitly out of scope.*