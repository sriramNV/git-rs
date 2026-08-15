# 11 — Three-Way Merge

## Why

Three-way merge is the fundamental operation for combining two branches of history. It uses a base (common ancestor) plus the two branch tips to produce a merged result, with conflict detection when the blobs diverge.

## How

- **Path resolution rules**: base==ours → take theirs; base==theirs → take ours; ours==theirs → take it; all three differ → conflict
- **Conflict file content**: `<<<<<<< HEAD\n<ours>\n=======\n<theirs>\n>>>>>>> <branch>\n` — exact marker format, `<branch>` is the merge source ref name
- **Clean merge tree write**: build result tree from resolved entries (reuse tree builder from 09) — the result tree must be sha-identical to real git's for clean merges
- **Merge commit**: parents = [HEAD, MERGE_HEAD], message `Merge branch '<x>'` (+ `into <y>` when HEAD not on main... v1: plain `Merge branch '<x>'`), author/committer rules from 09
- **Conflict state**: `.git/MERGE_HEAD` + `.git/MERGE_MSG`, merge commit on success, cleanup on resolution (`commit` completes it)
- **On success**: delete nothing automatically (that's the branch command's job — real git deletes merged branches only with `-d`)
- **No fast-forward, no `Already up to date.`** — every successful merge makes a merge commit (locked, D-018)
- **Strict dirty gate (locked)**: merge refused when index != HEAD tree, before any state is written: `error: Your local changes to the following files would be overwritten by merge:\n  <path>\nMerge with strategy ort failed.` (stderr, exit 2 — probed: this is the wording git's ort uses on a genuine diverged merge)
- **Conflict output (stdout)**: `Auto-merging <path>` + `CONFLICT (content|add/add): Merge conflict in <path>` / `CONFLICT (modify/delete): <path> deleted in <side> and modified in <side>.  Version <side> of <path> left in tree.` + `Automatic merge failed; fix conflicts and then commit the result.` (exit 1)
- **Unrelated histories**: `fatal: refusing to merge unrelated histories` (exit 128, probed)
- **MERGE_MSG**: `Merge branch '<x>'` + blank + `# Conflicts:\n#\t<path>` (only when conflicts exist); `commit` without `-m` during a merge uses MERGE_MSG with `#` comment lines stripped; commit during a merge skips the empty-commit checks; reflog `commit (merge): <subject>`; success reflog `merge <label>: Merge made by the 'ort' strategy.`; ORIG_HEAD written on every merge (before conflicts), removed state on success, kept on conflict (git parity)
- **`merge --abort`**: `reset --hard ORIG_HEAD` + delete MERGE_HEAD/MERGE_MSG; no merge in progress → `fatal: There is no merge to abort (MERGE_HEAD missing).` (exit 128, probed)
- **`reset --hard` root cause fix**: also deletes worktree files tracked in the current index but absent from the target tree (a merge's staged additions / a staged new file) — matches git; this is why `merge --abort` leaves no leftover files
- **`commit` with an unmerged index**: `U\t<path>` per unique path (stdout) + `error: Committing is not possible because you have unmerged files.` hint block (stderr), exit 128 — probed
- **Index during a conflict**: stages 1/2/3 written for conflicted paths, untouched paths keep their stage-0 entries, entries sorted by path (git's index order); conflict entries carry zero stat

## Usage

```bash
# Start a merge
git-rs merge feature

# Resolve conflicts manually, then:
git-rs commit   # Completes the merge

# Abort a merge
git-rs merge --abort  # Restores index+worktree from ORIG_HEAD

# Handle unrelated histories
git-rs merge --verbose unrelated-repo 2>&1; echo "exit: $?"
```

**Verification**: `tests/merge.rs` (11 integration tests, all green): byte-parity vs real git 2.55 on twin fixed-date repos — clean merge produces the SAME merge-commit sha, tree sha, worktree bytes, reflog `merge feature:` message and ORIG_HEAD; content/add-add/modify-delete conflicts produce identical stdout/stderr bytes, marker bytes, `ls-files -s` stage sets, MERGE_HEAD/MERGE_MSG/ORIG_HEAD files; dirty gate block (exit 2) and unrelated-histories fatal (128) byte-equal; `commit` during unmerged state byte-equal (`U\t` + error block, 128); `merge-base` output and bad-rev fatal equal; interop both ways — real git finishes our conflicted merge (2-parent commit, state cleaned), we finish real git's using MERGE_MSG with reflog `commit (merge): Merge branch 'feature'`; `--abort` byte-equal state + no leftover untracked files, and missing-merge abort fatal equal.