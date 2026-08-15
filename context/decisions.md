# Decisions

Record of every deliberate deviation from the plan (`build-plan.md` / `progress-tracker.md`). The plan documents how things are *supposed* to be built; this file documents when reality forced or justified something different — and why.

Without this file, a future session sees the plan, assumes it was followed, and re-introduces inconsistency. Read this file before starting any feature.

---

## How to Use

**Before starting a feature:** check for entries tagged with that step number. If one exists, it overrides what the plan says.

**When deviating:** the moment implementation would do something the plan doesn't say (or says differently), write the entry *at that time* — not at the end, not "later", and never silently. A deviation is only a problem if it happens off the record.

**What counts as a deviation:**

- A locked (bold) choice in progress-tracker.md that we didn't follow
- A format/behavior choice where the plan didn't specify and real git is ambiguous
- A deliberate scope cut (e.g., "not implementing X in v1")
- An added dependency, command, or feature not in the plan

**What does not** belong here: routine bug fixes, refactors that don't change behavior, or anything already specified in the plan.

---

## Entry Template

Copy this for each new decision. Number sequentially (`D-001`, `D-002`, ...). Append at the end of the log — never rewrite history.

```markdown
## D-00X — <Short title>

- **Date:** YYYY-MM-DD
- **Step(s) affected:** <step numbers from progress-tracker.md, or "global">
- **Plan said:** <what the plan/progress-tracker specifies>
- **Decision:** <what we actually did, precisely>
- **Why:** <the concrete reason — a real git behavior found, a constraint, a time-box>
- **Impact:** <what downstream work depends on this, what would break if reversed>
```

---

## Decision Log

### D-002 — No upward-directory walk for repository discovery (v1)

- **Date:** 2026-08-12
- **Step(s) affected:** 02 (ObjectStore::discover)
- **Plan said:** not specified; real git walks up parent directories to find `.git`.
- **Decision:** v1 resolves the repo from the current directory only: `GIT_OBJECT_DIRECTORY` env → `GIT_DIR` env + `objects` → `<cwd>/.git/objects`. No upward walk.
- **Why:** every command in the current steps runs from the repo root; the walk is ~15 lines but needs care (symlinks, mount boundaries, env interplay) — deferring keeps step 02 focused.
- **Impact:** commands fail with "Not a valid object name"-style errors when run from a subdirectory of a repo. Add the walk when subdirectory usage matters (before step 07 status, which users naturally run from anywhere).

### D-003 — `cat-file` missing-object message differs from git 2.55

- **Date:** 2026-08-12
- **Step(s) affected:** 02
- **Plan said:** step 17 will align error-message parity; step 02 locked the `fatal: Not a valid object name <id>` shape.
- **Decision:** keep `Not a valid object name <id>` (exit 128) in v1.
- **Why:** real git 2.55 on Windows prints `fatal: git cat-file: could not get object info` for `-t`/`-s` on a missing object, but `Not a valid object name` for malformed names with `-p`. We picked the classic message for both; both exit 128.
- **Impact:** step 17 must compare messages per option and switch to per-flag messages if parity is required.

### D-004 — Config subsections collapsed into section slot

- **Date:** 2026-08-12
- **Step(s) affected:** 03 (Config::parse)
- **Plan said:** not specified; real git distinguishes subsections (`[remote "origin"]` vs `[remote "upstream"]` are separate scopes).
- **Decision:** v1 parses subsections but collapses them: the subsection name is discarded and the key is stored under the bare section name — last one wins.
- **Why:** no step before 07 (status) or 13 (remote config) reads per-subsection keys; storing the real key shape `(section, subsection, key)` would ripple through every `get` call site now.
- **Impact:** `[remote "origin"]` and `[remote "upstream"]` share one slot. If step 13 needs both, parse must keep the subsection. Code carries a `ponytail:` comment pointing here.

### D-006 — Bad ident dates: `Corrupt` on read, `Invalid` on write

- **Date:** 2026-08-12
- **Step(s) affected:** 04 (object/commit.rs, object/tag.rs)
- **Plan said:** progress-tracker 04: "invalid dates are `Invalid` (real git rejects them)" — one variant for both paths.
- **Decision:** the error variant depends on the direction. `Ident::new` (write path) rejects out-of-range tz (`!(-1200..=1400)`) with `Invalid`. `Ident::parse` (read path) rejects malformed/unparseable dates with `Corrupt` — the same shape as every other format violation in object parsing (unknown header lines, bad oids).
- **Why:** read-path failures in a store are format violations of stored data (`Corrupt`, exit 128); write-path failures are user input (`Invalid`). Real git refuses bad dates at write time; on read it treats malformed headers as an error during object load.
- **Impact:** `fsck`-style code and `cat-file` get `Corrupt` for garbage stored objects; `commit`/`tag` writers get `Invalid` for user-supplied dates. Both are exercised by unit tests (`bad_dates_are_rejected`).

### D-005 — Repository format version guard matches git 2.55, not the plan

- **Date:** 2026-08-12
- **Step(s) affected:** 03 (Config::check_repository_version)
- **Plan said:** reject `core.repositoryformatversion > 0` with "Expected git repo version <= 0, found N".
- **Decision:** accept 0 and 1, reject 2+ with "Expected git repo version <= 1, found N".
- **Why:** verified against real git 2.55 on Windows: a repo with `repositoryformatversion = 1` is accepted by `git log`/`git status`/`git config` (exit 0); version 2 or 3 is refused with exit 128. Also learned: `git config` itself skips the setup check — `git log` is the reliable probe.
- **Impact:** repos created by newer git (which default to version 0) and extension-bearing repos at version 1 both work; only 2+ is refused, matching real git. Integration test asserts real git behavior too. A non-numeric value (`repositoryformatversion = abc`) is also refused with `bad numeric config value 'abc' for 'core.repositoryformatversion'` (Corrupt, exit 128), matching real git's refusal on every config read.

### D-001 — Structured Io error variant instead of `Io(String)`

- **Date:** 2026-08-12
- **Step(s) affected:** 01 (code-standards.md example, module-registry.md)
- **Plan said:** `code-standards.md` error example shows `Io(String)` — a message string; module-registry listed `Io(String)`.
- **Decision:** `GitError::Io { path: String, op: String, source: io::Error }` — structured fields plus `From<io::Error>` fallback (path marked `<unknown>`) and an `IoContext<T>` trait adding `.context(path, op)` to `io::Result`.
- **Why:** progress-tracker step 01 mandates the `.context(path, op)` helper so every I/O error names its path and operation; a struct variant keeps the underlying `io::Error` available for `source()` chaining, which a `String` loses.
- **Impact:** All I/O errors must be produced via `.context(path, op)`; raw `?` on `io::Result` works but reports `<unknown>` — treat as a smell to fix at the call site.

### D-007 — `cat-file -p` pretty-prints trees, stays raw for blob/commit/tag

- **Date:** 2026-08-12
- **Step(s) affected:** 04 (commands/hash_object.rs `print_tree`)
- **Plan said:** step-04 architect decision: "cat-file -p stays raw bytes for all types (that's what real git prints)".
- **Decision:** `cat-file -p` pretty-prints trees in ls-tree format — `<6-digit octal mode> <type> <sha>\t<name>` per entry, `<type>` = tree/commit/blob for dir/gitlink/file modes, entries joined by `\n` with NO trailing newline — while blobs/commits/tags print raw content.
- **Why:** verified against real git 2.55: `git cat-file -p <tree>` emits the pretty ls-tree format, not raw bytes (raw tree format is `<mode> <name>\0<20-byte oid>`). The assumption behind the architect decision was wrong.
- **Impact:** behavior is byte-identical to real git; integration test `cat_file_p_pretty_prints_tree_like_git` locks it in.

### D-008 — Step 05 extras: `update-ref` command, `Fatal` error variant, reflog date handling

- **Date:** 2026-08-12
- **Step(s) affected:** 05 (refs.rs, commands/hash_object.rs, error.rs, main.rs)
- **Plan said:** step 05 is library-only (`refs.rs`); the GitError enum has four variants (`NotFound`, `Corrupt`, `Invalid`, `Io`, step 01); reflog writes `<old> <new> <name> <email> <ts> <tz>\t<message>`.
- **Decision:** three deviations, all confirmed by the developer during the step-05 architect session:
  1. **Shipped the `update-ref [-m <reason>] <ref> <new> [<old>]` plumbing command** in commands/hash_object.rs so the tracker's verification ("create a branch + commit via our code, then real git reads it") runs through the real CLI. No `-d`/delete in v1 — no step needs it yet.
  2. **Added `GitError::Fatal(String)`** (exit 128) — real git 2.55 exits 128 for every update-ref failure (bad name, CAS mismatch, create-only conflict, nonexistent object), but `Invalid` maps to 1. Fatal is the honest fit: user-input errors real git treats as fatal.
  3. **Reflog timestamp**: `SystemTime::now()` for ts; tz comes from `GIT_COMMITTER_DATE` env (`<ts> <tz>`) when set — verified real git 2.55 honors it for update-ref reflogs — else `+0000` (UTC). Local tz offset needs chrono or unsafe FFI (`GetTimeZoneInformation`), both banned by the dependency policy and `deny(unsafe_code)`.
- **Why:** message parity with real git was the step-05 goal and every probe showed exit 128; byte-for-byte reflog parity in tests needs a pinned date, which `GIT_COMMITTER_DATE` provides; without it, +0000 keeps ts correct (epoch seconds are absolute).
- **Impact:** update-ref failures print git's exact `update_ref failed for ref '<name>': ...` messages with exit 128; the commit command (step 11) can reuse `Refs::update` + `GIT_COMMITTER_DATE` handling as-is. If local-tz parity is ever required, a decision is needed on an allowed dependency (chrono) or a scoped unsafe block.

### D-009 — Ref name validation: git's full `check-ref-format` set, not just the tracker subset

- **Date:** 2026-08-12
- **Step(s) affected:** 05 (Refs::validate_name)
- **Plan said:** reject names containing `..`, starting with `.`, containing whitespace or `~^:?*[\\` or control chars.
- **Decision:** the tracker's set plus git's extras, all rejected with the same `refusing to update ref with bad name '<name>'` message: names not starting with `refs/` (except literal `HEAD`), the bare `@` ref, trailing `.`, `.lock` suffix, `@{` sequence, `//` runs, and path components starting with `.` (covers `.` and `..` components, not just leading-dot names).
- **Why:** probed real git: `git check-ref-format` accepts `refs/heads/a@` (single trailing `@` is legal — the bare `@` ref is what's banned) but `git update-ref refs/heads/a/b` (no `refs/` prefix) is refused with the bad-name fatal. Matching git exactly avoids two classes of divergence: names we'd accept that git can't read, and the `.lock` collision (git's own lock-file scheme).
- **Impact:** `validate_name` is the single gate before any path join, so traversal safety holds. Unit test `validate_rejects_git_bad_names` locks the full table; update-ref integration tests compare bad-name behavior against real git byte-for-byte.

### D-010 — Step 05 review fixes: input normalization, date/global-config parity

- **Date:** 2026-08-12
- **Step(s) affected:** 05 (refs.rs, commands/hash_object.rs, config.rs)
- **Plan said:** update-ref accepts shas as typed; `GIT_COMMITTER_DATE` garbage silently falls back to UTC; reflog identity comes from repo config only.
- **Decision:** five fixes after the step-05 review (all probed against real git 2.55 first):
  1. **Malformed new sha** → `fatal: <value>: not a valid SHA1` (exit 128), not the nonexistent-object message — CLI validates with a new `is_40_hex` helper before calling `Refs::update`.
  2. **Malformed old sha** → `fatal: <value>: not a valid old SHA1` (exit 128); old was previously unvalidated.
  3. **Uppercase shas are accepted and lowercased** on both input paths (`run_update_ref` and `Refs::update`), matching git's object-name normalization; ref/reflog files then hold lowercase, so byte-parity holds.
  4. **Garbage `GIT_COMMITTER_DATE`** → `fatal: invalid date format: garbage` (exit 128) and no reflog written; `now_with_tz()` now returns `Result`.
  5. **`-m ""` is refused** with a usage error (real git: usage, exit 129; ours: `Invalid` usage message, exit 1 — the exit code differs deliberately, see D-005 for the 129-vs-1 stance). Omitting `-m` entirely stays legal (empty reflog line, no trailing tab).
  6. **Reflog identity now reads the global config layer** (`~/.gitconfig`) in addition to repo config, via `Config::load_with(git_dir, global_config_path())` — the global path resolver in config.rs became `pub(crate)`. Previously identity from the global file failed where real git succeeds; tests masked it by injecting env identity.
- **Why:** all six were parity gaps caught by comparing against real git; two (garbage date, uppercase) could produce silently-divergent on-disk state.
- **Impact:** env-dependent unit tests are serialized behind a shared `Mutex` (parallel test threads share the process env — the garbage-date test poisoned concurrent reflog tests). Integration suite now asserts stderr+exit parity for malformed new/old, and byte-parity for uppercase-sha ref files.

### D-011 — Index extensions are skipped, not rejected (tracker locked text overruled)

- **Date:** 2026-08-12
- **Step(s) affected:** 06 (src/index.rs)
- **Plan said:** tracker 06 locked: "Version 3+ and extensions (TREE, REUC, link, sdir): reject version > 2 in v1 with a clear message (do not silently misparse)".
- **Decision:** version != 2 is rejected with a clear `Corrupt` message; extensions are **skipped by their length field** (4-byte signature + 4-byte length + data, loop until the final 20-byte checksum), exactly like git's own reader. Signatures are validated as alphabetic; overruns are `Corrupt`.
- **Why:** real git 2.55 writes the TREE (cache-tree) extension into a version-2 index on every `git commit` (and rewrites it on various commands) — a v2 file *with* an extension is the normal state of any real repo with history. Hard-rejecting it would make step 07's verification impossible (we couldn't read any index a repo was committed in). Skipping is not misparsing: the length field is part of the format, and the trailing checksum still fails if anything was misread.
- **Impact:** step 07+ read real indices including post-commit ones. Extensions are dropped on rewrite (we emit no extensions) — git treats their absence as a cache miss, verified by `byte_round_trip_keeps_git_status_clean` (status/diff clean after our rewrite of a committed index).

### D-012 — Index extended entries: 2-byte field sits BEFORE the name, preserved verbatim

- **Date:** 2026-08-12
- **Step(s) affected:** 06 (src/index.rs, review fix)
- **Plan said:** step-06 architect decision 3: entries with the extended flag (1<<14) are read with their 2-byte extended-flags field and re-emitted with a zero field. The first implementation read those 2 bytes after the name's NUL.
- **Decision:** the extended-flags field is placed **between the 62-byte fixed part and the name** (git's `ondisk_cache_entry_extended` struct — fixed part is 64 bytes when bit 14 is set). It is parsed there and **preserved verbatim** via a new `extended_flags: u16` field on `IndexEntry`; serialize re-emits it only when bit 14 is set.
- **Why:** probed against real git 2.55 with a byte-crafted v2 index containing an extended entry (correct layout): `git ls-files --stage` and `git status` read it fine, exit 0 — git accepts v2 files with extended entries even though it only *writes* them at v3+. The wrong placement would misparse any such file; zeroing the field would silently drop real bits (e.g. skip-worktree) on round-trip.
- **Impact:** `IndexEntry` gains one field (callers constructing entries must set `extended_flags: u16`); round-trip of extended entries is byte-exact. Also part of this review pass: `unwrap()` removed from non-test `read()` (step-01 standard) and parse errors now carry entry position + cause.

### D-013 — Step 07 scope cuts: ignore sources, unstaged renames, merge entries

- **Date:** 2026-08-12
- **Step(s) affected:** 07 (src/ignore.rs, src/worktree.rs, src/commands/add.rs, src/commands/status.rs)
- **Plan said:** tracker 07 lists `.gitignore` support with git's matching rules; status shows the porcelain short format.
- **Decision:** three scope cuts, all approved by the developer in the step-07 architect session:
  1. **Ignore sources: per-directory `.gitignore` files only.** `.git/info/exclude` and `core.excludesfile` are skipped in v1; the documented pattern semantics (negation, dir-only, anchoring, `**`, classes, deeper-wins, prune-ignored-dirs) are implemented and probed against real git's `check-ignore`.
  2. **Rename detection: HEAD↔index only, exact content match** (`R  old -> new`). Unstaged renames (index↔worktree) are not detected; staged-deleted+untracked-recreated files still print as `D  <path>` + `?? <path>`.
  3. **Merge-conflict entries (stage 1-3) are skipped** by status; `git add` of a conflicted path replaces all stages with the stage-0 entry (via `Index::stage` semantics).
- **Why:** all three were architect-approved time-boxes; the tracked verification footprint (byte-identical status across new/modified/deleted/staged-then-modified/untracked/ignored/symlink/subdir paths) passes without them, and each has a natural v2 owner (info/exclude + core.excludesfile, Y-column renames, conflict markers).
- **Impact:** repos using `.git/info/exclude` will show ignored files as untracked until step 14 (or whenever ignore sources widen). Note: `git add <explicit-ignored-file>` exits **1** with the bare message (no `fatal:` prefix, real-git-probed) while `git add .` skips ignored files silently — `Invalid` is the error bucket used for the exit-1 case (see main.rs's prefix split).

### D-014 — Step 08 scope: no `xdl_change_compact`, gitlinks skipped, plain pathspecs

- **Date:** 2026-08-13
- **Step(s) affected:** 08 (src/diff.rs, src/commands/diff.rs)
- **Plan said:** tracker 08 verification: "byte-identical output vs real git diff ... on: modified lines, insertions, deletions, hunks split across context, new/deleted files, binary files, files with trailing-newline differences".
- **Decision:** three scope decisions:
  1. **No `xdl_change_compact` port.** The engine emits a content-valid, edit-distance-optimal Myers script, but it does **not** replicate git's post-pass `xdl_change_compact` (which slides change groups through runs of identical lines) or its exact split tie-breaking. On fixtures with all-identical runs between changes, hunk *boundary positions* can differ from real git 2.55 while the changed-line *content* is identical. Approved by the developer on 2026-08-13: document rather than port.
  2. **Gitlink (mode 160000) entries are skipped** by `diff` (submodule diffs are out of v1 scope).
  3. **`-- <paths>` uses plain path-prefix matching** — no globs or pathspec magic; non-matching pathspecs silently produce no output (probed against git 2.55).
- **Why:** (1) the split/merge *semantics* match git exactly (`distance = next.i1 - (prev.i1 + prev.chg1) > 2*ctxlen` splits; funcname suffixes and sticky carry-over are replicated); only the placement of a change group inside an identical run differs (e.g. 13 identical lines with `y`@3, `z`@11: git 2.55 slides the deletes to the right edge → `@@ -1,5 +1,6 @@` + `@@ -7,7 +8,6 @@`; ours keep them at the left → `@@ -1,7 +1,6 @@` + `@@ -9,5 +8,6 @@`). (2) git's own sliding is version-unstable (compaction was rewritten across releases; `diff.indentHeuristic` changes it), so pinning it would freeze our output to one git version. (3) Byte-parity on distinct-line fixtures — the realistic majority — already holds, verified by tests/diff.rs against real git 2.55. (2)+(3) of the Decision are small v1 time-boxes like D-013's.
- **Impact:** integration tests (tests/diff.rs) use distinct-line fixtures for byte-parity; the unit test `hunks_merge_within_six_context_lines` locks our deterministic boundary placement for the identical-line case with a D-014 comment. The step-15 compat matrix byte-compare may hit these cases; revisit there if identical-line placement parity becomes a requirement. Related in this step: `emit_snake` was rewritten to content-checked walks (the old position-based emission produced content-invalid scripts — `Equal` ops pairing non-equal lines), the root cause of the previous boundary bugs.

### D-015 — Step 09 scope: oneline log, stat-not-patch show, linear graph, silent commit

- **Date:** 2026-08-13
- **Step(s) affected:** 09 (src/revwalk.rs, src/commands/commit.rs, log.rs, show.rs)
- **Plan said:** log "full header" not specified; show = "commit summary + `--stat`-style patch"; commit success output per real git (branch line + stat block); root reflog `commit (initial)` vs tracker's plain `commit:`; `--graph --all` with merge corners; `--allow-empty`.
- **Decision:** six scope choices, all confirmed by the developer on 2026-08-13:
  1. **Log is oneline-only** (`--oneline` always; full header format deferred — its locale-dependent date rendering can't be byte-pinned portably).
  2. **Show = header + stat, no patch.** Header (`commit <sha>`, `Author:`, `Date:` in the ident's tz via hand-rolled civil_from_days, indented message) plus the stat block (per-file `<name> | <n> <+->` + summary with singular/plural), byte-identical to `git show --stat`. The patch body is skipped in v1. Annotated tags show the commit header without the tag-object header or `Date:` line (deferred).
  3. **`--graph --all` renders linear-only columns** (`* ` per commit). Merge-corner glyphs (`| *`, `|/`) for side-branch topologies are deferred; tests compare only linear graphs (`--graph` alone) against real git.
  4. **Commit success output is silent** (like `git commit -q`); real git's `[branch sha] subject` + stat block is not printed.
  5. **Root reflog message is `commit (initial): <subject>`** (probed: that's what real git 2.55 writes), not the tracker's `commit:`.
  6. **`--allow-empty` is not supported** (empty-commit check always active). Date env accepts only `<unix-ts> <tz>` (git's internal form), not ISO/RFC forms.
- **Why:** each is a probed git-2.55 behavior where full parity needs a chunk of machinery (patch rendering, graph topology, tag-object headers, locale dates) that nothing downstream depends on; the sha-parity, reflog, log-oneline, and stat byte-parity the tracker requires are all locked by tests/log_commit.rs.
- **Impact:** steps 10-13 consume our commits/reflogs (fine — real git reads them, fsck clean); `show`/`log` format expansions are independent. `--graph --all` in step 11's merge verification (which draws topology) will need the glyph work before it can byte-match; revisit there.

### D-016 — Step 09 verification fixes: message trailing newline, empty-message ordering, stat skips unchanged blobs

- **Date:** 2026-08-13
- **Step(s) affected:** 09 (src/commands/commit.rs clean_message + empty-message check, src/commands/show.rs print_stat)
- **Plan said:** clean_message strips trailing blank lines; empty `-m ""` aborts with git's message before any other check; stat lists every file present in either tree.
- **Decision:** three fixes found by byte-comparing against real git 2.55 (integration tests first failed, then locked):
  1. **The commit-object message ends with a single trailing `\n`** (git stores `init` as `init\n` in the object — probed via cat-file hexdump and sha trials). clean_message now appends `\n` to non-empty results; empty results stay empty.
  2. **The empty-message abort fires AFTER the nothing-to-commit checks**, not before: on an empty index git reports `nothing to commit ...` (exit 1) and never mentions the empty message; `Aborting commit due to empty commit message.` appears only when there is something to commit.
  3. **Stat skips unchanged blobs** (same oid on both sides): the union-of-paths loop previously emitted `path | 0` lines git never prints.
- **Why:** all three were sha- or byte-divergences caught by the parity tests (root commit sha mismatch was the trailing `\n`; the empty-message test saw git exit 1 with empty stderr; show listed unchanged files as `| 0`).
- **Impact:** commit objects hash identically to real git (same tree/identity/dates → same sha, locked by `commit_shas_match_real_git`); `-m ""` and empty-index orderings match git on all four probed message variants.

### D-017 - Step 10 scope: checkout dirty gate, tag editor, tag sort, cat-file refs

- **Date:** 2026-08-14
- **Step(s) affected:** 10 (src/commands/checkout.rs, branch.rs, tag.rs, reset.rs, hash_object.rs, src/revwalk.rs, src/worktree.rs)
- **Plan said:** checkout refuses when a path is untracked in the worktree AND would be overwritten (`untracked working tree files would be overwritten`, exit 1); tracker locked "v1 requires clean index (index == HEAD tree) to switch branches; -f discards"; tag list sorting unspecified; cat-file took only raw object ids.
- **Decision:** five probed-against-git-2.55 adjustments and scope cuts:
  1. **The dirty gate is path-based, not index-clean-based.** Refusal happens only for paths whose worktree/index content would be overwritten by the switch: `error: Your local changes to the following files would be overwritten by checkout:` + `\t<path>` per file + `Please commit your changes or stash them before you switch branches.` + `Aborting`, exit 1. Same-tree switch while dirty is ALLOWED (probed). `-f` discards via `force_sync_worktree` (overwrites tracked files whose content differs from the target oid). **Untracked-file-overwrite protection is not in v1** (untracked file would be overwritten by a path the target tree has -> ours overwrites it silently; real git refuses with the untracked-file message).
  2. **`tag -a` without `-m` does not spawn an editor** in v1 - the message stays empty (git launches $EDITOR; out of scope). Tag-dates/identity use the committer chain + `commit::commit_dates`, giving byte-identical tag objects when env dates are set (integration test locks this with tag-name normalization).
  3. **`tag -l` sorts plain lexicographic** (probed live on git 2.55: `bar, foo, v0.9, v1-rc1, v1.0-rc1, v1.0.1, v1.02, v1.0alpha, ...` - git's tag builtin sorts by refname, NOT versioncmp; the earlier strverscmp porting plan was dropped; comment in print_tag_list documents this).
  4. **`cat-file <ref-name>` resolves refs** (raw object first, then refs/tags/, refs/heads/, refs/; never peels) so `git cat-file tag <tag>` interop checks pass; error for unknown names stays `Not a valid object name <n>`.
  5. **Detached checkout prints only the final line** `HEAD is now at <7-sha> <subject>` - the `You are in 'detached HEAD' state.` advice block is omitted in v1.
- **Why:** all five items were confirmed by live byte-comparisons against git 2.55 during the step-10 verification pass; each full-parity version needs machinery v1 does not ship (paths-only overwrite staging, editor spawning, versioncmp, advice blocks).
- **Impact:** tests/checkout_branch_tag_reset.rs locks the 5-parity surfaces; reset --hard had to write worktree files BEFORE rewriting the index (stale stat fields made real git report phantom ` M` on Windows - racy-mtime trap, probed; ordering is now locked in the tracker); `Refs::update` skips reflogs for `refs/tags/` (git never reflogs tags); `ObjectStore::write_object` tmp names include the object id (concurrent rename races, step-09 note).

### D-018 - Step 11: merge scope and probed wording fixes

- **Date:** 2026-08-14
- **Step(s) affected:** 11 (src/merge.rs, src/commands/merge.rs, commit.rs, reset.rs, error.rs, main.rs)
- **Plan said:** merge-base via two-pass BFS; criss-cross multi-base recursive strategy in v1; whole-file conflict markers; merge commit on success; dirty handling per git; `merge --abort` restores from ORIG_HEAD.
- **Decision:** nine confirmed-by-developer or byte-probed adjustments:
  1. **Every successful merge creates a merge commit -- no fast-forward and no `Already up to date.` handling** (user-confirmed on 2026-08-14; a plain `merge feature` where HEAD is an ancestor still writes a merge commit and warns nothing). The merged diffstat block git prints after the success line is not printed (success line `Merge made by the 'ort' strategy.` only). Fixes would land when rebase (step 12) is in via a straight-port check; until then this is the locking rule.
  2. **Criss-cross is deferred**: `merge_base` returns the first common ancestor; the recursive (merge-the-bases) strategy is a later step.
  3. **Whole-file conflict markers, no hunk merging** (user-confirmed): when all three blobs differ the ENTIRE file is a conflict with `<<<<<<< HEAD / ours / ======= / theirs / >>>>>>> <label>` (byte-exact when git's ort also emits a single whole-file hunk -- disjoint content). With shared context lines git's ort shrinks the hunk to the diverging region; our markers stay whole-file (locked deviation, documented in the tracker).
  4. **Dirty gate is strict index==HEAD refuse** -- stricter than git's per-path gate: `error: Your local changes to the following files would be overwritten by merge:` + `  <path>\n` per file + `Merge with strategy ort failed.` on stderr, exit 2. The wording probed is git 2.55's for a GENUINE diverged merge (ort engages); git's ff-path wording (`Please commit your changes or stash them before you merge.\nAborting`, tab indent) differs, and since v1 never fast-forwards we match the ort form including the two-space indent. New `GitError::Failure` variant carries the exit code 2 (printed bare, like Invalid). The gate fires before any state file is written (no ORIG_HEAD in the dirty case, matching git).
  5. **Conflict stdout lines** (probed): stdout carries `Auto-merging <path>`, `CONFLICT (content): Merge conflict in <path>`, and the final `Automatic merge failed; fix conflicts and then commit the result.` -- all stdout, exit 1 (sentinel `GitError::Invalid(empty)`); nothing on stderr.
  6. **`commit` during a merge**: empty-commit checks are SKIPPED (an unchanged-tree merge commit is allowed, matching git); message precedence `-m` > MERGE_MSG (comment lines `#` stripped then normal cleanup); reflog `commit (merge): <subject>`; state files removed on success, ORIG_HEAD kept. Unmerged-index refuse prints `U\t<path>` per UNIQUE path on stdout + git's error/hint block on stderr, exit 128. (The `fatal: ` prefix on empty sentinel errors is suppressed in main.rs for Invalid/Failure/Fatal(empty).)
  7. **`merge --abort`** = `reset --hard ORIG_HEAD` + delete MERGE_HEAD/MERGE_MSG; `fatal: There is no merge to abort (MERGE_HEAD missing).` exit 128. Its correctness depended on a root-cause fix to `reset --hard`: it now also DELETES worktree files that are in the current index (stage 0) but absent from the target tree (a merge's staged additions / staged new files) -- git does this; without it abort left `?? d.txt` untracked garbage (caught by the parity test).
  8. **Merge index layout**: untouched paths keep their stage-0 entries (add/add conflicts no longer drop them), merged paths replace theirs, conflict paths get stages 1/2/3 with zero stat, the whole index re-sorted by path -- byte-identical `ls-files -s` vs git.
  9. **MERGE_MSG** carries `Merge branch '<x>'` + blank + `# Conflicts:\n#\t<path>` per conflicted file (probed); ORIG_HEAD is written on EVERY merge before output (git parity), kept after both success (where git keeps it) and conflict.
- **Why:** item 1/2/3 are the developer's explicit v1 scope; 4-9 are byte-parity findings from twin-repo integration tests that failed first and were then locked (dirty wording/indent, exit codes, dedupe of `U\t` lines, index preservation, ORIG_HEAD-on-success, `fatal:` prefix suppression).
- **Impact:** rebase (12) consumes the merge commit/markers/state; the whole-file-marker deviation means conflicted files with shared context differ from real git's (documented in tests/merge.rs); the `Failure` variant and main.rs match arms are shared infrastructure for later commands.

### D-019 - Step 12: rebase scope and probed behavior

- **Date:** 2026-08-15
- **Step(s) affected:** 12 (src/commands/rebase.rs; refs.rs `set_head_sha`/`update_quiet`; merge.rs `apply_merged_files`/`print_merged_lines`; reset.rs `hard_sync`; commit.rs `strip_comment_lines`; show.rs `stat_summary`; checkout.rs detached fix)
- **Plan said:** fast-forward prelude (user-approved), "a commit whose tree == its parent's tree is dropped unless --keep-empty (v1: always drop)", first-parent range walk, own state format under `.git/rebase-merge/`, refuse real git's dir.
- **Decision:** every item probed byte-for-byte against git 2.55 (or read from rebase.c v2.55 source, fetched for the preludes):
  1. **There is NO fast-forward.** A non-empty range always replays with new shas (probed: HEAD-behind-upstream replays `Rebasing (1/1)`). `Current branch <b> is up to date.` (stdout, rc 0, no state) prints ONLY when `can_fast_forward` holds: the fork is the upstream tip AND the first-parent chain from HEAD down to it is merge-free (git's `can_fast_forward` + `is_linear_history` in rebase.c — a merge commit on the chain replays instead). An EMPTY range (HEAD's commits all already in upstream, e.g. feature merged into main) SILENTLY fast-forwards: branch ref -> upstream tip, worktree/index synced to it, only `Successfully rebased and updated refs/heads/<b>.` on stderr, rc 0 (probed; ORIG_HEAD and run reflogs still written).
  2. **The range is `rev-list --reverse --topo-order upstream..HEAD`**, NOT the first-parent chain (probed on a branch with a merged-in topic: the topic's commits ARE replayed). Replicated exactly: range = ancestors(HEAD) minus ancestors(fork) (single merge base), sorted committer-date descending, indegree (children-within-range counts) stack: seeds pushed in list order, a popped commit's parents pushed in parent order, LIFO pops, final reversal of the emission; merge commits gate their parents but are dropped (flattening, silently). Probe reproduced git's order on every fixture. Date ties break by descending sha (git keeps its walk insertion order; distinct dates make it moot).
  3. **Originally-empty commits ARE replayed and kept** — git 2.55 defaults to `rebase.drop-empty=false` (probed: an `--allow-empty` commit in the range survives with identical sha). The tracker's "always drop" is superseded. A pick whose merged tree equals the current HEAD tree (became empty / already-applied) is dropped SILENTLY; git's `warning: skipped previously applied commit <sha>` + `use --reapply-cherry-picks` / `advice.skippedCherryPicks` hints are not emitted in v1 (same end state, different stderr bytes).
  4. **Conflict stop** (rc 1): stdout = merge's `Auto-merging <p>` / `CONFLICT (...) Merge conflict in <p>` lines (shared `print_merged_lines`); stderr = `Rebasing (k/N)\r` progress + `error: could not apply <7sha>... <subject>` + the five verbatim hint lines (incl. the literal `<conflicted_files>` placeholder — git 2.55 does NOT fill it in) + `Could not apply <7sha>... # <subject>`; markers `<<<<<<< HEAD` / `>>>>>>> <7sha> (<subject>)` (probed: ours = onto tip, theirs = the pick); HEAD parked on the onto commit; branch ref untouched; state dir kept.
  5. **Progress bytes**: `Rebasing (k/N)\r` — carriage return, no newline (byte-verified captured stderr); success line ends with `\n`.
  6. **`--continue`**: unmerged-index refusal is `a.txt: needs merge\nYou must edit all merge conflicts and then\nmark them as resolved using git add\n` on STDOUT, rc 1 (different from commit's block — probed). Otherwise commits the staged index with the ORIGINAL author (name/email/date verbatim, from the state's author-script) + a fresh committer + the state message with `#` lines stripped; prints `[detached HEAD <7sha>] <subject>` + ` Author: <name> <email>` (only when author != committer) + the stat SUMMARY line only (git 2.55's `commit` prints no per-file lines — `stat_summary` extracted from show.rs); reflog `rebase (continue): <subject>`; then replays the rest (no `Rebasing` line for the committed pick itself).
  7. **`--abort`**: silent (rc 0, neither stream). Branch ref restored to orig-head with NO branch reflog (new `Refs::update_quiet` — probed: git adds none on abort); worktree+index hard-reset (shared `reset::hard_sync`); HEAD returned to the symref with reflog `rebase (abort): returning to refs/heads/<b>`; state dir removed; ORIG_HEAD kept. `--abort`/`--continue`/`--skip` with no state -> `fatal: no rebase in progress`, rc 128.
  8. **In-progress refusal** (plain `rebase` while a state dir exists): git's exact block, 341 bytes file-captured — including the wrapped `I wonder ... is the\ncase, please try`, `...have something\nvaluable there.` lines and the trailing blank line. Caution: on Windows git's `die()` text also wraps at the console width, so the test asserts our exact bytes and git's structure separately.
  9. **Detached HEAD is refused** with our own fatal (`rebase: detached HEAD is not supported in v1`) — git supports it; the user-approved refusal has no git wording to copy. Bad upstream -> `invalid upstream 'x'`, rc 128. Rebase onto unrelated histories replays with the empty tree as base (writes the empty-tree object via `tree_from_index(&[])`). State files (head-name/onto/orig-head/msgnum/end/git-rebase-todo/done/message/author-script) are OUR format: git's `--continue` on our state does NOT work (its sequencer re-applies the paused pick because our todo keeps it, and it writes patch/stopped-sha) — interop is one-directional by design (we refuse git's dir); bonus: real `git status` DOES read our dir and reports `You are currently rebasing branch 'x' on 'y'.` (verified in a test).
  10. **Reflogs** (probed, `%gs`-verified in tests): HEAD `rebase (start): checkout <upstream>` -> `rebase (pick): <subject>` per pick -> `rebase (finish): returning to refs/heads/<b>`; branch `rebase (finish): refs/heads/<b> onto <onto-full-sha>` at finish only (nothing on abort); ORIG_HEAD = pre-rebase HEAD. Unrelated to git-rs: `checkout <tag|sha>` now genuinely detaches (`Refs::set_head_sha` writes the raw sha into the HEAD file) — the previous `refs.update("HEAD", ...)` symref-resolution bug moved the BRANCH ref instead (pre-existing, found while building rebase).
- **Why:** every item above is a byte probe, a rebase.c source reading, or a user-approved scope cut (detached refusal). The git-on-our-state continue was never a requirement (plan: our own format + refuse git's).
- **Impact:** step 13 (stash) can reuse the replay/state machinery; pick-order and drop-empty rules are locked by tests/rebase.rs (10 tests, sha-parity + byte-parity + interop); the todo/done semantic difference means a git-rs rebase must be finished with git-rs.
