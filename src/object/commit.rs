//! Commit objects: `tree`, `parent*`, `author`, `committer` headers, blank
//! line, message (see rules.md).
//!
//! Format (locked): strict parse — `tree` line first, then 0+ `parent`
//! lines, then `author`, then `committer`, then a blank line, then the
//! message. Anything else is `Corrupt`. Ident lines are
//! `<name> <<email>> <unix-ts> <tz>`; tz is `+HHMM`/`-HHMM` within
//! `-1200..=+1400` (git's bound).

use crate::error::{GitError, Result};
use crate::object::parse_oid_line;

/// An identity line in a commit or tag: `Name <email> <ts> <tz>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident {
    pub name: String,
    pub email: String,
    /// Unix timestamp in seconds.
    pub ts: i64,
    /// Timezone offset as signed HHMM, e.g. `530` or `-700`.
    pub tz: i32,
}

impl Ident {
    /// Build an identity for writing. Rejects timezone offsets outside
    /// git's valid range (`-1200..=+1400`) — real git refuses these.
    pub fn new(name: impl Into<String>, email: impl Into<String>, ts: i64, tz: i32) -> Result<Self> {
        if !(-1200..=1400).contains(&tz) {
            return Err(GitError::Invalid(format!(
                "invalid timezone offset {tz}: must be between -1200 and +1400"
            )));
        }
        Ok(Ident { name: name.into(), email: email.into(), ts, tz })
    }

    /// Parse an ident line (without the `author `/`committer `/`tagger `
    /// prefix). Same split rule as git: the last `<`, the `>` after it,
    /// then `<ts> <tz>`.
    pub fn parse(line: &str) -> Result<Ident> {
        let lt = line
            .rfind('<')
            .ok_or_else(|| GitError::Corrupt(format!("bad ident line '{line}'")))?;
        let gt = line[lt..]
            .find('>')
            .map(|p| lt + p)
            .ok_or_else(|| GitError::Corrupt(format!("bad ident line '{line}'")))?;
        let name = line[..lt].trim().to_string();
        let email = line[lt + 1..gt].to_string();
        let date = line[gt + 1..].trim();
        let (ts, tz) = date
            .split_once(' ')
            .ok_or_else(|| GitError::Corrupt(format!("bad ident date '{date}'")))?;
        let ts: i64 = ts
            .trim()
            .parse()
            .map_err(|_| GitError::Corrupt(format!("bad ident timestamp '{ts}'")))?;
        let tz = parse_tz(tz.trim())?;
        Ok(Ident { name, email, ts, tz })
    }

    /// Render `Name <email> <ts> <tz>` for serialization.
    pub fn render(&self) -> String {
        let sign = if self.tz < 0 { '-' } else { '+' };
        format!("{} <{}> {} {sign}{:04}", self.name, self.email, self.ts, self.tz.abs())
    }
}

/// Parse a signed HHMM timezone (`+0530`, `-0700`) into signed minutes-as-
/// HHMM (`530`, `-700`), validating git's `-1200..=+1400` range.
fn parse_tz(s: &str) -> Result<i32> {
    let (sign, digits) = match s.strip_prefix('-') {
        Some(d) => (-1, d),
        None => match s.strip_prefix('+') {
            Some(d) => (1, d),
            None => return Err(GitError::Corrupt(format!("bad ident timezone '{s}'"))),
        },
    };
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(GitError::Corrupt(format!("bad ident timezone '{s}'")));
    }
    let value: i32 = digits
        .parse()
        .map_err(|_| GitError::Corrupt(format!("bad ident timezone '{s}'")))?;
    let tz = sign * value;
    if !(-1200..=1400).contains(&tz) {
        return Err(GitError::Corrupt(format!(
            "bad ident timezone '{s}': must be between -1200 and +1400"
        )));
    }
    Ok(tz)
}

/// A parsed commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub tree: [u8; 20],
    pub parents: Vec<[u8; 20]>,
    pub author: Ident,
    pub committer: Ident,
    pub message: Vec<u8>,
}

impl Commit {
    /// Strict parse of commit content (header stripped). Header order is
    /// enforced: tree, parents, author, committer, blank, message — any
    /// other line is `Corrupt` (real git also rejects unknown headers).
    pub fn parse(content: &[u8]) -> Result<Commit> {
        let text = std::str::from_utf8(content)
            .map_err(|_| GitError::Corrupt("commit content not UTF-8".into()))?;
        let (headers, message) = text
            .split_once("\n\n")
            .ok_or_else(|| GitError::Corrupt("commit missing blank line before message".into()))?;
        let mut lines = headers.lines();

        let tree_line = lines
            .next()
            .ok_or_else(|| GitError::Corrupt("commit missing tree line".into()))?;
        let tree = parse_oid_line("tree", tree_line)?;

        let mut parents = Vec::new();
        loop {
            match lines.next() {
                Some(l) if l.starts_with("parent ") => parents.push(parse_oid_line("parent", l)?),
                Some(l) if l.starts_with("author ") => {
                    let author = Ident::parse(&l[7..])?;
                    let committer_line = lines
                        .next()
                        .ok_or_else(|| GitError::Corrupt("commit missing committer line".into()))?;
                    let committer_line = committer_line
                        .strip_prefix("committer ")
                        .ok_or_else(|| GitError::Corrupt("commit missing committer line".into()))?;
                    let committer = Ident::parse(committer_line)?;
                    if lines.next().is_some() {
                        return Err(GitError::Corrupt("commit has unexpected header line".into()));
                    }
                    return Ok(Commit { tree, parents, author, committer, message: message.as_bytes().to_vec() });
                }
                Some(_) => return Err(GitError::Corrupt("commit has unexpected header line".into())),
                None => return Err(GitError::Corrupt("commit missing author/committer".into())),
            }
        }
    }

    /// Serialize: `tree`, `parent*`, `author`, `committer`, blank line,
    /// message (message bytes verbatim).
    pub fn serialize(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        out.extend_from_slice(b"tree ");
        out.extend_from_slice(hex(&self.tree).as_bytes());
        out.push(b'\n');
        for p in &self.parents {
            out.extend_from_slice(b"parent ");
            out.extend_from_slice(hex(p).as_bytes());
            out.push(b'\n');
        }
        out.extend_from_slice(b"author ");
        out.extend_from_slice(self.author.render().as_bytes());
        out.push(b'\n');
        out.extend_from_slice(b"committer ");
        out.extend_from_slice(self.committer.render().as_bytes());
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

    fn ident(name: &str, email: &str, ts: i64, tz: i32) -> Ident {
        Ident::new(name, email, ts, tz).unwrap()
    }

    fn sample_commit() -> Commit {
        Commit {
            tree: oid(1),
            parents: vec![oid(2), oid(3)],
            author: ident("A U Thor", "a@example.com", 1700000000, 530),
            committer: ident("C O Mitter", "c@example.com", 1700000001, -700),
            message: b"subject\n\nbody\n".to_vec(),
        }
    }

    #[test]
    fn roundtrip_with_parents_and_multiline_message() {
        let c = sample_commit();
        let bytes = c.serialize().unwrap();
        let parsed = Commit::parse(&bytes).unwrap();
        assert_eq!(parsed, c);
    }

    #[test]
    fn roundtrip_no_parents_empty_message() {
        let c = Commit {
            tree: oid(9),
            parents: vec![],
            author: ident("Only Author", "a@e.co", 0, 0),
            committer: ident("Only Author", "a@e.co", 0, 0),
            message: Vec::new(),
        };
        let bytes = c.serialize().unwrap();
        assert_eq!(Commit::parse(&bytes).unwrap(), c);
    }

    #[test]
    fn message_with_blank_lines_survives() {
        let c = Commit {
            tree: oid(4),
            parents: vec![],
            author: ident("A", "a@e.co", 1, 0),
            committer: ident("A", "a@e.co", 1, 0),
            message: b"first\n\nsecond\n\nthird\n".to_vec(),
        };
        let bytes = c.serialize().unwrap();
        assert_eq!(Commit::parse(&bytes).unwrap().message, c.message);
    }

    #[test]
    fn strict_header_order_is_enforced() {
        // tree must come first.
        let mut c = sample_commit();
        c.parents.clear();
        let mut text = String::from_utf8(c.serialize().unwrap()).unwrap();
        assert!(Commit::parse(text.as_bytes()).is_ok());
        // Unknown header between tree and author → corrupt.
        text = text.replace("tree ", "foo ");
        assert!(Commit::parse(text.as_bytes()).is_err());
        // Unknown header between committer and blank line → corrupt.
        let mut c = sample_commit();
        c.parents.clear();
        let text = String::from_utf8(c.serialize().unwrap()).unwrap();
        let mut parts = text.splitn(2, "\n\n").collect::<Vec<_>>();
        let with_extra = format!("{}\ngpgsig BAD", parts[0]);
        parts[0] = &with_extra;
        assert!(Commit::parse(format!("{}\n\n{}", parts[0], parts[1]).as_bytes()).is_err());
    }

    #[test]
    fn missing_pieces_are_corrupt() {
        let c = sample_commit();
        let bytes = c.serialize().unwrap();
        // No blank line before message.
        let mut text = String::from_utf8(bytes.clone()).unwrap();
        text = text.replace("\n\n", "\n");
        assert!(Commit::parse(text.as_bytes()).is_err());
        // Truncated: no committer.
        let text = String::from_utf8(bytes).unwrap();
        let cut = text.find("committer").unwrap();
        assert!(Commit::parse(text[..cut].as_bytes()).is_err());
    }

    #[test]
    fn bad_dates_are_rejected() {
        // Out-of-range tz: reject on write (Invalid) and on read (Corrupt).
        assert!(Ident::new("A", "a@e.co", 1, 1401).is_err());
        assert!(Ident::new("A", "a@e.co", 1, -1201).is_err());
        assert!(Ident::new("A", "a@e.co", 1, -1200).is_ok());
        assert!(Ident::new("A", "a@e.co", 1, 1400).is_ok());
        assert!(Ident::parse("A <a@e.co> 1 +2500").is_err());
        assert!(Ident::parse("A <a@e.co> 1 +05").is_err());
        assert!(Ident::parse("A <a@e.co> 1 +05a0").is_err());
        assert!(Ident::parse("A <a@e.co> 1 05").is_err());
        assert!(Ident::parse("A <a@e.co> abc +0530").is_err());
        assert!(Ident::parse("A <a@e.co> 1").is_err());
        // No angle brackets at all.
        assert!(Ident::parse("A a@e.co 1 +0530").is_err());
    }

    #[test]
    fn ident_parse_matches_git_split_rule() {
        // git splits at the LAST '<' — a name containing '<' is fine.
        let i = Ident::parse("A < B <c@d.co> 42 +0530").unwrap();
        assert_eq!(i.name, "A < B");
        assert_eq!(i.email, "c@d.co");
        assert_eq!(i.ts, 42);
        assert_eq!(i.tz, 530);
        // Negative tz.
        let i = Ident::parse("X <x@y.z> 7 -0700").unwrap();
        assert_eq!(i.tz, -700);
    }

    #[test]
    fn render_matches_git_ident_format() {
        assert_eq!(ident("A U Thor", "a@e.co", 1700000000, 530).render(), "A U Thor <a@e.co> 1700000000 +0530");
        assert_eq!(ident("A", "a@e.co", 1, -700).render(), "A <a@e.co> 1 -0700");
        assert_eq!(ident("A", "a@e.co", 1, 0).render(), "A <a@e.co> 1 +0000");
    }
}
