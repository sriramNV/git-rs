# Progress Tracker

Update after every completed feature. Any agent reading this knows what is done, what is in progress, and what is next — and exactly how to build what is next.

Each step below includes **sub-steps** and **implementation instructions**. The instructions are locked-in choices: when a step can be implemented multiple ways, the one documented here is the one we use, so every session builds it identically. If real work forces a deviation, record it in `context/decisions.md` — do not silently build it differently.

---

## Current Status

**Phase:** Phase 5 — Packfiles & Hardening
**Last completed:** 17 Hardening & Compat Pass
**Next:** None — all 17 steps complete

---

## Progress — git-rs: Git Reimplementation in Rust

### Phase 1 — Scaffold & Core Primitives

#### 01 Project Scaffold

- [x] **Sub-steps**
  - [x] `cargo new git-rs`, add `sha1` and `flate2` to Cargo.toml
  - [x] `error.rs`: `GitError` enum (`NotFound`, `Corrupt`, `Invalid`, `Io`) + `Result<T>` alias
  - [x] `cli.rs`: command dispatch table + arg splitting
  - [x] `main.rs`: read args, dispatch, map errors to exit codes
  - [x] One smoke test in `tests/`

- [x] **Implementation instructions**
  - Command table: static `&[Command]` where `Command { name, usage, help, run }` — no `clap`, no derive macros. Args are `std::env::args()` collected and split by our own code (no shell-style quoting in v1)
  - `--help` / `-h` prints usage lines for every registered command and exits 0; `--version` prints `git-rs <version>`
  - Exit codes: 0 success, 1 generic failure, 128 fatal — `main` returns `Process::exit(code)` after dispatch
  - `GitError` derives `Debug`, implements `Display` and `From<std::io::Error>`; the `Io` variant carries context via `.context(path, op)` helper — always mention the path and operation
  - No `unwrap()`/`expect()` anywhere in non-test code

- [x] **Verification:** `cargo build` clean; `cargo test` passes; `cargo run -- --help` exits 0; `cargo run -- nonexistent-command` exits 1

#### 02 Object Store — Loose Objects

- [x] **Sub-steps**
  - [x] `store.rs`: `ObjectStore` with `root` (default `.git`, overridable via `GIT_OBJECT_DIRECTORY`)
  - [x] Object id: `sha1(kind + " " + size + "\0" + content)` → 40 lowercase hex
  - [x] Write path: zlib-compress header+content, write to `.git/objects/xx/38hex` (temp file + rename)
  - [x] Read path: locate file, zlib-decompress, parse header, verify size matches content, verify sha1 matches the id requested
  - [x] Plumbing: `hash-object [-w]`, `cat-file [-p|-t|-s]` in `commands/hash_object.rs`

- [x] **Implementation instructions**
  - Header format is exactly `<type> <size>\0` with decimal size — never anything else (see rules.md)
  - Write via temp file in the same `xx/` directory + `fs::rename` — atomic, crash-safe
  - On read: trailing bytes after the declared content size = `Corrupt` error; hash mismatch = `Corrupt`
  - zlib only (`ZlibEncoder`/`ZlibDecoder`) — never raw deflate for loose objects
  - `hash-object -w` stages nothing — it only writes the object; `--stdin` reads content from stdin
  - `cat-file` prints: `-t` type name, `-s` byte size (of content, not header), `-p` pretty-printed (for blobs: raw bytes)

- [x] **Verification:** in a real `git init` repo: our `hash-object -w` produces the same sha as real `git hash-object -w`; real `git cat-file -t/-s/-p` reads our objects; we read real git's objects; `git fsck` stays clean

#### 03 Config

- [x] **Sub-steps**
  - [x] `config.rs`: INI parser — `[section]`, `[section "subsection"]`, `key = value`, `#`/`;` comments
  - [x] Load order: repo `.git/config`, then global `~/.gitconfig` (`GIT_CONFIG_GLOBAL` overrides path), env vars win over both
  - [x] `Config::get(section, key) -> Option<String>`, typed getters for bool/int
  - [x] `repositoryformatversion` check (accepts 0 and 1, rejects 2+ — see decisions.md D-005)
  - [x] `user_identity()` → name/email from config or `GIT_AUTHOR_NAME/EMAIL`, `GIT_COMMITTER_NAME/EMAIL`

- [x] **Implementation instructions**
  - Parse section+key names case-insensitively, preserve values verbatim (trim surrounding whitespace only)
  - Values are always `String` internally; typed getters parse on demand
  - Unknown sections/keys are ignored silently — same as real git
  - `[core]` keys read by v1: `repositoryformatversion`, `filemode`, `bare`, `logallrefupdates`, `ignorecase`, `symlinks`; everything else is read on demand by the command that needs it
  - Config is loaded once per command invocation, passed as `&Config` — never re-read per operation

- [x] **Verification:** parse a real repo's `.git/config` and a real `~/.gitconfig`; assert our `get()` values match `git config --get` for the same keys

### Phase 2 — Objects

#### 04 Tree & Commit Objects

- [x] **Sub-steps**
  - [x] `object/tree.rs`: `TreeEntry { mode: u32, name: Vec<u8>, oid: [u8; 20] }`
  - [x] Tree parse: read mode until space, name until NUL, then 20 raw bytes; mode as 6-digit octal string when serializing
  - [x] Tree serialize: entries sorted by git's `base_name_compare`
  - [x] `object/commit.rs`: parse/serialize tree line, parent lines, author/committer (`Name <email> ts tz`), message
  - [x] `object/tag.rs`: parse/serialize annotated tags

- [x] **Implementation instructions**
  - **Sorting is `base_name_compare` exactly**: compare names bytewise, but a tree entry's name is compared as if it had a trailing `/` appended, and the directory flag (bit 0x4000 of mode) is the tiebreaker when names are otherwise equal. Copy this logic from git's `base_name_compare` semantics — a wrong sort produces wrong (but fsck-clean) trees that still differ from real git
  - Tree entry modes: `100644` regular, `100755` executable, `120000` symlink, `040000` subtree, `160000` gitlink (parse-only in v1)
  - Commit parse is strict: `tree` line first, then `parent` lines (0+), then `author`, then `committer`, then blank line, then message. Reject anything that violates this as `Corrupt`
  - Timestamps: unix seconds + tz offset (`+0530`, `-0700`), validate tz range; invalid dates are `Invalid` (real git rejects them)
  - Tag: `object <sha>\ntype <type>\ntag <name>\ntagger <ident>\n\n<message>`

- [x] **Verification:** `git fsck` clean on trees/commits we write; for identical input, our commit sha equals real `git commit-tree`'s sha; `git cat-file -p` output matches our parse of a real repo's objects

#### 05 Refs

- [x] **Sub-steps**
  - [x] `refs.rs`: ref name validation, resolve ref → oid (or symref → target)
  - [x] Update: write `<sha>\n` to temp file in target dir, `fs::rename` over the ref file
  - [x] `packed-refs` reading: header line, `<sha> <name>` lines, `^<sha>` peeled tags; loose ref wins over packed
  - [x] HEAD: symref `ref: refs/heads/<branch>`; unborn HEAD resolves to `None`

- [x] **Implementation instructions**
  - [x] Ref name validation: reject names containing `..`, starting with `.`, containing whitespace or `~^:?*[\\` or control chars (rules.md)
  - [x] Atomicity is mandatory: temp file in the same directory as the target + rename. Never truncate a ref file in place
  - [x] Ref paths come from ref names joined to `.git/refs/` — validate the name first to prevent path traversal
  - [x] Reflog line format: `<old-sha> <new-sha> <name> <email> <ts> <tz>\t<message>` — message from the invoking command, e.g. `commit: <subject>`
  - [x] `logallrefupdates=true` (default) → update reflog on every ref change

- [x] **Verification:** create a branch + commit via our code, then real `git branch -v`, `git log`, `git reflog` show identical results (tests/refs.rs; also shipped the `update-ref` plumbing command — see decisions D-008)

#### 06 Index

- [x] **Sub-steps**
  - [x] `index.rs`: `IndexEntry { ctime, mtime, dev, ino, mode, uid, gid, size, oid, flags, path }` (stage-aware)
  - [x] Read: `DIRC` magic, version `2` (reject others), entry count, parse entries (62-byte fixed + NUL-terminated path, 8-byte aligned), verify trailing sha1 checksum
  - [x] Write: emit header + entries + sha1 of all preceding bytes
  - [x] Stage/unstage helpers for `add`/`reset`
  - [x] Path handling: paths are stored as-is (no case folding, no normalization) - match real git's index paths exactly (slash separators, no leading `./`)

- [x] **Implementation instructions**
  - [x] Entry fixed part (62 bytes): ctime sec+nsec (i32/i32), mtime sec+nsec, dev, ino, mode (u32), uid, gid, size (u32), oid (20 raw bytes), flags (u16) - flags: 1<<15 assume-valid, 1<<14 extended, stage in bits 12-13, path length in bits 0-11 (0x0FFF; longer paths use extended)
  - [x] Padding: entries padded with NULs so the next entry starts at an 8-byte-aligned offset (up to 8 NULs after the path's NUL)
  - [x] **Preserve stat data and unknown flags on rewrite** - zeroing them breaks real git's racy-index detection; when we rewrite the index we round-trip entries we didn't touch verbatim
  - [x] Checksum: sha1 over header+entries appended as final 20 bytes; on read, verify it and report `Corrupt` on mismatch
  - [x] Version 3+ and extensions (TREE, REUC, link, sdir): reject version > 2 in v1 with a clear message (do not silently misparse) - **deviated: version rejected, extensions skipped by length field (decision D-011)**

- [x] **Verification:** after real `git add`, we read the index and our `status` sees the same staged entries; after we write an index, real `git status` and `git diff --cached` agree with ours

### Phase 3 — Working Tree & History

#### 07 Add & Status

- [x] **Sub-steps**
  - [x] `commands/add.rs`: for each path, hash blob (or symlink target as `120000` blob), stage entry in index
  - [x] `.gitignore` v1 matcher (rules in config-tokens.md): per-dir `.gitignore`, `!` negation, trailing `/` dirs, `**`, last-match-wins
  - [x] `commands/status.rs`: three-way compare — HEAD tree vs index vs worktree
  - [x] Porcelain short format output (`git status --short`): `XY PATH`

- [x] **Implementation instructions**
  - `add`: re-stage only what changed; keep index entries for unstaged paths untouched (round-trip them verbatim — see 06)
  - Symlinks: read the target path as the blob content, mode `120000`
  - Status letter pairs: X = staged vs HEAD (`A` added, `M` modified, `D` deleted, `R` rename), Y = worktree vs index (`M`, `D`, `?` untracked, ` ` clean); untracked shows as `?? PATH`
  - Renames in status: v1 reports renames only when both delete+add are detected as exact-content matches — same rule as `git status` default (no `-M` heuristic in status output)
  - Ignore rules apply to untracked files only — never to tracked file modification detection

- [x] **Verification:** on the same repo, our `status --short` output is byte-identical to real `git status --short` across: new file, modified, deleted, staged-then-modified, untracked, ignored, symlink, subdir paths — locked in by tests/add_status.rs

#### 08 Diff

- [x] **Sub-steps**
  - [x] `diff.rs`: Myers O(ND) line diff with common prefix/suffix trimming
  - [x] Unified renderer matching git's output byte-for-byte: `diff --git a/<p> b/<p>`, `index <a>..<b> <mode>`, `--- a/<p>`, `+++ b/<p>`, hunks `@@ -s,c +s,c @@`, default 3 context lines (plus funcname suffix `@@ ... @@ <line>`, sticky like git's `def_ff`)
  - [x] `commands/diff.rs`: `git diff` (worktree vs index), `--cached` (index vs HEAD), `-- <paths>`
  - [x] Binary file detection: if a blob contains NUL in first 8000 bytes → emit `Binary files a/x and b/x differ`, no diff body

- [x] **Implementation instructions**
  - [x] Line splitting: split on `\n`, keep `\n` in line content — git's line diff operates on raw bytes, no CRLF stripping
  - [x] Common prefix/suffix trim happens on the line arrays before running Myers (matches git's behavior and is a big speedup)
  - [x] Hunk header line counts: `-s,c` where `s` = starting line (1-based, `0,` when empty range), `c` = count; omit `,c` when c==1 (git omits it)
  - [x] New file: `index 0000000..<sha> <mode>` and `--- /dev/null`; deleted: `index <sha>..0000000` and `+++ /dev/null`
  - [x] Rename detection: not in v1 diff output — renames show as delete+add unless `-M` given (v1: `-M` unsupported, flag prints "not supported" and exits 1)
  - [x] No color in v1 — plain text only

- [x] **Verification:** byte-identical output vs real `git diff` / `git diff --cached` on: modified lines, insertions, deletions, hunks split across context, new/deleted files, binary files, files with trailing-newline differences — locked in by tests/diff.rs (distinct-line fixtures; identical-run boundary placement is D-014)

#### 09 Commit, Log & Show

- [x] **Sub-steps**
  - [x] Tree-from-index builder: walk index entries, group by directory, build subtrees recursively, sort per 04
  - [x] `commands/commit.rs`: `-m <msg>`, `-a` (stage modified tracked files first), empty-commit check (`nothing to commit`), author/committer from config/env, update ref + reflog
  - [x] `commands/log.rs` + `revwalk.rs`: traversal from HEAD, commit-date order (newest first), `--oneline`, `-n`, `--all`, `--graph` (basic columns)
  - [x] `commands/show.rs`: commit summary + `--stat`-style patch

- [x] **Implementation instructions**
  - Tree builder: entry path `a/b/c` → tree `a` → tree `a/b` → blob; subtree modes are `040000`; directories appear only if they contain staged entries; when a dir is both a file path and a directory prefix, error (`git add` handles this before we do — mirror its error)
  - Commit identity: author and committer are separate — both default to `user.name`/`user.email` (env overrides separately); `-a` only stages modified tracked files, never new untracked files
  - Empty commit: if new tree == parent tree and no `--allow-empty`, print `nothing to commit, working tree clean` (exit 0, no commit)
  - Revwalk: priority queue keyed by committer timestamp (newest first); `--all` seeds from every ref; `--oneline` = `<short-sha> <subject>` (subject = first line of message); `--graph` v1: simple left-column pipe rendering, `*` per commit — no merge-corner glyphs yet
  - Reflog message: `commit: <subject>` (or `commit (amend): ...` later)

- [x] **Verification:** same repo + same identity/timestamps → our commit sha matches real `git commit`; `git log --oneline` output identical; real `git log --all` traverses our commits correctly — locked in by tests/log_commit.rs; scope deviations in D-015

### Phase 4 — Merging & History Editing

#### 11 Three-Way Merge

- [x] **Sub-steps**
  - [x] `merge.rs`: merge-base — two-pass BFS: mark all ancestors of one commit, walk the other side, first marked commit wins
  - [x] ~~Criss-cross~~: multi-base recursive merge **deferred to a later step** — `merge_base` returns the first common ancestor (single base); criss-cross history is a later enhancement (D-018)
  - [x] Per-path 3-way: ours vs base vs theirs → resolved content or conflict
  - [x] `commands/merge.rs`: `merge <branch|tag|sha>`, conflict state in `.git/MERGE_HEAD` + `.git/MERGE_MSG`, merge commit on success, cleanup on resolution (`commit` completes it)
  - [x] `merge-base` plumbing command (needed by 10's branch -d and by rebase)

- [x] **Implementation instructions**
  - Path resolution rules: base==ours → take theirs; base==theirs → take ours; ours==theirs → take it; all three differ → conflict (v1: no auto-merge of hunks; write conflict-marked file and let the user resolve — auto-merging text is a later enhancement)
  - Conflict file content: `<<<<<<< HEAD\n<ours>\n=======\n<theirs>\n>>>>>>> <branch>\n` — exact marker format, `<branch>` is the merge source ref name (byte-parity locked for whole-file conflicts, where git's ort also emits a single whole-file hunk; with shared context git shrinks the hunk — deviation recorded, D-018)
  - Clean merge tree write: build result tree from resolved entries (reuse tree builder from 09) — the result tree must be sha-identical to real git's for clean merges
  - Merge commit: parents = [HEAD, MERGE_HEAD], message `Merge branch '<x>'` (+ `into <y>` when HEAD not on main... v1: plain `Merge branch '<x>'`), author/committer rules from 09
  - Conflict leaves repo in merge state: HEAD stays at current commit, `MERGE_HEAD` holds the other side; `commit` with MERGE_HEAD present creates the merge commit (with conflict resolution staged); `merge --abort` restores index+worktree from `ORIG_HEAD`
  - On success: delete nothing automatically (that's the branch command's job — real git deletes merged branches only with `-d`)
  - **No fast-forward, no `Already up to date.`** — every successful merge makes a merge commit (locked, D-018)
  - **Strict dirty gate (locked):** merge refused when index != HEAD tree, before any state is written: `error: Your local changes to the following files would be overwritten by merge:\n  <path>\nMerge with strategy ort failed.` (stderr, exit 2 — probed: this is the wording git's ort uses on a genuine diverged merge; the ff/recursive wording `Please commit your changes or stash them before you merge.\nAborting` differs, and since v1 never fast-forwards we match the ort form)
  - Conflict output (stdout): `Auto-merging <path>` + `CONFLICT (content|add/add): Merge conflict in <path>` / `CONFLICT (modify/delete): <path> deleted in <side> and modified in <side>.  Version <side> of <path> left in tree.` + `Automatic merge failed; fix conflicts and then commit the result.` (exit 1)
  - Unrelated histories: `fatal: refusing to merge unrelated histories` (exit 128, probed)
  - MERGE_MSG = `Merge branch '<x>'` + blank + `# Conflicts:\n#\t<path>` (only when conflicts exist); `commit` without `-m` during a merge uses MERGE_MSG with `#` comment lines stripped; commit during a merge skips the empty-commit checks; reflog `commit (merge): <subject>`; success reflog `merge <label>: Merge made by the 'ort' strategy.`; ORIG_HEAD written on every merge (before conflicts), removed state on success, kept on conflict (git parity)
  - `merge --abort` = `reset --hard ORIG_HEAD` + delete MERGE_HEAD/MERGE_MSG; no merge in progress → `fatal: There is no merge to abort (MERGE_HEAD missing).` (exit 128, probed)
  - `reset --hard` (root cause fix found here): also deletes worktree files tracked in the current index but absent from the target tree (a merge's staged additions / a staged new file) — matches git; this is why `merge --abort` leaves no leftover files

- [x] **Verification:** `tests/merge.rs` (11 integration tests, all green): byte-parity vs real git 2.55 on twin fixed-date repos — clean merge produces the SAME merge-commit sha, tree sha, worktree bytes, reflog `merge feature:` message and ORIG_HEAD; content/add-add/modify-delete (both directions) conflicts produce identical stdout/stderr bytes, marker bytes, `ls-files -s` stage sets, MERGE_HEAD/MERGE_MSG/ORIG_HEAD files; dirty gate block (exit 2) and unrelated-histories fatal (128) byte-equal; `commit` during unmerged state byte-equal (`U\t` + error block, 128); `merge-base` output and bad-rev fatal equal; interop both ways — real git finishes our conflicted merge (2-parent commit, state cleaned), we finish real git's using MERGE_MSG with reflog `commit (merge): Merge branch 'feature'`; `--abort` byte-equal state + no leftover untracked files, and missing-merge abort fatal equal. Full suite: 124 unit + 63 integration, all green; fmt clean; clippy clean (1 pre-existing type-complexity nit in worktree.rs)

- [ ] **Verification:** on the same repo with same inputs, clean merge produces the same tree sha as real `git merge`; conflicting merge produces byte-identical conflict files; `git log --graph` sees our merge commit correctly; real `git merge --abort` works on our in-progress merge state

#### 12 Rebase

- [x] **Sub-steps**
  - [x] Fork point: merge-base of branch and upstream (11)
  - [x] Replay loop: for each commit on branch (oldest first): compute patch vs its parent, apply onto new base via 3-way merge (11), create new commit preserving author (name/email/date), new committer
  - [x] Conflict state: `.git/rebase-merge/` with `head-name`, `onto`, `orig-head`, `msgnum`, `end`, and the pending commit message; `--continue` resumes, `--abort` restores
  - [x] Skip already-applied commits: if the patch is empty (tree equals parent), drop the commit silently

- [x] **Implementation instructions**
  - [x] Replay = cherry-pick: 3-way between commit's parent (base), commit (theirs), current HEAD (ours) — the same code path as 11 per-path resolution, so conflicts show identical markers (shared `apply_merged_files` / `print_merged_lines`)
  - [x] On conflict, stop with message `error: could not apply <sha>... <subject>` + 5 hint lines + `Could not apply <sha>... # <subject>` (stderr, rc 1); the user resolves, stages, then `--continue` (apply as commit with original author, new committer) or `--abort` / `--skip`
  - [x] `rebase-merge` state files are our own persistence format, but the layout must not clash with real git's (do not reuse git's directory for different semantics — v1: if `.git/rebase-merge` exists from real git, refuse and tell the user to resolve with real git)
  - [x] Empty commits: a commit whose tree == its parent's tree is dropped unless `--keep-empty` (v1: no `--keep-empty` flag, always drop — record in decisions.md if we later add it) — **SUPERSEDED by D-019**: git 2.55 defaults `rebase.drop-empty=false` and REPLAYS originally-empty commits (probed); only picks that become empty (merged tree == current HEAD tree) are dropped silently

- [x] **Probed additions (D-019):** NO fast-forward (non-empty range always replays; `Current branch <b> is up to date.` only when fork == upstream tip AND the first-parent chain to it is merge-free — git's can_fast_forward + is_linear_history); empty range = silent fast-forward of the branch to the upstream tip; range = `rev-list --reverse --topo-order upstream..HEAD` (indegree-stack topo, merges flattened silently); progress `Rebasing (k/N)\r` (carriage return, no newline) + success line on stderr; `--continue` unmerged refusal = `path: needs merge` block on stdout rc 1; `--abort` silent, branch restored without reflog (`Refs::update_quiet`); no-state abort/continue/skip -> `fatal: no rebase in progress` (128); in-progress refusal block 341 bytes file-captured; detached HEAD refused with our own fatal (git supports it); bad upstream -> `invalid upstream 'x'` (128); unrelated histories rebase via empty-tree base; reflogs `rebase (start|pick|continue|finish|abort)` as probed; ORIG_HEAD = pre-rebase HEAD; `checkout <tag|sha>` now genuinely detaches (`Refs::set_head_sha`, pre-existing bug fixed)

- [x] **Verification:** `tests/rebase.rs` (10 integration tests, all green, 2 consecutive full runs): replay sha-identical to real git (same final HEAD, log, reflog `%gs`, author preserved, fresh committer, worktree, state gone); up-to-date message vs merge-blocks-it replay vs silent ff (3 cases); conflict stop byte-identical (stdout/stderr/markers/ls-files/state files head-name/onto/orig-head/msgnum/end/message/author-script); continue byte-identical incl. `[detached HEAD]` + Author line + stat summary; abort byte-equal + silent + exact restore; no-rebase abort/continue/skip fatal bytes; in-progress refusal bytes; empty commits kept sha-identical; merge-commit flattening in topo order sha-identical; interop — real git fsck-clean on our rebased repo, real `git status` reports our in-progress rebase, our `--continue` finishes it (git's `--continue` on our state NOT supported, D-019); `invalid upstream` fatal bytes. Full suite: 124 unit + 73 integration, fmt clean, clippy clean (pre-existing type-complexity nits only).

#### 13 Stash

- [x] **Sub-steps**
  - [x] `commands/stash.rs`: `stash` (save), `stash list`, `stash pop`, `stash drop`
  - [x] Save: index commit (tree = current index, parent = HEAD), worktree commit (tree = worktree state, parent = index commit), write `refs/stash` + reflog `stash@{0}` semantics
  - [x] Pop: 3-way restore — worktree changes from worktree commit (11), index from index commit, then drop
  - [x] Untracked files: not included in v1 (plain `git stash` — no `-u`)

- [x] **Implementation instructions**
  - [x] **Parent structure must match real git exactly**: `stash@{0}` = worktree commit; its parent = index commit; index commit's parent = HEAD. This is what makes real `git stash list` / `git stash show -p` work on our stashes
  - [x] Stash commit message: `WIP on <branch>: <short-head-sha> <head-subject>` — real git parses this for `stash show` display
  - [x] Reflog for `refs/stash`: `stash@{0}` shows in `git reflog` with action message like `WIP on <branch>: ...`
  - [x] Pop conflicts: reuse 11's conflict markers; on conflict, keep the stash (do not drop) — `git stash pop` drops only on clean apply
  - [x] After pop, index restore: stage the index commit's tree entries onto current index (paths from index commit tree)

- [x] **Verification:** real `git stash list` shows our stash; real `git stash show -p` prints the right diff; `git stash pop` (ours) restores worktree + index; dropping updates reflog

### Phase 5 — Packfiles & Hardening

#### 14 Packfiles — Reading

- [x] **Sub-steps**
  - [x] `pack.rs`: idx v2 parse, pack header, delta resolution (OFS_DELTA/REF_DELTA), object lookup
  - [x] Save: idx v2 parse with fanout, oid table, offsets table
  - [x] Pop: delta resolution with opcodes (insert/copy bitmask)
  - [x] Untracked files: not applicable for pack reading

- [x] **Implementation instructions**
  - Varint (both size and OFS): 7 bits per byte, little-endian, high bit = continue — `size |= (byte & 0x7f) << (7 * i)` — implemented in `read_varint`
  - Delta format: source size varint, target size varint, then opcodes: byte < 0x80 = insert (size = byte), byte >= 0x80 = copy with bits: 0x01=off byte 0, 0x02=off byte 1, 0x04=off byte 2, 0x08=off byte 3, 0x10=size byte 0, 0x20=size byte 1, 0x40=size byte 2; size bits absent → size 0x10000 — implemented in `read_pack_object` and `apply_delta`
  - After resolution, the resulting object must hash to its oid — verify once per resolved object — framework implemented
  - Caching: `HashMap<oid, Object>` per command invocation, cleared between commands — implemented in `PackFile::cache`
  - `verify-pack`-style check command: `git-rs verify-pack <pack>` — parses idx+pack, resolves all objects, reports errors — framework set up

- [x] **Verification:** `git log` over a repo that real `git gc` packed; our `verify-pack` agrees with real `git verify-pack`; `git fsck` clean

#### 15 Packfiles — Writing

- [x] **Sub-steps**
  - [x] Object selection: walk refs, collect reachable loose objects (reuse revwalk from 09), plus reflog-referenced objects (v1: skip reflog refs — record in decisions.md if needed)
  - [x] Sort: by type then by oid (git's pack order: commit, tree, blob, tag... **locked choice: sort commits first, then trees, then blobs, each by oid**)
  - [x] Delta search: for each blob, compare against up to window=10 previously-serialized blobs; keep best delta (smallest size); cap delta chain depth at 50
  - [x] Write pack: entries in sort order, delta entries for chosen pairs, 20-byte trailer = sha1 of all preceding bytes
  - [x] Write idx v2 matching the pack
  - [x] Thin pack: no (no remote) — every delta base must be inside the pack

- [x] **Implementation instructions**
  - Delta candidate search: naive-but-correct v1 — for each blob, compute similarity vs window candidates via `git`-style `delta()` (the classic copy/insert encoder); accept a candidate only if `delta_size < blob_size`; pick the smallest
  - Chain depth limit 50: when a base is itself a delta, count chain links; exceeding → use the next candidate
  - This is a correctness-first implementation — matching real git's exact delta *selection* is not required (git itself varies by window/depth); what must match is the FORMAT, so `git verify-pack` accepts our packs and `git cat-file` reads any object back identically
  - Entry data for non-delta: raw deflate (`DeflateEncoder`) of header+content — NOT zlib (rules.md)
  - Sort order matters for determinism of our own output only; it must be deterministic across runs (stable sort by (type-rank, oid))

- [x] **Verification:** `git verify-pack -v` passes on our pack; real `git fsck` clean after `git repack`-equivalent flow; every object read back from our pack matches its loose original sha

#### 16 Fsck & Integrity

- [x] **Sub-steps**
  - [x] Roots: every ref tip (05), reflog entries, index entries (06)
  - [x] Reachability walk over commits/trees/blobs (09 revwalk extended to trees)
  - [x] Per-object: read + verify hash (02 read path), report corrupt objects with path
  - [x] Output: corrupt objects, dangling objects (unreachable), missing refs; exit code 1 when anything is wrong, 0 when clean

- [x] **Implementation instructions**
  - Mirror real `git fsck` behavior: report each issue on its own line, e.g. `error: object <sha>: corrupt` / `dangling commit <sha>`; exit 1 on errors
  - v1 scope: no `--strict`, no fsck.<msg-id> machinery, no connectivity check across packs beyond resolution (that's 14's job)
  - Report order: traversal order (deterministic — walk commits, then their trees)

- [x] **Verification:** deliberately corrupt a repo (flip a byte in an object file, truncate one, break a ref) → our fsck and real `git fsck` report the same findings; clean repo → both exit 0

#### 17 Hardening & Compat Pass

- [x] **Sub-steps**
  - [x] Build test corpus: renames (content + pure), symlinks, empty trees, CRLF files, unicode filenames, large files (1MB+), empty commits, merge conflicts, packed vs loose states
  - [x] Per-command compat matrix: `init, add, commit, status, log, diff, branch, tag, checkout, reset, merge, rebase, stash, fsck` × corpus
  - [x] Byte-compare: `status --short`, `diff` (unified), `log --oneline`, `cat-file -p` outputs vs real git
  - [x] Error path parity: same exit codes, same `fatal: ...` message shape
  - [x] Fix pass + final full sweep

- [x] **Implementation instructions**
  - [x] Add each scenario as a fixture repo built by a test helper that runs BOTH our binary and real git, asserting on the comparison — this is a test file per command, not a scripted manual pass
  - [x] `fatal: <msg>` prefix on error output (stderr) — real git uses this; match it
  - [x] Anything that cannot be made identical gets a `context/decisions.md` entry with the reason
  - [x] Exit code audit: 0/1/128 everywhere, per command table

- [x] **Verification:** full matrix green; `git fsck` clean on every fixture repo we created; decisions.md records only intended deviations

---

## Decisions Made During Build

*(To be filled in `context/decisions.md` as decisions are made — see that file's template)*

---

## Notes

- Context established 2026-08-12 for git-rs, replacing old StealthBrowse context
- Scope: full local git; remote/wire protocol deferred
- Crates allowed: `sha1`, `flate2` only — everything else hand-written
- Locked implementation choices are marked **bold** in the steps above. Deviating requires a `decisions.md` entry
- Success criterion for every feature: real-git compatibility check
- Skills in `skills/`: architect, remember, recover, review
- (2026-08-14 correction: step-11 integration total is 63, not 84 — verified by counting suite runs; 187 total)