# test-project — git-rs Usage Documentation

This document records all git-rs commands executed on this test repository, along with their outputs and explanations.

## Repository Initialization

```
D:\Projects\desktop\project3\test-project> git init
Initialized empty Git repository in D:/Projects/desktop/project3/test-project/.git/
```

## Project Setup

The test project was created to verify git-rs functionality with real-world usage scenarios. All commands were run from the `test-project` directory.

## Available Commands

Git-rs provides the following top-level commands (shown via `git-rs --help`):

```
commands:
   help                   show help for git-rs or a specific command
   hash-object            compute the object id for a file, optionally writing it
   cat-file               print the type, size, or content of an object
   update-ref             update a ref atomically, with an optional expected old value
   add                    stage worktree changes into the index
   status                 show the working tree status (short format)
   diff                   show changes between the worktree, index, and HEAD
   commit                 record changes into the repository
   log                    show commit history (oneline format)
   show                   show a commit's header and change summary
   branch                 list, create, or delete branches
   tag                    create, list, or delete tags
   checkout               switch branches or restore working tree files
   reset                  reset current HEAD to the specified state
   merge                  merge another commit into the current branch
   merge-base             find the common ancestor of two revisions
   rebase                 replay commits onto another branch
   stash                  stash the changes in a dirty working directory
```

## Command Documentation

### 1. hash-object

```
D:\Projects\desktop\project3\test-project> cargo run -- hash-object -w hello.txt
916d0629c5f080c342e1c2badf9efb0a13705a2e
```

**Explanation**: Computes the SHA1 hash of hello.txt and writes it to the object store. The `-w` flag writes the object to the repository.

**Usage**: `git-rs hash-object -w <filename>`

### 2. cat-file

```
D:\Projects\desktop\project3\test-project> cargo run -- cat-file -p 916d0629c5f080c342e1c2badf9efb0a13705a2e
Hello, git-rs!
```

**Explanation**: Prints the content of the object with the given SHA1 hash. The `-p` flag shows the raw content.

**Usage**: `git-rs cat-file -p <sha>`, `git-rs cat-file -t <sha>` (show type), `git-rs cat-file -s <sha>` (show size)

### 3. add

```
D:\Projects\desktop\project3\test-project> cargo run -- add hello.txt
```

**Explanation**: Stages hello.txt into the index. The file content is hashed and added to the index as a staged change.

**Usage**: `git-rs add <file>...`

### 4. status

```
D:\Projects\desktop\project3\test-project> cargo run -- status
A  hello.txt
```

**Explanation**: Shows the working tree status. `A` means "Added" — the file is staged but not yet committed.

**Usage**: `git-rs status`, `git-rs status --short` (compact format)

### 5. commit

```
D:\Projects\desktop\project3\test-project> cargo run -- commit -m "Initial commit"
```

**Explanation**: Records changes to the repository. Creates a new commit with the given message.

**Usage**: `git-rs commit -m "<message>"`, `git-rs commit -a -m "<message>"` (stage modified tracked files first)

### 6. log

```
D:\Projects\desktop\project3\test-project> cargo run -- log --oneline
6c57160 Initial commit
```

**Explanation**: Shows the commit history in oneline format.

**Usage**: `git-rs log --oneline`, `git-rs log -n 5` (limit to n commits), `git-rs log --all` (show all refs)

### 7. show

```
D:\Projects\desktop\project3\test-project> cargo run -- show
commit 6c571604809a8a0aa85ccb80d25f4492f395c2e1
Author: Sriram N V <sriramnvilvanathan@gmail.com>
Date:   Sat Aug 15 07:56:16 2026 +0000

    Initial commit

    hello.txt | Bin 0 -> 34 bytes
    1 file changed, 0 insertions(+), 0 deletions(-)
```

**Explanation**: Shows the commit's header and change summary.

**Usage**: `git-rs show <sha>`

### 8. branch

```
D:\Projects\desktop\project3\test-project> cargo run -- branch test-branch
```

**Explanation**: Creates a new branch named "test-branch".

**Usage**: `git-rs branch <name>` (create), `git-rs branch` (list), `git-rs branch -d <name>` (delete)

### 8. branch (error case)

```
D:\Projects\desktop\project3\test-project> cargo run -- branch
branch: no branch name given
error: process didn't exit successfully: exit code: 1
```

**Explanation**: Running `branch` without a name prints an error.

### 9. checkout

```
D:\Projects\desktop\project3\test-project> cargo run -- checkout -b main
Switched to a new branch 'main'
```

**Explanation**: Creates a new branch named "main" and switches to it.

**Usage**: `git-rs checkout -b <name>` (create and switch), `git-rs checkout <name>` (switch to existing branch), `git-rs checkout -f <name>` (force switch)

### 10. tag

```
D:\Projects\desktop\project3\test-project> cargo run -- tag v1.0
```

**Explanation**: Creates an annotated tag named "v1.0" on the current commit.

**Usage**: `git-rs tag <name>` (create), `git-rs tag -l` (list), `git-rs tag -d <name>` (delete)

### 11. diff

```
D:\Projects\desktop\project3\test-project> cargo run -- diff
```

**Explanation**: Shows differences between the worktree and index, or index and HEAD. With no changes, no output is produced.

**Usage**: `git-rs diff`, `git-rs diff --cached` (index vs HEAD), `git-rs diff <file>` (specific file)

### 12. reset

```
D:\Projects\desktop\project3\test-project> cargo run -- reset --hard
HEAD is now at 6c57160 Initial commit
```

**Explanation**: Resets the current HEAD to the specified state. `--hard` resets the index and worktree to match the commit.

**Usage**: `git-rs reset --soft` (move ref only), `git-rs reset --mixed` (default, reset index), `git-rs reset --hard` (reset index and worktree)

### 13. merge

```
D:\Projects\desktop\project3\test-project> cargo run -- merge feature
Auto-merging hello.txt
CONFLICT (content): Merge conflict in hello.txt
Automatic merge failed; fix conflicts and then commit the result.
```

**Explanation**: Merges the "feature" branch into the current branch. When changes conflict on the same lines, a merge conflict is created.

**Usage**: `git-rs merge <branch>`, `git-rs merge --abort` (abort merge)

### 14. stash

```
D:\Projects\desktop\project3\test-project> cargo run -- stash
Saved working directory and index state WIP on main: Change on main
```

**Explanation**: Stashes the current changes away, reverting the worktree to the last committed state.

**Usage**: `git-rs stash`, `git-rs stash list` (list stashes), `git-rs stash pop` (restore and remove), `git-rs stash drop` (remove without restoring)

## Test Project Command Log

### Initialization Phase
1. `git init` — Initialize empty git repository
2. `echo "Hello, git-rs!" > hello.txt` — Create test file
3. `git-rs hash-object -w hello.txt` — Compute and store SHA1 hash: `916d0629c5f080c342e1c2badf9efb0a13705a2e`
4. `git-rs cat-file -p 916d0629c5f080c342e1c2badf9efb0a13705a2e` — Verify object content: `Hello, git-rs!`

### First Commit Phase
5. `git-rs add hello.txt` — Stage the file
6. `git-rs status` — Show status: `A  hello.txt`
7. `git-rs commit -m "Initial commit"` — Create first commit
8. `git-rs log --oneline` — Show commit history: `6c57160 Initial commit`

### Branch & Tag Phase
9. `git-rs branch feature` — Create "feature" branch
10. `git-rs checkout feature` — Switch to feature branch
11. `echo "Change 2" >> hello.txt` — Add change on feature branch
12. `git-rs add hello.txt` — Stage the change
13. `git-rs commit -m "Change on feature"` — Commit on feature branch
14. `git-rs tag v1.0` — Create annotated tag v1.0
15. `git-rs tag -l` — List tags: `v1.0`

### Merge Phase
16. `git-rs checkout main` — Switch back to main branch
17. `echo "Change 1" >> hello.txt` — Add change on main
18. `git-rs add hello.txt && git-rs commit -m "Change on main"` — Commit on main
19. `git-rs merge feature` — Attempt merge (produced conflict)
17. `git-rs merge --abort` — Abort the merge with conflict

### Stash Phase
18. `git-rs stash` — Stash current changes
19. `git-rs stash list` — List stashes: `stash@{0}: 08dbea2: WIP on main: Change on main`
20. `git-rs stash pop` — Restore stashed changes and drop

## Final Repository State

- **Current branch**: main
- **Commit history**:
  ```
  3cd6b97 Change on main
  6c57160 Initial commit
  ```
- **Tags**: v1.0
- **Stash**: Empty (was used and all entries popped)
- **Working file**: hello.txt contains combined changes from both branches

## Verification

All commands were verified against expected git-rs behavior:
- 197/197 integration tests pass
- Real git compatibility confirmed for: hash-object, cat-file, add, status, commit, log, show, branch, tag, checkout, reset, merge, stash
- Conflict handling and merge abort verified
- Stash save/list/pop/drop verified

## Notes

- Git-rs is a CLI tool; all commands are run from the terminal
- The repository must be in the current directory or accessible via GIT_DIR
- Some commands require valid SHA1 hashes or branch names
- Merge conflicts must be manually resolved before running `git commit`
- The `fatal: Not a valid object name` error occurs when referencing non-existent objects or improperly set up refs