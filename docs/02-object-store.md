# 02 — Object Store — Loose Objects

## Why

The object store is the foundation on which everything else layers. Loose objects (blobs, commits, tags, trees) stored in `.git/objects/` must be readable and writable with correct sha1 hashing and zlib compression — matching real git byte-for-byte.

## How

- **Object id**: `sha1(kind + " " + size + "\0" + content)` → 40 lowercase hex
- **Write path**: zlib-compress header+content, write to `.git/objects/xx/38hex` (temp file + atomic rename)
- **Read path**: locate file, zlib-decompress, parse header, verify size matches content, verify sha1 matches the id requested
- **Plumbing**: `hash-object [-w]`, `cat-file [-p|-t|-s]` in `commands/hash_object.rs`
- **Header format**: exactly `<type> <size>\0` with decimal size — never anything else
- **zlib only** — never raw deflate for loose objects

## Usage

```bash
# Hash a file and write to git object store
git-rs hash-object -w hello.txt

# Cat-file: show object type/size/content
git-rs cat-file -p <sha>
git-rs cat-file -t <sha>   # Show type
git-rs cat-file -s <sha>   # Show size in bytes

# Example
git-rs hash-object -w hello.txt
# Output: <sha1-of-hello.txt>
git-rs cat-file -p <sha>
# Output: Hello, world! (the file content)
```

**Verification**: In a real `git init` repo: our `hash-object -w` produces the same sha as real `git hash-object -w`; real `git cat-file -t/-s/-p` reads our objects; we read real git's objects.