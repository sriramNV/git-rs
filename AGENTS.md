# AGENTS.md

Git — reimplemented from scratch in Rust. Pure CLI, minimal dependencies (`sha1`, `flate2` only). Full local git; remote/wire protocol is explicitly out of scope for now.

## Before any work

Read these context files in order — they are the single source of truth:

1. `context/project-overview.md` — what this project is and its scope
2. `context/architecture.md` — module layout and data flow
3. `context/code-standards.md` — Rust conventions, error handling, testing
4. `context/rules.md` — git-format correctness rules (must not deviate)
5. `context/build-plan.md` — phases and current feature
6. `context/decisions.md` — recorded deviations from the plan; read before starting any feature
7. `context/module-registry.md` — what each module looks like, match existing patterns
8. `context/config-tokens.md` — config keys and env vars
9. `context/library-docs.md` — how we use sha1/flate2 here
10. `context/progress-tracker.md` — what is done, what is next, and the locked implementation instructions per step

Update `progress-tracker.md` after every completed feature and `module-registry.md` after every new module.

If implementation must deviate from what `progress-tracker.md` / `build-plan.md` document, record the deviation in `context/decisions.md` at the time it happens — never silently. Locked choices are marked bold in the tracker.

## Skills

Skills live in `skills/`:

- `/architect` — before building any feature: align on decisions, produce implementation plan
- `/review` — after building any feature: verify against plan, architecture, standards
- `/recover` — when something goes wrong: diagnose failure mode before responding
- `/remember save|restore` — save session state at end, restore at start

## Commands

```bash
cargo build      # build
cargo test       # run unit + integration tests
cargo run -- <args>   # run the git clone
```

## Dependency policy

- Allowed: `sha1` (hashing), `flate2` (zlib compression)
- Everything else is hand-written with std: CLI parsing, config, diff, merge, packfiles
- Never add a crate without updating `context/library-docs.md` and `context/code-standards.md`

## Verification

Every feature must end with: unit tests + a check against real git on the same repo (`git fsck`, `git log`, etc.). Compatibility with real git is the success criterion, not just self-consistency.
