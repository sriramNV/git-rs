//! Annotated tag objects: `object`, `type`, `tag`, `tagger` headers, blank
//! line, message (see rules.md).
//!
//! Format (locked): `object <sha>\ntype <type>\ntag <name>\ntagger
//! <ident>\n\n<message>`. Strict parse like commits.

use crate::error::{GitError, Result};
use crate::object::commit::Ident;
use crate::object::parse_oid_line;

/// A parsed annotated tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    /// Object the tag points at (raw oid).
    pub object: [u8; 20],
    /// Type of that object: `blob`, `tree`, `commit`, `tag`.
    pub obj_type: String,
    pub name: String,
    pub tagger: Ident,
    pub message: Vec<u8>,
}

impl Tag {
    /// Strict parse of tag content (header stripped). Header order is
    /// enforced: `object`, `type`, `tag`, `tagger`, blank, message.
    pub fn parse(content: &[u8]) -> Result<Tag> {
        let text = std::str::from_utf8(content)
            .map_err(|_| GitError::Corrupt("tag content not UTF-8".into()))?;
        let (headers, message) = text
            .split_once("\n\n")
            .ok_or_else(|| GitError::Corrupt("tag missing blank line before message".into()))?;
        let mut lines = headers.lines();
        let object_line = lines
            .next()
            .ok_or_else(|| GitError::Corrupt("tag missing object line".into()))?;
        let object = parse_oid_line("object", object_line)?;
        let type_line = lines
            .next()
            .ok_or_else(|| GitError::Corrupt("tag missing type line".into()))?;
        let obj_type = type_line
            .strip_prefix("type ")
            .ok_or_else(|| GitError::Corrupt("tag missing type line".into()))?
            .trim()
            .to_string();
        let tag_line = lines
            .next()
            .ok_or_else(|| GitError::Corrupt("tag missing tag line".into()))?;
        let name = tag_line
            .strip_prefix("tag ")
            .ok_or_else(|| GitError::Corrupt("tag missing tag line".into()))?
            .trim()
            .to_string();
        let tagger_line = lines
            .next()
            .ok_or_else(|| GitError::Corrupt("tag missing tagger line".into()))?;
        let tagger = Ident::parse(
            tagger_line
                .strip_prefix("tagger ")
                .ok_or_else(|| GitError::Corrupt("tag missing tagger line".into()))?,
        )?;
        if lines.next().is_some() {
            return Err(GitError::Corrupt("tag has unexpected header line".into()));
        }
        Ok(Tag {
            object,
            obj_type,
            name,
            tagger,
            message: message.as_bytes().to_vec(),
        })
    }

    /// Serialize in the locked header order, message bytes verbatim.
    pub fn serialize(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        out.extend_from_slice(b"object ");
        out.extend_from_slice(hex(&self.object).as_bytes());
        out.push(b'\n');
        out.extend_from_slice(b"type ");
        out.extend_from_slice(self.obj_type.as_bytes());
        out.push(b'\n');
        out.extend_from_slice(b"tag ");
        out.extend_from_slice(self.name.as_bytes());
        out.push(b'\n');
        out.extend_from_slice(b"tagger ");
        out.extend_from_slice(self.tagger.render().as_bytes());
        out.push(b'\n');
        out.push(b'\n');
        out.extend_from_slice(&self.message);
        Ok(out)
    }
}

fn hex(oid: &[u8; 20]) -> String {
    oid.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(first: u8) -> [u8; 20] {
        let mut o = [0u8; 20];
        o[0] = first;
        o
    }

    fn sample_tag() -> Tag {
        Tag {
            object: oid(7),
            obj_type: "commit".into(),
            name: "v1.0".into(),
            tagger: Ident::new("T Ag", "t@e.co", 123, -100).unwrap(),
            message: b"release notes\n".to_vec(),
        }
    }

    #[test]
    fn roundtrip() {
        let t = sample_tag();
        let bytes = t.serialize().unwrap();
        assert_eq!(Tag::parse(&bytes).unwrap(), t);
    }

    #[test]
    fn strict_header_order_is_enforced() {
        let bytes = sample_tag().serialize().unwrap();
        let text = String::from_utf8(bytes).unwrap();
        // Wrong first header.
        let swapped = text.replacen("object ", "tag ", 1);
        assert!(Tag::parse(swapped.as_bytes()).is_err());
        // Missing blank line.
        let no_blank = text.replace("\n\n", "\n");
        assert!(Tag::parse(no_blank.as_bytes()).is_err());
        // Extra header after tagger.
        let (heads, msg) = text.split_once("\n\n").unwrap();
        let extra = format!("{heads}\nfoo bar\n\n{msg}");
        assert!(Tag::parse(extra.as_bytes()).is_err());
    }

    #[test]
    fn truncated_tag_is_corrupt() {
        let bytes = sample_tag().serialize().unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let cut = text.find("tagger").unwrap();
        assert!(Tag::parse(&text.as_bytes()[..cut]).is_err());
    }
}
