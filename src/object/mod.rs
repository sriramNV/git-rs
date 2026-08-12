//! Object types: dispatch between blob, tree, commit, and tag bodies.

pub mod commit;
pub mod tag;
pub mod tree;

pub use commit::{Commit, Ident};
pub use tag::Tag;
pub use tree::{Tree, TreeEntry};

use crate::error::{GitError, Result};
use crate::store::Kind;

/// A parsed object body (header stripped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Object {
    Blob(Vec<u8>),
    Tree(Tree),
    Commit(Commit),
    Tag(Tag),
}

impl Object {
    /// Parse raw object content according to its kind.
    pub fn parse(kind: Kind, content: &[u8]) -> Result<Object> {
        match kind {
            Kind::Blob => Ok(Object::Blob(content.to_vec())),
            Kind::Tree => Ok(Object::Tree(Tree::parse(content)?)),
            Kind::Commit => Ok(Object::Commit(Commit::parse(content)?)),
            Kind::Tag => Ok(Object::Tag(Tag::parse(content)?)),
        }
    }

    /// Serialize back to raw content. Blobs are verbatim; trees/commits/tags
    /// are re-encoded (trees get sorted, which must be a no-op for valid
    /// trees — git stores them pre-sorted).
    pub fn serialize(&self) -> Result<Vec<u8>> {
        match self {
            Object::Blob(b) => Ok(b.clone()),
            Object::Tree(t) => t.serialize(),
            Object::Commit(c) => c.serialize(),
            Object::Tag(t) => t.serialize(),
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            Object::Blob(_) => Kind::Blob,
            Object::Tree(_) => Kind::Tree,
            Object::Commit(_) => Kind::Commit,
            Object::Tag(_) => Kind::Tag,
        }
    }
}

/// Parse a 40-hex oid from a header line like `tree <oid>`, validating the
/// prefix. The oid must be 40 lowercase/uppercase hex digits.
pub(crate) fn parse_oid_line(what: &str, line: &str) -> Result<[u8; 20]> {
    let hex = line
        .strip_prefix(&format!("{what} "))
        .ok_or_else(|| GitError::Corrupt(format!("line missing '{what} ' prefix")))?;
    if hex.len() != 40 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(GitError::Corrupt(format!("bad {what} oid '{hex}'")));
    }
    let mut oid = [0u8; 20];
    for i in 0..20 {
        oid[i] = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)
            .map_err(|_| GitError::Corrupt(format!("bad {what} oid '{hex}'")))?;
    }
    Ok(oid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_oid_line_validates() {
        let ok = parse_oid_line("tree", &format!("tree {}", "ab".repeat(20))).unwrap();
        assert_eq!(ok, [0xab; 20]);
        assert!(parse_oid_line("tree", "tree abc").is_err());
        let nothex = "tree nothex".repeat(2);
        assert!(parse_oid_line("tree", &nothex).is_err());
        assert!(parse_oid_line("tree", "no prefix here").is_err());
    }

    #[test]
    fn blob_roundtrip_is_verbatim() {
        let o = Object::parse(Kind::Blob, b"\x00\xffhi").unwrap();
        assert_eq!(o.serialize().unwrap(), b"\x00\xffhi");
        assert_eq!(o.kind(), Kind::Blob);
    }
}