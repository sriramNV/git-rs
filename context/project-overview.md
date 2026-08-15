# Project Overview

## About the Project

A from-scratch reimplementation of git's core in Rust. A single binary (`git-rs`) that can initialize repositories, track files, create commits, navigate history, and merge branches — producing `.git` directories that are byte-compatible with real git.

## Scope — What We Build

Full local git functionality:

- Object model: blobs, trees, commits, tags (loose objects, SHA-1)
- The index (staging area)
- Working-tree operations: add, status, diff, checkout
- History: commit, log, show, branch, tag, reset
- Merging: 3-way merge, merge commits, rebase, stash
- Packfiles: read and write, delta encoding, gc
- Repository config, refs, packed-refs, reflog

## Out of Scope (for now)

- Remote protocol, fetch, push, pull, clone from a URL
- Submodules
- Partial clone, sparse checkout, worktrees
- git-lfs and other extensions
- HTTP smart/dumb transport

## Success Criteria

- Real git can operate on repos we create: `git fsck` clean, `git log` matches ours, `git diff` identical
- We can operate on repos real git created (read-only at minimum)
- Every object we write round-trips: `git fsck` finds nothing
- History integrity: every commit we create has the exact same SHA-1 that real git would compute for identical metadata

## Non-Goals

- Performance parity with git — correctness first, fast enough second
- Windows/macOS-specific plumbing beyond what the std library gives us for free
