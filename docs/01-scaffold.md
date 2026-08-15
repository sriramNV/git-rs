# 01 — Scaffold & Core Primitives

## Why

The foundation of git-rs. Without a solid scaffold, no other features can be built. This step establishes the CLI argument dispatch, error handling framework, and project structure that every subsequent command relies on.

## How

- **CLI**: Custom argument dispatch using a static `[Command]` table — no `clap`, no derive macros. Args are `std::env::args()` collected and split by our own code.
- **Error handling**: `GitError` enum with variants `NotFound`, `Corrupt`, `Invalid`, `Io`. The `Io` variant carries context via `.context(path, op)` helper — always mention the path and operation. No `unwrap()`/`expect()` in non-test code.
- **Exit codes**: 0 success, 1 generic failure, 128 fatal. `main` returns `Process::exit(code)` after dispatch.
- **`--help`/`-h`**: Prints usage lines for every registered command and exits 0; `--version` prints `git-rs <version>`.

## Usage

```bash
git-rs --help              # Show all commands and usage
git-rs --version           # Print git-rs version
git-rs <command> <args>    # Run a specific command
```

**Smoke test**: `cargo run -- --help` exits 0 and shows usage; `cargo run -- nonexistent-command` exits 1.