# Code Standards

Implementation rules for every session. The AI agent must follow these without exception. No pattern drift across sessions.

## Engineering Mindset

- **Think before implementing** — understand what is being built and why before writing a line
- **Read context files first** — never assume; verify against architecture.md and rules.md
- **Scope is sacred** — only build what the current feature requires
- **Every feature must be testable** — if it cannot be verified, it is incomplete
- **Clean over clever** — simple readable code wins over clever abstractions
- **One thing at a time** — complete one feature fully before touching the next
- **Compatibility is the goal** — code is done only when a check against real git passes

## Rust Conventions

- Edition 2024 in Cargo.toml
- `#![deny(unsafe_code, unused_must_use)]` in lib.rs
- Never use `unsafe`. Never use `unwrap()`/`expect()` in production code — return `Result`
- All public functions must have doc comments (`///`)
- Errors: single `GitError` enum in `error.rs` with `Display` impl (no thiserror/anyhow)
- Logging: nothing fancy — `eprintln!` for warnings/errors, no tracing dependency. Keep messages stable.
- Use `#[derive(Debug, Clone, PartialEq)]` liberally on data types
- Module structure: `mod.rs` re-exports public items
- Bytes: use `u8` slices and `b"..."` literals for format-level code; `String` only at CLI boundaries

### File Order

```rust
// 1. Module doc comment
//! Loose object storage.

// 2. Std imports
use std::fs;
use std::path::PathBuf;

// 3. Crate imports
use crate::error::{GitError, Result};

// 4. Constants
const OBJECT_DIR: &str = "objects";

// 5. Types
pub struct ObjectStore { root: PathBuf }

// 6. Public impl
impl ObjectStore {
    pub fn write_blob(&self, data: &[u8]) -> Result<String> { /* ... */ }
}

// 7. Private helpers
fn header_for(kind: &str, size: usize) -> Vec<u8> { /* ... */ }
```

### Error Handling

```rust
#[derive(Debug)]
pub enum GitError {
    NotFound(String),          // object/ref/path not found
    Corrupt(String),           // format violation discovered on read
    Invalid(String),           // user input error
    Fatal(String),             // user-input error real git reports as fatal (exit 128)
    Io(String),                // wrapped io::Error with context
}

impl std::fmt::Display for GitError { /* ... */ }
impl From<std::io::Error> for GitError { /* ... */ }
```

- Always wrap `io::Error` with context: which path, which operation
- Format-corruption errors are `Corrupt` — never panic, never silently skip

## Testing

- Unit tests inline in each module (`#[cfg(test)] mod tests`)
- Integration tests in `tests/` exercising the binary end to end
- Every feature must end with a **real-git check**: run `git fsck`, `git log`, `git diff` against the repo the feature produced
- Test fixtures: create repos via the same code paths as real usage (no hand-crafted binary fixtures unless a format is read-only)

## Comments

- Comments for **why**, never **what** — code must be self-explanatory
- No TODO comments in committed code — track in progress-tracker.md
- Format-level code (object headers, pack parsing, index layout) gets doc comments naming the exact git format being mirrored

## Dependencies

Never add a crate without:

1. Checking if std already provides the functionality
2. Updating `library-docs.md` and `code-standards.md`

### Approved Crates

| Crate | Purpose |
|-------|---------|
| `sha1` | SHA-1 for object IDs |
| `flate2` | zlib (loose objects) + deflate (packfiles) |

These two only. Everything else — CLI parsing, config parsing, diff, merge, packfile delta encoding, revwalk — is hand-written or depends on them.