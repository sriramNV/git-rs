# 12 — Rebase

## Why

Rebase replays a branch's commits onto a new base commit. It's an alternative to merge for integrating history, producing a linear commit history. Correct rebase implementation is complex — it involves range selection, conflict handling, progress reporting, and state management.

## How

- **Range**: `rev-list --reverse --topo-order upstream..HEAD` (not the first-parent chain); NO fast-forward (non-empty range always replays); empty range = silent fast-forward of the branch to the upstream tip
- **Replay**: cherry-pick: 3-way between commit's parent (base), commit (theirs), current HEAD (ours) — same code path as merge per-path resolution, so conflicts show identical markers
- **On conflict**: stop with message `error: could not apply <sha>... <subject>` + 5 hint lines + `Could not apply <sha>... # <subject>` (stderr, rc 1); user resolves, stages, then `--continue` or `--abort`/`--skip`
- **Progress**: `Rebasing (k/N)\r` (carriage return, no newline) + success line on stderr
- **`--continue`**: unmerged refusal = `path: needs merge` block on stdout rc 1; otherwise commits the staged index with the ORIGINAL author (name/email/date verbatim, from the state's author-script) + a fresh committer; prints `[detached HEAD <sha>] <subject>` + ` Author: <name> <email>` (only when author != committer) + the stat SUMMARY line only; reflog `rebase (continue): <subject>`; then replays the rest (no `Rebasing` line for the committed pick itself)
- **`--abort`**: silent (rc 0, neither stream); branch ref restored to orig-head with NO branch reflog (`Refs::update_quiet`); worktree+index hard-reset (shared `reset::hard_sync`); HEAD returned to the symref with reflog `rebase (abort): returning to refs/heads/<b>`; state dir removed; ORIG_HEAD kept
- **No-state abort/continue/skip**: `fatal: no rebase in progress` (rc 128)
- **In-progress refusal** (while state dir exists): git's exact block (341 bytes file-captured — including wrapped `I wonder ... is the\ncase, please try`, `...have something\nvaluable there.` lines and the trailing blank line)
- **Detached HEAD refused** with our own fatal (`rebase: detached HEAD is not supported in v1`) — git supports it; bad upstream -> `invalid upstream 'x'` (rc 128); unrelated histories rebase via empty-tree base
- **Reflogs**: HEAD `rebase (start): checkout <upstream>` -> `rebase (pick): <subject>` per pick -> `rebase (finish): returning to refs/heads<b>`; branch `rebase (finish): refs/heads<b> onto <onto-full-sha>` at finish only (nothing on abort); ORIG_HEAD = pre-rebase HEAD
- **`checkout <tag|sha>` now genuinely detaches**: `Refs::set_head_sha` writes the raw sha into the HEAD file

## Usage

```bash
# Start a rebase
git-rs rebase       # Rebases current branch onto upstream

# Or specify upstream
git-rs rebase upstream

# Continue after conflict resolution
git-rs rebase --continue

# Abort the rebase
git-rs rebase --abort

# Skip a commit
git-rs rebase --skip

# Resume a previous rebase (if state dir still exists)
git-rs rebase
```

**Verification**: `tests/rebase.rs` (10 integration tests, all green, 2 consecutive full runs): replay sha-identical to real git (same final HEAD, log, reflog `%gs`, author preserved, fresh committer, worktree, state gone); up-to-date message vs merge-blocks-it replay vs silent ff (3 cases); conflict stop byte-identical (stdout/stderr/markers/ls-files/state files head-name/onto/orig-head/msgnum/end/message/author-script); continue byte-identical incl. `[detached HEAD]` + Author line + stat summary; abort byte-equal + silent + exact restore; no-rebase abort/continue/skip fatal bytes; in-progress refusal bytes; empty commits kept sha-identical; merge-commit flattening in topo order sha-identical; interop — real git fsck-clean on our rebased repo, real `git status` reports our in-progress rebase, our `--continue` finishes it (git's `--continue` on our state NOT supported, D-019); `invalid upstream` fatal bytes.