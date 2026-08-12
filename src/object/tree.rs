//! Tree objects: entries of `<mode> <name>\0<20-byte oid>`, serialized in
//! git's `base_name_compare` order (see rules.md).
//!
//! Format (locked): an entry is the octal file mode (5-6 digits, no leading
//! zero padding beyond the leading `0` of `040000`), a space, the raw name
//! bytes, a NUL, then 20 raw oid bytes. Entries appear back-to-back; an
//! empty tree is valid.

use std::cmp::Ordering;

use crate::error::{GitError, Result};

/// One tree entry: file mode, raw name bytes, 20-byte raw object id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub mode: u32,
    pub name: Vec<u8>,
    pub oid: [u8; 20],
}

impl TreeEntry {
    /// Git's `S_ISDIR`: the directory bit (0o040000) is set.
    pub fn is_dir(&self) -> bool {
        self.mode & 0o040000 != 0
    }
}

/// A parsed tree: entries in stored order. Parsing does not re-sort; the
/// stored order is trusted (real git also trusts it on read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    pub entries: Vec<TreeEntry>,
}

impl Tree {
    /// Parse raw tree content (header stripped). Any malformed entry is
    /// `Corrupt`: bad mode, empty name, name containing `/`, truncated oid,
    /// or trailing garbage.
    pub fn parse(content: &[u8]) -> Result<Tree> {
        let mut entries = Vec::new();
        let mut rest = content;
        while !rest.is_empty() {
            let sp = rest
                .iter()
                .position(|&b| b == b' ')
                .ok_or_else(|| GitError::Corrupt("tree entry missing mode separator".into()))?;
            let mode = parse_mode(&rest[..sp])?;

            let name_start = sp + 1;
            let nul = rest[name_start..]
                .iter()
                .position(|&b| b == b'\0')
                .map(|p| name_start + p)
                .ok_or_else(|| GitError::Corrupt("tree entry name missing NUL terminator".into()))?;
            let name = &rest[name_start..nul];
            if name.is_empty() {
                return Err(GitError::Corrupt("tree entry has empty name".into()));
            }
            if name.contains(&b'/') {
                return Err(GitError::Corrupt(format!(
                    "tree entry name '{}' contains '/'",
                    String::from_utf8_lossy(name)
                )));
            }

            let oid_start = nul + 1;
            let oid_end = oid_start + 20;
            let oid_bytes = rest
                .get(oid_start..oid_end)
                .ok_or_else(|| GitError::Corrupt("tree entry oid truncated".into()))?;
            let mut oid = [0u8; 20];
            oid.copy_from_slice(oid_bytes);

            entries.push(TreeEntry { mode, name: name.to_vec(), oid });
            rest = &rest[oid_end..];
        }
        Ok(Tree { entries })
    }

    /// Serialize entries in git's sort order (`base_name_compare`), modes
    /// as octal, oid as 20 raw bytes. The result must hash to the same id
    /// real git produces for the same entries.
    pub fn serialize(&self) -> Result<Vec<u8>> {
        let mut entries = self.entries.clone();
        entries.sort_by(|a, b| base_name_compare(&a.name, a.is_dir(), &b.name, b.is_dir()));
        let mut out = Vec::new();
        for e in &entries {
            out.extend_from_slice(format!("{:o}", e.mode).as_bytes());
            out.push(b' ');
            out.extend_from_slice(&e.name);
            out.push(b'\0');
            out.extend_from_slice(&e.oid);
        }
        Ok(out)
    }
}

/// Git's `base_name_compare`: bytewise name comparison, but a tree entry's
/// name is compared as if it had a trailing `/` appended; when names are
/// otherwise equal the directory flag is the tiebreaker (dirs sort after
/// plain files). A wrong sort produces wrong-but-fsck-clean trees that
/// differ from real git (see rules.md).
fn base_name_compare(a: &[u8], a_dir: bool, b: &[u8], b_dir: bool) -> Ordering {
    let len = a.len().min(b.len());
    match a[..len].cmp(&b[..len]) {
        Ordering::Equal => {}
        o => return o,
    }
    let ca = a.get(len).copied().unwrap_or(if a_dir { b'/' } else { 0 });
    let cb = b.get(len).copied().unwrap_or(if b_dir { b'/' } else { 0 });
    ca.cmp(&cb)
}

/// Parse an octal mode (5-6 digits, all `0-7`).
fn parse_mode(s: &[u8]) -> Result<u32> {
    if s.is_empty() || s.len() > 6 || !s.iter().all(|b| b.is_ascii_digit() && *b < b'8') {
        return Err(GitError::Corrupt(format!(
            "tree entry has bad mode '{}'",
            String::from_utf8_lossy(s)
        )));
    }
    let text = std::str::from_utf8(s)
        .map_err(|_| GitError::Corrupt("tree entry mode not ASCII".into()))?;
    u32::from_str_radix(text, 8)
        .map_err(|_| GitError::Corrupt(format!("tree entry has bad mode '{text}'")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(mode: u32, name: &str, first_byte: u8) -> TreeEntry {
        let mut oid = [0u8; 20];
        oid[0] = first_byte;
        TreeEntry { mode, name: name.as_bytes().to_vec(), oid }
    }

    fn tree(entries: Vec<TreeEntry>) -> Tree {
        Tree { entries }
    }

    #[test]
    fn base_name_compare_sorts_like_git() {
        // Equal names: the file sorts before the dir.
        assert_eq!(base_name_compare(b"a", false, b"a", true), Ordering::Less);
        assert_eq!(base_name_compare(b"a", true, b"a", false), Ordering::Greater);
        // "a" vs "a.txt": "a" (file, ends NUL) < "a.txt" ('.' == 0x2e).
        assert_eq!(base_name_compare(b"a", false, b"a.txt", false), Ordering::Less);
        // "a" dir behaves as "a/": '/' (0x2f) > '.' → "a.txt" sorts first.
        assert_eq!(base_name_compare(b"a", true, b"a.txt", false), Ordering::Greater);
        // Prefix: "a" < "ab".
        assert_eq!(base_name_compare(b"a", false, b"ab", false), Ordering::Less);
        // Identical: equal.
        assert_eq!(base_name_compare(b"a", false, b"a", false), Ordering::Equal);
    }

    #[test]
    fn serialize_sorts_entries() {
        // git order: "a.txt" (file), then "a" dir (as "a/"), then "b".
        let t = tree(vec![
            entry(0o100644, "b", 1),
            entry(0o040000, "a", 2),
            entry(0o100644, "a.txt", 3),
        ]);
        let bytes = t.serialize().unwrap();
        let mut oid1 = [0u8; 20];
        oid1[0] = 1;
        let mut oid2 = [0u8; 20];
        oid2[0] = 2;
        let mut oid3 = [0u8; 20];
        oid3[0] = 3;
        let mut expected = Vec::new();
        expected.extend_from_slice(b"100644 a.txt\0");
        expected.extend_from_slice(&oid3);
        expected.extend_from_slice(b"40000 a\0");
        expected.extend_from_slice(&oid2);
        expected.extend_from_slice(b"100644 b\0");
        expected.extend_from_slice(&oid1);
        assert_eq!(bytes, expected);
    }

    #[test]
    fn parse_roundtrips_serialize() {
        let t = tree(vec![
            entry(0o100755, "run.sh", 1),
            entry(0o120000, "link", 2),
            entry(0o040000, "sub", 3),
        ]);
        let bytes = t.serialize().unwrap();
        let parsed = Tree::parse(&bytes).unwrap();
        assert_eq!(parsed.entries.len(), 3);
        assert_eq!(parsed.entries[0].name, b"link");
        assert_eq!(parsed.entries[0].mode, 0o120000);
        assert_eq!(parsed.entries[1].name, b"run.sh");
        assert_eq!(parsed.entries[1].mode, 0o100755);
        assert_eq!(parsed.entries[2].name, b"sub");
        assert_eq!(parsed.entries[2].mode, 0o040000);
        assert_eq!(parsed.serialize().unwrap(), bytes);
    }

    #[test]
    fn empty_tree_is_valid() {
        let t = tree(vec![]);
        assert!(t.serialize().unwrap().is_empty());
        assert_eq!(Tree::parse(b"").unwrap().entries.len(), 0);
    }

    #[test]
    fn bad_modes_are_rejected() {
        // Missing separator entirely.
        assert!(Tree::parse(b"100644").is_err());
        // Non-octal digit.
        assert!(Tree::parse(b"100649 name\0\x01").is_err());
        // Too long.
        assert!(Tree::parse(b"1006440 name\0\x01").is_err());
        // Empty mode.
        assert!(Tree::parse(b" name\0\x01").is_err());
    }

    #[test]
    fn bad_names_are_rejected() {
        // Empty name.
        assert!(Tree::parse(b"100644 \0\x01").is_err());
        // Slash in name.
        assert!(Tree::parse(b"100644 a/b\0\x01").is_err());
        // Missing NUL terminator.
        assert!(Tree::parse(b"100644 name").is_err());
    }

    #[test]
    fn truncated_oid_is_rejected() {
        // Only 3 of 20 oid bytes.
        assert!(Tree::parse(b"100644 name\0\x01\x02\x03").is_err());
    }
}
