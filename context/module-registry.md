# Module Registry

Living document. Updated after every module is built. Read before building — match existing patterns before inventing new ones.

---

## Baseline — Established 2026-08-12

Built: `GitError` (error.rs), `CliDispatcher` (cli.rs), `ObjectStore` + `Kind` (store.rs), plumbing commands `hash-object`/`cat-file` (commands/hash_object.rs), `Config` (config.rs), `Object`/`Tree`/`Commit`/`Ident`/`Tag` (object/), `Refs` (refs.rs), plumbing `update-ref` (commands/hash_object.rs), `Index` (index.rs), `IgnoreMatcher` (ignore.rs), `Worktree` helpers (worktree.rs), `add`/`status` (commands/add.rs, commands/status.rs), `DiffEngine` (diff.rs), `RevWalk` (revwalk.rs), `commit`/`log`/`show` (commands/commit.rs, log.rs, show.rs), `branch`/`tag` (commands/branch.rs, tag.rs), `checkout` (commands/checkout.rs), `reset` (commands/reset.rs). More modules will fill in as features land.

---

## Core Modules

### ObjectStore
File: `src/store.rs`

| Property | Value |
|----------|-------|
| Purpose | Loose object read/write, hashing, storage layout |
| Format | zlib(header + content), `.git/objects/xx/38hex` |
| Depends on | `sha1`, `flate2` (ZlibEncoder/Decoder) |
| Key methods | `write_blob(&[u8])`, `write_object(kind, &[u8])`, `read_object(&str)`, `object_path(&str)`, `hash(kind, content)` |
| Kind enum | `Blob`/`Tree`/`Commit`/`Tag` with `as_str()`/`parse()`; `read_object` returns `(Kind, Vec<u8>)` |
| Discovery | `GIT_OBJECT_DIRECTORY` → `GIT_DIR`+`objects` → `.git/objects` (cwd only, see decisions D-002) |
| Integrity | size check + sha1 re-hash on every read; violations → `Corrupt` |

### PlumbingCommands (hash-object / cat-file)
File: `src/commands/hash_object.rs`

| Property | Value |
|----------|-------|
| Purpose | Direct object-store access commands |
| `hash-object` | `[-w] [--stdin] <file>` — compute blob id, `-w` writes |
| `cat-file` | `(-t | -s | -p) <object>` — type/size/content; missing object → `NotFound` (exit 128); `-p` pretty-prints trees in ls-tree format (decisions D-007); argument may be a ref name (raw object first, then `refs/tags/`, `refs/heads/`, `refs/` — never peels; step-10 addition) |
| `update-ref` | `[-m <reason>] <ref> <new> [<old>]` — atomic create/update with optional CAS; failure messages byte-match git 2.55, exit 128 via `Fatal` (decisions D-008, D-009) |
| Error shape | `fatal: Not a valid object name <id>` (see decisions D-003) |

### GitError
File: `src/error.rs`

| Property | Value |
|----------|-------|
| Purpose | Single error enum: `NotFound`, `Corrupt`, `Invalid`, `Fatal`, `Io` (path+op+source) |
| Key ids | Associated `Result<T>` alias exported from `error.rs` |
| Patterns | `From<io::Error>` fallback (marks path `<unknown>`); prefer `io_result.context(path, op)` via `IoContext` trait so every I/O error names path + operation; `Fatal` → exit 128 (real git treats ref-update failures as fatal — decisions D-008) |

### CliDispatcher
File: `src/cli.rs`

| Property | Value |
|----------|-------|
| Purpose | Hand-written CLI: static command table + raw arg split (no clap) |
| Table | `static COMMANDS: &[Command]` — `{ name, usage, help, run: fn(&[String]) -> Result<()> }`, appended per feature |
| Flags | `--help`/`-h`, `--version`/`-V` handled in `dispatch()`, exit 0 |
| Errors | Unknown command → `Invalid` → exit 1 with `fatal: <msg>` on stderr |

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
| Update | temp file + rename + fsync, never in-place |
| Key methods | `resolve(HEAD)`, `update(name, sha, msg)`, `read_packed_refs()`, `validate_name()`, `ZERO_SHA` |
| Behavior | loose ref wins over packed; symrefs followed (10-hop loop guard); unborn → `None`; broken ref file → `Corrupt`; reflog appended when `core.logallrefupdates` unset/true with `GIT_COMMITTER_DATE` (or UTC `+0000`) ts/tz; `update("HEAD", ...)` writes the dereferenced branch file but logs to `logs/HEAD` — matches git's update-ref (decisions D-008, D-009) |

### Index
File: `src/index.rs`

| Property | Value |
|----------|-------|
| Purpose | Stage-aware index v2 read/write with checksum and stage/unstage helpers |
| Format | `DIRC` + version 2 (others rejected, `Corrupt`) + 62-byte fixed part + NUL-terminated path (raw bytes, never UTF-8) + NUL padding to 8-byte alignment + optional extensions (skipped by sig+len, decision D-011) + trailing sha1 of all preceding bytes |
| Key methods | `read(path)`, `write(path)` (atomic temp+rename, sorted by path bytes then stage), `entries()`/`entries_mut()`, `stage(entry)` (insert/replace per stage), `unstage(path)` (all stages), `IndexEntry::stage()` |
| Behavior | namelen bits recomputed from path length at write, all other flag bits round-trip verbatim (stat data preserved - real `git status` stays clean after our rewrite); extended entries (flag bit 14) keep their 2-byte field BEFORE the name and preserve it verbatim (decisions D-012); unknown extensions and checksum violations  `Corrupt`; stage bits 12-13 (0 normal, 1-3 merge slots) verified against real `git update-index --index-info` |

### IgnoreMatcher
File: `src/ignore.rs`

| Property | Value |
|----------|-------|
| Purpose | `.gitignore` matching for add/status untracked filtering (v1: per-directory `.gitignore` only — no info/exclude, no core.excludesfile, decision D-013) |
| Key methods | `load(root, git_dir)`, `is_ignored(path, is_dir)` |
| Behavior | last-match-wins across the whole rule set; deeper `.gitignore` files override shallower ones (rules carry their dir; `applies_to` prefix-gates); `!` negation, trailing `/` dir-only, leading `/` anchoring (any `/` anchors to the rule's dir), `*`/`?` never cross `/` except a TRAILING `*` (probed: `dir/*` ignores `dir/x/y.txt`), `**` only as full segments (`a/**/z` matches `a/z`), `[...]` classes with ranges and `!`/`^` negation; trailing spaces stripped, leading spaces kept; walk prunes ignored directories ("cannot re-include a file if a parent directory is excluded"); trailing `/` tolerated on queried paths |

### Worktree
File: `src/worktree.rs`

| Property | Value |
|----------|-------|
| Purpose | Shared add/status helpers: stat fields, blob hashing, repo-relative walking |
| Key methods | `stat_file(path)` (Windows: dev/ino/uid/gid = 0, real fs times — git re-hashes on mismatch, output identical), `blob_content(path)` (symlink → target bytes), `hash_entry(store, path, write)`, `repo_root(git_dir)`, `abs_git_dir(git_dir)`, `index_path(git_dir)` (`GIT_INDEX_FILE` honored), `walk_worktree(root, git_dir, matcher)`, `parse_oid(str)`, `sync_worktree(store, root, old_tree, new_tree)` (write changed/remove gone/leave untracked/prune empty dirs), `force_sync_worktree(store, root, old_tree, new_tree)` (same but overwrites tracked files whose content differs from the target oid — discards local edits; `checkout -f`, `reset --hard`), `tree_entries(store, tree)` (flatten tree to leaf `(path, mode, oid)`) |
| Behavior | walk skips `git_dir`, prunes ignored dirs, reports embedded-repo dirs (contain `.git`) as leaves; `WalkItem { path: Vec<u8>, is_dir }` |

### Config
File: `src/config.rs`

| Property | Value |
|----------|-------|
| Purpose | INI-style config parsing: repo + global + env overrides |
| Key methods | `load()`, `load_with(git_dir, global)`, `get(section, key)`, `get_bool(section, key)`, `get_int(section, key)`, `check_repository_version()`, `author_identity()`, `committer_identity()`, `user_identity()` |
| Behavior | repo layer wins over global; `[section "sub"]` subsections collapsed into section slot (decisions D-004); version guard accepts 0/1, rejects 2+ and non-numeric values (decisions D-005); `#`/`;` comments, bare key = empty value (bool true), trailing-`\` line continuation; unknown sections/keys ignored |

---

## Object Modules

### Object
File: `src/object/mod.rs`

| Property | Value |
|----------|-------|
| Purpose | Object body dispatch: blob/tree/commit/tag |
| Key methods | `Object::parse(kind, bytes)`, `Object::serialize()`, `Object::kind()`, `parse_oid_line(what, line)` (crate-private) |

### Tree
File: `src/object/tree.rs`

| Property | Value |
|----------|-------|
| Purpose | Tree parse/serialize, git's base_name_compare sort |
| Key methods | `Tree::parse(bytes)`, `Tree::serialize()`, `base_name_compare` |
| Format | `<octal-mode> <name>\0<20-byte oid>` entries; empty tree valid; bad mode/empty name/slash in name/truncated oid → `Corrupt` |

### Commit
File: `src/object/commit.rs`

| Property | Value |
|----------|-------|
| Purpose | Commit parse/serialize; `Ident` shared with tags |
| Key methods | `Commit::parse(bytes)`, `Commit::serialize()`, `Ident::new(name, email, ts, tz)`, `Ident::parse(line)`, `Ident::render()` |
| Format | strict: tree → parent* → author → committer → blank → message; ident `Name <email> ts tz`; tz `-1200..=+1400` |

### Tag
File: `src/object/tag.rs`

| Property | Value |
|----------|-------|
| Purpose | Annotated tag parse/serialize |
| Key methods | `Tag::parse(bytes)`, `Tag::serialize()` |
| Format | strict: object → type → tag → tagger → blank → message |

---

## Algorithm Modules

### DiffEngine
File: `src/diff.rs`

| Property | Value |
|----------|-------|
| Purpose | Myers line diff, unified output matching git's format (D-014: no `xdl_change_compact` port — identical-run boundary placement may differ from git, content identical) |
| Key methods | `split_lines(bytes)` (split on `\n`, keep `\n` in line content), `is_binary(bytes)` (NUL in first 8000 bytes), `diff_lines(old, new) -> Vec<Hunk>` (prefix/suffix trim → Myers O(ND) → group+split hunks at `> 2*ctxlen` gap, funcname suffix: scan from hunk pre-context start-1 down to previous hunk's start-1 (exclusive), first line starting with alpha/`_`/`$`, ≤80 bytes, trailing-ws trimmed, sticky carry-over), `render(f: &FileDiff) -> Vec<u8>` (`diff --git`/`index`/`---`/`+++` headers, `Binary files a/x and b/x differ`, hunks `@@ -s,c +s,c @@ [funcname]`, `\ No newline at end of file` on its own line after unterminated records) |

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
| Purpose | Commit graph traversal: max-heap by committer timestamp, visited dedup, `-n` limit; revision resolution (40-hex, `HEAD`, `refs/heads/<n>`, `refs/tags/<n>`, tag peeling) |
| Key methods | `Revwalk::new(store)`, `set_limit(n)`, `seed(sha)` (commit or tag, recursive peel), `pop_next()` (name is `pop_next` to avoid clippy's Iterator-confusion lint), `resolve_rev(refs, store, name) -> Result<Option<[u8;20]>>` (40-hex, `HEAD`/`@`, `HEAD~N` first-parent walk with nested `~`, `refs/heads/<n>`, `refs/tags/<n>`; nothing else falls back to HEAD — unknown names → `None`, step-10 fix), `unborn_fatal(refs)`, `object_name_error(name)` (git's exact `fatal: ambiguous argument '<n>': unknown revision or path not in the working tree.` + `Use '--'` hint), `hex(oid)`, `parse_oid(str)` |
| Behavior | tie-break on equal timestamps is heap-pop order (insertion order), matching git's date-order semantics on distinct-date history; `--all` seeds refs/heads + refs/tags + HEAD (no remotes/stash/notes — D-015); tags peel to their commit; a ref to a non-commit yields `None` (`ambiguous argument`, like git) |

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
| `add` | `add.rs` | `<pathspec>...` + `.`; stages deletions of tracked files (named path or inside a dir arg); `add .` skips ignored files silently; explicit ignored FILE pathspec aborts with git's exact message + hints, exit 1 (bare message, no `fatal:` prefix — decision D-013); missing untracked path → `fatal: pathspec 'x' did not match any files`, exit 128; paths relative to cwd (`../` escapes refused), stat fields written per worktree.rs |
| `commit` | `commit.rs` | `-m <msg> [-m <msg>] [-a] [-q]`; joins messages with `\n`, per-line trailing-ws strip, trailing blank lines dropped, internal blanks kept; object message gets a single trailing `\n` (D-016); identity: env > config with git's fallback chain (committer falls back through user config then the author env pair; missing email → git's `identity unknown` hint block + `unable to auto-detect email address (got '<user>@<HOST>.(none)')`, exit 128); empty-commit probes: head-tree==index-tree → `no changes added to commit ...` / `nothing to commit, working tree clean`, unborn+empty index → `nothing to commit (create/copy files ...)`, unborn+untracked → `nothing added to commit but untracked files present ...` (v1 prints only the final line, not git's status block — D-015); empty message aborts AFTER those checks with `Aborting commit due to empty commit message.` (D-016); `-a` restages modified tracked files (`restage_all` via add::build_entry, index/content differ); tree via `tree_from_index` (group by dir, subtrees sorted per 04's base_name_compare); ref update via `refs.update("HEAD", ...)` + reflog `commit (initial): <subject>` for root, `commit: <subject>` after (D-015); dates: `GIT_AUTHOR_DATE`/`GIT_COMMITTER_DATE` (only `<unix-ts> <tz>` form) each falling back to the other, then now; success output silent (D-015) |
| `status` | `status.rs` | porcelain short: tracked section (X=index vs HEAD `A/M/D/R`, Y=worktree vs index `M/D/ `, hashing worktree files — no stat shortcut), then untracked `??` section; both sorted; untracked dirs collapse to the topmost ancestor with no tracked descendants (`dir/` sorts after `dir`-prefixed files); rename detection = exact oid match HEAD↔index only (decision D-013); paths relative to cwd with `../` prefix (GIT_DIR required from subdirs — D-002), C-quoted (spaces force quotes, octal escapes for non-ASCII/control, probed against git 2.55) |
| `log` / `show` | `log.rs`, `show.rs` | `log`: oneline always (`<short-sha> <subject>`), `--all` (refs/heads+refs/tags+HEAD seeds), `-n <count>`, `--graph` (`* ` prefix, linear only — D-015); unborn HEAD → `fatal: your current branch '<name>' does not have any commits yet` (exit 128); unborn `--all` silent exit 0; bad rev → `object_name_error` (revwalk.rs). `show [<rev>]` (default HEAD): `commit <sha>` + `Author:` + `Date:` (author date in the ident's tz, hand-rolled weekday/month names + civil_from_days) + indented message + stat block (`<name padded> | <n> <+->`, `Bin <old> -> <new> bytes` for binary, `N files changed, X insertions(+), Y deletions(-)` with singulars, unchanged blobs skipped — D-016); no patch body, annotated tags peel without the tag-object header (D-015) |
| `diff` | `diff.rs` | `git diff` (worktree vs index), `--cached`/`--staged` (index vs HEAD), `-- <paths>` (plain pathspecs: exact prefix match on repo-relative path, no globs — D-014); HEAD blobs via `load_head_tree` (unborn HEAD = empty); worktree diff skips paths in HEAD but not in index (staged deletions); gitlink (mode 160000) entries skipped — D-014; `-M`/`-R` print "not supported" and exit 1; path labels via `quote_two` (git's CQUOTE_NODQ: spaces unquoted, non-ASCII octal-escaped in one quote pair), `---`/`+++` labels get a trailing tab when they contain a space, `Binary files` line uses the `/dev/null`-aware labels (probed vs git 2.55); byte-identical on distinct-line fixtures incl. spaced/non-ASCII/binary names (tests/diff.rs) |
| `branch` | `branch.rs` | `<name> [<start>]` create (existing name → `a branch named '<name>' already exists` fatal, exit 128; bad start rev → `branch: not a valid revision: '<rev>'`, exit 128); `-a`/`-l` list: `* ` current branch, `(HEAD detached at <7-sha>)` marker, sorted lexicographic, `refs/heads/` stripped; `-d <name>`: refuse current branch (`error: cannot delete branch '<name>' used by worktree at '<root>'`, forward-slash root path, exit 1), refuse unmerged (`error: the branch '<name>' is not fully merged` + 2 hints, exit 1; merged = merge_base(HEAD, tip) == tip); `-D` skips the merged check; `-q` silent; delete removes ref + reflog (`Refs::delete`) |
| `tag` | `tag.rs` | `tag <name>` lightweight (blob/tree/commit/tag all allowed), `-a -m <msg>` annotated (object/tag.rs serialize; tagger = committer chain env>config>author, dates via `commit::commit_dates` — byte-parity with git when env dates set; missing `-m` in v1 does NOT spawn an editor — D-017), `-d <name>` (`Deleted tag '<name>' (was <7-sha>)`), `-l`/no-arg list sorted plain lexicographic (matches git 2.55 refname sort — D-017), `refs/tags/` stripped; duplicate name → `fatal: tag '<name>' already exists`; no reflog for tags (`Refs::update` skips `refs/tags/`) |
| `checkout` | `checkout.rs` | `[-b <name>] [-f] [-q] [<target>]`; target order: unresolved `-b` → `HEAD`; refs/heads → refs/tags (peeled to commit, non-commit tag → fatal `tag '<name>' does not point to a commit`) → revision-like (`resolve_rev`, detached); `-b` creates branch at target then switches; unknown target → `error: pathspec 'x' did not match any file(s) known to git` (exit 1); dirty gate: same-tree switch allowed while dirty, cross-tree refused unless `-f` (`error: Your local changes to the following files would be overwritten by checkout:\n\t<path>` + `Please commit your changes or stash them before you switch branches.\nAborting`, exit 1 — D-017 deviation: untracked-file overwrite protection not in v1); messages: `Switched to branch '<name>'` / `Switched to a new branch '<name>'` / detached `HEAD is now at <7-sha> <subject>`; materialization via `sync_worktree`/`force_sync_worktree` (files BEFORE index rewrite — stat-parity), HEAD move via `set_head_symref`/`refs.update` |
| `config` | `config.rs` | Read/write config values |
| `merge` | `merge.rs` (cmd), `merge.rs` (algo), `merge-base` in `merge.rs` (cmd) | `merge <branch|tag|sha>`: strict dirty gate (index == HEAD tree, refused with ort wording, exit 2), merge-base via `revwalk::merge_base` (first ancestor; unrelated → `fatal: refusing to merge unrelated histories`, exit 128), per-path 3-way resolution table (base==ours→theirs; base==theirs→ours; ours==theirs→either; all-differ → whole-file conflict, no hunk auto-merge), worktree written BEFORE index (stat parity), merged index = existing stage-0 entries + replaced/removed paths, sorted by path, conflict paths at stages 1/2/3 with zero stat; state files MERGE_HEAD/MERGE_MSG/ORIG_HEAD written before output; conflict output on stdout (`Auto-merging` + `CONFLICT (content|add/add|modify/delete): ...` + `Automatic merge failed; fix conflicts and then commit the result.`, exit 1 — sentinel `GitError::Invalid(empty)`); success = merge commit (NO fast-forward — D-018), `Merge made by the 'ort' strategy.` + reflog `merge <label>: ...`, state files removed, ORIG_HEAD kept; `--abort` = `reset --hard ORIG_HEAD` + remove state (`fatal: There is no merge to abort (MERGE_HEAD missing).`, exit 128); `merge-base <rev1> <rev2>` prints best common ancestor, bad rev → `fatal: Not a valid object name <rev>` (128); label = branch/tag name or 7-char sha; `-q` suppresses `Auto-merging`/`CONFLICT`/success lines; merge-commit message `Merge branch|tag '<x>'` / `Merge commit '<7-sha>'`, MERGE_MSG gets `# Conflicts:` block when conflicted. Algorithm module `src/merge.rs`: `merge_trees(base, ours, theirs) -> MergeResult{files: Vec<MergeFile>}`, `conflict_marker` (byte-exact whole-file markers), unit tests incl. CRLF |
| `reset` | `reset.rs` | `[--soft|--mixed|--hard] [-q] [<commit>]` (default `--mixed` to HEAD); `--soft` moves HEAD only; `--mixed` + rewrites index to target tree (fresh stat per worktree.rs) + `Unstaged changes after reset:` block (stdout; `M\t<path>` / `D\t<path>` lines; suppressed by `-q`); `--hard` + `force_sync_worktree` FIRST then index rewrite (ordering = stat parity, probed — D-017) + `HEAD is now at <7-sha> <subject>` (stdout); unknown option → `reset: unknown option '<x>'` (exit 1); bad rev → 3-line `object_name_error` fatal (exit 128); unborn HEAD + no target → `fatal: ambiguous argument 'HEAD'` family; `hard_sync(store, root, idx, ipath, old_tree, new_tree)` pub(crate) — force-sync + delete index-only files + rewrite index, shared with rebase start/abort |
| `rebase` | `rebase.rs` | `rebase <upstream>` / `--continue` / `--abort` / `--skip`. Preludes (probed, D-019): detached HEAD refused (own fatal); bad upstream → `invalid upstream '<x>'` (128); in-progress state dir → git's 341-byte refusal block (128); up-to-date = fork == upstream tip AND linear first-parent chain to it (git's can_fast_forward + is_linear_history — a merge on the chain replays) → `Current branch <b> is up to date.` (stdout, rc 0); empty range → silent fast-forward (branch ref → upstream tip, worktree/index synced, success line only). Range = `rev-list --reverse --topo-order upstream..HEAD` (indegree-stack topo, date desc, merges flattened silently). Replay = 3-way per pick (parent tree base / HEAD tree ours / pick tree theirs; root pick → empty tree), labels `<<<<<<< HEAD` / `>>>>>>> <7sha> (<subject>)`; progress `Rebasing (k/N)\r` + success line `Successfully rebased and updated refs/heads/<b>.` on stderr; conflict stop rc 1 (stdout `Auto-merging`/`CONFLICT` lines via shared `print_merged_lines`; stderr error/hint block); became-empty picks dropped silently; empty commits kept (drop-empty=false, D-019); state files (head-name/onto/orig-head/msgnum/end/git-rebase-todo/done/message/author-script) own format under `.git/rebase-merge/`; `--continue` (unmerged refusal `path: needs merge` block on stdout rc 1) commits staged index with original author (author-script verbatim) + fresh committer, prints `[detached HEAD <7sha>] <subject>` + ` Author:` line + stat summary only (shared `show::stat_summary`), reflog `rebase (continue): <subject>`; `--abort` silent (branch restored via `Refs::update_quiet` without reflog, HEAD symref reflog `rebase (abort): returning to refs/heads/<b>`); no-state → `fatal: no rebase in progress` (128); reflogs `rebase (start): checkout <upstream>` / `rebase (pick): <subject>` / finish messages; ORIG_HEAD = pre-rebase HEAD; detached HEAD writes via `Refs::set_head_sha` (also fixes checkout's detach) |
| plumbing | `hash_object.rs` | hash-object, cat-file, ls-tree, update-ref, etc. |
| `fsck` | `fsck.rs` | Full integrity walk |