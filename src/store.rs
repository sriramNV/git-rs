//! Loose object storage: hashing, read, and write.
//!
//! Format (locked): loose object = zlib stream of `header + content`, where
//! header is exactly `<type> <decimal-size>\0`. The object id is the sha1 of
//! `header + content`, lowercase hex, stored at `objects/<2>/<38>`.

use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use sha1::{Digest, Sha1};

use crate::error::{GitError, IoContext, Result};

/// The four git object types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Blob,
    Tree,
    Commit,
    Tag,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Blob => "blob",
            Kind::Tree => "tree",
            Kind::Commit => "commit",
            Kind::Tag => "tag",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "blob" => Some(Kind::Blob),
            "tree" => Some(Kind::Tree),
            "commit" => Some(Kind::Commit),
            "tag" => Some(Kind::Tag),
            _ => None,
        }
    }
}

/// Loose object store rooted at an objects directory.
///
/// Everything that touches `.git/objects` goes through this type.
pub struct ObjectStore {
    root: PathBuf,
}

impl ObjectStore {
    /// Store rooted at an explicit objects directory (tests use this).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        ObjectStore { root: root.into() }
    }

    /// Resolve the objects directory: `GIT_OBJECT_DIRECTORY` env, else
    /// `GIT_DIR` env + `objects`, else `<cwd>/.git/objects`.
    ///
    /// ponytail: no upward-directory walk in v1 (see decisions.md D-002);
    /// commands must run from the repo root or set GIT_DIR.
    pub fn discover() -> Result<Self> {
        let root = match env::var("GIT_OBJECT_DIRECTORY") {
            Ok(dir) => PathBuf::from(dir),
            Err(_) => {
                let git_dir = match env::var("GIT_DIR") {
                    Ok(dir) => PathBuf::from(dir),
                    Err(_) => PathBuf::from(".git"),
                };
                git_dir.join("objects")
            }
        };
        Ok(ObjectStore { root })
    }

    /// Compute the object id for a kind + content: sha1 of `header + content`.
    pub fn hash(kind: Kind, content: &[u8]) -> String {
        let mut hasher = Sha1::new();
        hasher.update(kind.as_str().as_bytes());
        hasher.update(b" ");
        hasher.update(content.len().to_string().as_bytes());
        hasher.update(b"\0");
        hasher.update(content);
        let digest = hasher.finalize();
        digest.iter().map(|b| format!("{b:02x}")).collect::<String>()
    }

    /// The on-disk path for an object id: `objects/<2>/<38>`.
    pub fn object_path(&self, id: &str) -> PathBuf {
        self.root.join(&id[..2]).join(&id[2..])
    }

    /// Write a blob object, returning its id. Idempotent: an already-present
    /// object is not rewritten.
    pub fn write_blob(&self, content: &[u8]) -> Result<String> {
        self.write_object(Kind::Blob, content)
    }

    /// Write a loose object, returning its id. Writes go through a temp file
    /// plus rename so a crash never leaves a half-written object.
    pub fn write_object(&self, kind: Kind, content: &[u8]) -> Result<String> {
        let id = Self::hash(kind, content);
        let path = self.object_path(&id);
        if path.exists() {
            return Ok(id);
        }

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(kind.as_str().as_bytes())
            .context(&path, "encode object header")?;
        encoder.write_all(b" ").context(&path, "encode object header")?;
        encoder
            .write_all(format!("{}", content.len()).as_bytes())
            .context(&path, "encode object header")?;
        encoder.write_all(b"\0").context(&path, "encode object header")?;
        encoder
            .write_all(content)
            .context(&path, "encode object content")?;
        let compressed = encoder.finish().context(&path, "finish zlib stream")?;

        let dir = path
            .parent()
            .ok_or_else(|| GitError::Invalid(format!("object path {id} has no parent")))?;
        fs::create_dir_all(dir).context(dir, "create object directory")?;
        let tmp = dir.join(format!(".tmp-{}", std::process::id()));
        fs::write(&tmp, &compressed).context(&tmp, "write object")?;
        fs::rename(&tmp, &path).context(&path, "commit object")?;
        Ok(id)
    }

    /// Read a loose object, verifying type, size, and hash integrity.
    ///
    /// Returns `(kind, content)` where content is exactly the object body
    /// (header stripped). Any format or integrity violation is `Corrupt`.
    pub fn read_object(&self, id: &str) -> Result<(Kind, Vec<u8>)> {
        if !is_valid_id(id) || !id.chars().all(|c| c.is_ascii_hexdigit())
            || id.chars().any(|c| c.is_ascii_uppercase())
        {
            // Message matches real git's fatal for a bad object name;
            // NotFound exits 128 like real git's fatal.
            return Err(GitError::NotFound(format!("Not a valid object name {id}")));
        }
        let path = self.object_path(id);
        let compressed = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(GitError::NotFound(format!("Not a valid object name {id}")));
            }
            Err(e) => {
                return Err(GitError::io(path.display().to_string(), "read object", e))
            }
        };

        let mut decoder = ZlibDecoder::new(&compressed[..]);
        let mut buf = Vec::new();
        decoder
            .read_to_end(&mut buf)
            .context(&path, "decompress object")?;
        if decoder.total_in() != compressed.len() as u64 {
            return Err(GitError::Corrupt(format!(
                "object {id} has trailing garbage after zlib stream"
            )));
        }

        let nul = buf
            .iter()
            .position(|&b| b == b'\0')
            .ok_or_else(|| GitError::Corrupt(format!("object {id} header missing NUL")))?;
        let header = std::str::from_utf8(&buf[..nul])
            .map_err(|_| GitError::Corrupt(format!("object {id} header not ASCII")))?;
        let (kind_str, size_str) = header
            .split_once(' ')
            .ok_or_else(|| GitError::Corrupt(format!("object {id} header missing size")))?;
        let kind = Kind::parse(kind_str)
            .ok_or_else(|| GitError::Corrupt(format!("object {id} has bad type '{kind_str}'")))?;
        let size: usize = size_str
            .parse()
            .map_err(|_| GitError::Corrupt(format!("object {id} has bad size '{size_str}'")))?;

        let content = &buf[nul + 1..];
        if content.len() != size {
            return Err(GitError::Corrupt(format!(
                "object {id} declares size {size} but has {} bytes",
                content.len()
            )));
        }
        if Self::hash(kind, content) != id {
            return Err(GitError::Corrupt(format!("object {id} hash mismatch")));
        }
        Ok((kind, content.to_vec()))
    }
}

/// A valid loose-object id: 40 lowercase hex characters.
fn is_valid_id(id: &str) -> bool {
    id.len() == 40 && id.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_store() -> ObjectStore {
        let dir = env::temp_dir().join(format!(
            "git-rs-store-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        ObjectStore::new(dir)
    }

    #[test]
    fn hash_matches_known_git_blob_ids() {
        // Known git object ids: "hello world\n" blob and the empty blob.
        assert_eq!(
            ObjectStore::hash(Kind::Blob, b"hello world\n"),
            "3b18e512dba79e4c8300dd08aeb37f8e728b8dad"
        );
        assert_eq!(
            ObjectStore::hash(Kind::Blob, b""),
            "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"
        );
    }

    #[test]
    fn write_then_read_roundtrips() {
        let store = temp_store();
        let id = store.write_blob(b"the quick brown fox").unwrap();
        assert_eq!(
            id,
            ObjectStore::hash(Kind::Blob, b"the quick brown fox")
        );
        let (kind, content) = store.read_object(&id).unwrap();
        assert_eq!(kind, Kind::Blob);
        assert_eq!(content, b"the quick brown fox");
        let _ = fs::remove_dir_all(store.root);
    }

    #[test]
    fn empty_blob_roundtrips() {
        let store = temp_store();
        let id = store.write_blob(b"").unwrap();
        assert_eq!(id, "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
        let (_, content) = store.read_object(&id).unwrap();
        assert!(content.is_empty());
        let _ = fs::remove_dir_all(store.root);
    }

    #[test]
    fn write_is_idempotent() {
        let store = temp_store();
        let a = store.write_blob(b"same").unwrap();
        let b = store.write_blob(b"same").unwrap();
        assert_eq!(a, b);
        assert!(store.object_path(&a).exists());
        let _ = fs::remove_dir_all(store.root);
    }

    #[test]
    fn content_mismatch_is_detected() {
        let store = temp_store();
        let id = store.write_blob(b"integrity").unwrap();
        // Re-encode a *different* payload under the original object id's
        // header: header parses fine, size matches, but the hash must not.
        let path = store.object_path(&id);
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"blob 9\0integrity!").unwrap();
        let bytes = encoder.finish().unwrap();
        fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            store.read_object(&id),
            Err(GitError::Corrupt(_))
        ));
        let _ = fs::remove_dir_all(store.root);
    }

    #[test]
    fn truncated_object_is_detected() {
        let store = temp_store();
        let id = store.write_blob(b"truncated content").unwrap();
        let path = store.object_path(&id);
        let bytes = fs::read(&path).unwrap();
        fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        assert!(matches!(
            store.read_object(&id),
            Err(GitError::Corrupt(_))
        ));
        let _ = fs::remove_dir_all(store.root);
    }

    #[test]
    fn bad_object_names_are_rejected() {
        let store = temp_store();
        assert!(matches!(
            store.read_object("nothex"),
            Err(GitError::NotFound(_))
        ));
        assert!(matches!(
            store.read_object("zzz"),
            Err(GitError::NotFound(_))
        ));
        let _ = fs::remove_dir_all(store.root);
    }
}