# Library Docs

Project-specific usage patterns for the two approved crates. Read AGENTS.md first, then this file, then general knowledge.

## `sha1` (hashing)

Object IDs are `sha1(header + content)`:

```rust
use sha1::{Digest, Sha1};

fn object_id(kind: &str, content: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(kind.as_bytes());
    hasher.update(b" ");
    hasher.update(content.len().to_string().as_bytes());
    hasher.update(b"\0");
    hasher.update(content);
    hex(&hasher.finalize())  // 40 lowercase hex chars
}
```

**Rules:**
- Hash input is always the header prefix + content — never content alone
- Output written as lowercase hex, exactly 40 chars
- The same hash feeds both the object id AND the loose-object filename

## `flate2` (compression)

Two distinct modes — never mix them up:

### zlib (loose objects) — `ZlibEncoder` / `ZlibDecoder`

```rust
use flate2::write::ZlibEncoder;
use flate2::Compression;

// write a loose blob
let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
encoder.write_all(&header)?;
encoder.write_all(content)?;
let compressed = encoder.finish()?;
// compressed → .git/objects/xx/38hex
```

**Rules:**
- Loose objects MUST be zlib-wrapped (`ZlibEncoder`). Raw deflate is corrupt output.
- On read: decompress fully; trailing bytes after the object body = corruption (`Corrupt` error)
- flate2 does not reliably surface zlib adler32 mismatches on read — do not rely on it. The object's sha1 re-hash in `store.rs` is the integrity gate (matching real git, which ignores adler32 and checks the content hash)

### raw deflate (packfiles) — `DeflateEncoder` / `DeflateDecoder`

```rust
use flate2::write::DeflateEncoder;

// pack entry data is raw deflate, NOT zlib
let mut e = DeflateEncoder::new(Vec::new(), Compression::default());
e.write_all(&entry_data)?;
let packed = e.finish()?;
```

**Rules:**
- Pack entries are raw deflate streams (no zlib wrapper, no adler32)
- Pack trailer and idx checksums are SHA-1 of accumulated bytes — computed manually, not by flate2

## Patterns

- Compression level: `Compression::default()` everywhere; size over speed
- Read path mirrors write path: decode with matching decoder, verify object hash after decompress — a valid object must always match its filename-derived id
- All format code uses byte slices; no `String` for binary content