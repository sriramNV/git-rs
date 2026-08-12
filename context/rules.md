# Rules

Concise git-format correctness rules. These are non-negotiable — deviate and real git will reject or misread our repositories.

## Object Format Rules

- **Loose object layout**: stored as `<zlib of header + content>` where header is `<type> <content-size>\0` — `"blob 1234\0"`. Never vary the header format.
- **Object ID**: `sha1(header + content)`, written lowercase hex, 40 chars
- **Storage path**: `.git/objects/<first-2-hex>/<remaining-38-hex>` — lower hex only, no padding, no subdirs beyond one level
- **Objects are immutable**: never rewrite an object; a changed content is a new object with a new id. `fsck` must always pass.
- **Tree entries**: `<mode> <name>\0<20-byte-sha>` — mode is an octal string (`100644`, `100755`, `120000`, `040000` for subtree… use padded 6-digit octal), entries sorted by name bytewise (with special dir-trailing-slash rules from git's `base_name_compare`)
- **Commit header**:
  ```
  tree <sha>
  parent <sha>            (0+ parent lines)
  author Name <email> <unix-ts> <tz>
  committer Name <email> <unix-ts> <tz>

  <message>
  ```
  Exact spacing: `<unix-ts> <tz>` with tz like `+0530`. Author and committer lines are mandatory, even if identical.
- **Tag object**: `object <sha>\ntype <type>\ntag <name>\ntagger <ident>\n\n<message>`
- **Type names**: `blob`, `tree`, `commit`, `tag` — lowercase, no underscores

## Object Store Rules

- Empty garbage field in object headers (`<type> <size>\0`) is the ONLY delimiter — sizes are decimal, no leading zeros
- zlib stream must decompress completely — trailing garbage after the object body is corruption
- Never compress loose objects with raw deflate — it must be zlib-wrapped (flate2's `ZlibEncoder`, not `DeflateEncoder`)

## Index (v2) Rules

- Header: `DIRC` + version `2` (u32) + entry count (u32)
- Each entry: 62-byte fixed part + variable path (NUL-terminated), padded so entries are 8-byte aligned (up to 8 NULs)
- Checksum: sha1 of all preceding bytes (header + all entries), appended as last 20 bytes
- Stat data (ctime, mtime, dev, ino, uid, gid, size) must be read on entry; when writing, real git round-trips them — preserve unknown fields rather than zeroing
- Flags: assume-valid (1<<15), extended (1<<14); stage mask is bits 12-13
- extensions (TREE, REUC, etc.) must be preserved on rewrite or stripped; unknowable extensions must never break reading

## Refs Rules

- Ref files are plain text holding `<sha>\n` (with possible trailing newline) — `ref: refs/heads/<name>\n` for symrefs (HEAD)
- Ref names never contain `..`, never start with `.`, never contain ` `, `~`, `^`, `:`, `?`, `*`, `[`, `\`, or control chars
- **Atomic update**: never write a ref file by truncating in place. Write to a temp file and rename over the target. Ref corruption = corrupted repository.
- `packed-refs` lines: `<sha> <refname>`, `^<sha>` lines for peeled tags, `# pack-refs with:` header comment
- Resolve order: loose ref wins over packed-refs
- HEAD defaults to unborn branch `refs/heads/main` — `core.bare` false by default

## Working Tree Rules

- `checkout`/`reset` never touch untracked files unless the command explicitly removes them (`--hard` semantics)
- File mode: only `100644`/`100755` are materialized (exec bit), symlinks as `120000`
- Line endings: write files exactly as stored in blobs — no CRLF translation, ever
- On checkout, files that are identical between old and new tree stay untouched (preserve mtime) — git only rewrites files when content changes

## Config Rules

- `.git/config` / global `~/.gitconfig` are INI-style: `[section "subsection"]`, `key = value`
- `repositoryformatversion` must be `0` — refuse to work on repos with a higher version
- Unknown extensions are ignored unless `extensions.*` gates them
- Env vars override file config: `GIT_DIR`, `GIT_OBJECT_DIRECTORY`, `GIT_INDEX_FILE`, `GIT_AUTHOR_NAME/EMAIL`, `GIT_COMMITTER_NAME/EMAIL`, `GIT_CONFIG_*`

## Merge / Diff Rules

- 3-way merge base: merge-base = lowest common ancestor(s) in commit graph; recurse when multiple bases
- Conflict markers: `<<<<<<< HEAD`, `=======`, `>>>>>>> <ref>` — exact format, no extra characters
- Rename detection is opt-in and heuristic (`-M`); never claim renames by default in ways that change diff output vs real git
- Dates in commits are epoch seconds + tz offset, validated: real git rejects impossible dates

## Do Nots

- Never write extra metadata (custom files, custom headers, extra objects) — everything we write must be something real git actually writes
- Never create objects in alternate object stores unless asked
- Never auto-gc mid-command — gc is an explicit command
- Never delete loose objects when writing a pack — leave originals; `git gc`-style pruning is out of scope per command
- Never log object contents or user data to stderr beyond git's own diagnostics
- Never panic on corrupt input — `Corrupt` error or a clear message, and exit code 128 (git's fatal signal) for failure
- Keep exit codes compatible: 0 success, 1 generic failure, 128 fatal