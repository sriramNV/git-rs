# 14 — Packfiles — Reading

## Why

Pack files combine multiple objects into a single file for efficient storage and transfer. The idx (index) file provides random access to the objects within the pack. Correct pack/idx reading is essential for `git gc`, `git fetch`, `git clone`, and `git log` over packed repositories.

## How

- **Varint (both size and OFS)**: 7 bits per byte, little-endian, high bit = continue — `size |= (byte & 0x7f) << (7 * i)` — implemented in `read_varint`
- **Delta format**: source size varint, target size varint, then opcodes:
  - byte < 0x80 = insert (size = byte)
  - byte >= 0x80 = copy with bits: 0x01=off byte 0, 0x02=off byte 1, 0x04=off byte 2, 0x08=off byte 3, 0x10=size byte 0, 0x20=size byte 1, 0x40=size byte 2; size bits absent → size 0x10000
  - implemented in `read_pack_object` and `apply_delta`
- **After resolution**: the resulting object must hash to its oid — verify once per resolved object — framework implemented in `PackFile::cache`
- **Caching**: `HashMap<oid, Object>` per command invocation, cleared between commands — implemented in `PackFile::cache`
- **`verify-pack`-style check command**: `git-rs verify-pack <pack>` — parses idx+pack, resolves all objects, reports errors — framework set up
- **Pack header**: magic `\377tOc`, version
- **Idx v2 parse**: fanout table, oid table, offsets table
- **Pack object lookup**: loose-first then pack lookup via `PackFile::resolve()`
- **OFS_DELTA/REF_DELTA resolution**: with opcode processing (insert/copy bitmask)

## Usage

```bash
# Verify a pack file
git-rs verify-pack <pack-file>

# Check pack integrity
# For `git log` over a repo that real `git gc` packed,
# our verify-pack agrees with real `git verify-pack`

# Check fsck status
git-rs fsck
```

**Verification**: `git log` over a repo that real `git gc` packed; our `verify-pack` agrees with real `git verify-pack`; `git fsck` clean.