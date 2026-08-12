//! The index (`.git/index`): stage-aware view of the working tree.
//!
//! Format (locked, see tracker 06): header `DIRC` + version `2` + entry
//! count; entries of 62 fixed bytes + NUL-terminated path, each padded with
//! NULs to an 8-byte-aligned offset; optional extensions (`<4-byte sig> +
//! <4-byte len> + data`); a trailing sha1 of all preceding bytes.
//!
//! Untouched entries round-trip verbatim: stat fields, raw path bytes, and
//! flag bits we don't interpret are preserved exactly.

use std::fs;
use std::io::Write;
use std::path::Path;

use sha1::{Digest, Sha1};

use crate::error::{GitError, IoContext, Result};

/// Number of fixed bytes of an entry before the NUL-terminated path.
const FIXED: usize = 62;

/// Bit 15: entry assumed unchanged, skip stat check (read for documentation
/// and tests only — v1 never writes it).
#[allow(dead_code)]
const FLAG_ASSUME_VALID: u16 = 1 << 15;
/// Bit 14: a 2-byte extended-flags field follows the fixed part, before the
/// name (git's `ondisk_cache_entry_extended`). Rare in v2 files (git only
/// *writes* extended entries for v3+, but reads them in any version).
const FLAG_EXTENDED: u16 = 1 << 14;
/// Bits 12-13: stage (0 normal, 1-3 merge slots).
const FLAG_STAGE_MASK: u16 = 0x3000;
/// Bits 0-11: path length, advisory on read; 0x0FFF when longer.
const FLAG_NAMEMASK: u16 = 0x0FFF;

/// One index entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub ctime_sec: i32,
    pub ctime_nsec: i32,
    pub mtime_sec: i32,
    pub mtime_nsec: i32,
    pub dev: u32,
    pub ino: u32,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u32,
    pub oid: [u8; 20],
    pub flags: u16,
    /// The 2-byte extended-flags field, present only when `flags` bit 14 is
    /// set; preserved verbatim on round-trip (v1 never sets its bits).
    pub extended_flags: u16,
    /// Raw path bytes — never assumed UTF-8, slash separators, no leading `./`.
    pub path: Vec<u8>,
}

impl IndexEntry {
    /// Stage slot from flags bits 12-13 (0 = normal).
    pub fn stage(&self) -> u8 {
        ((self.flags & FLAG_STAGE_MASK) >> 12) as u8
    }

    /// Parse one entry from `data` at `pos`, returning the entry and the
    /// position just past its padding. The fixed part is 62 bytes, plus a
    /// 2-byte extended-flags field when bit 14 is set (before the name —
    /// git's `ondisk_cache_entry_extended`). NUL is the only terminator —
    /// the namelen bits are advisory (git writes 0x0FFF for long paths).
    fn parse(data: &[u8], pos: usize) -> Result<(IndexEntry, usize)> {
        if data.len() < pos + FIXED + 1 {
            return Err(GitError::Corrupt("index entry truncated".into()));
        }
        let f = &data[pos..pos + FIXED];
        let take = |off: usize, n: usize| {
            let mut b = [0u8; 4];
            b[..n].copy_from_slice(&f[off..off + n]);
            b
        };
        let u32at = |off: usize| {
            let b = take(off, 4);
            u32::from_be_bytes(b)
        };
        let ctime_sec = i32::from_be_bytes(take(0, 4));
        let ctime_nsec = i32::from_be_bytes(take(4, 4));
        let mtime_sec = i32::from_be_bytes(take(8, 4));
        let mtime_nsec = i32::from_be_bytes(take(12, 4));
        let dev = u32at(16);
        let ino = u32at(20);
        let mode = u32at(24);
        let uid = u32at(28);
        let gid = u32at(32);
        let size = u32at(36);
        let mut oid = [0u8; 20];
        oid.copy_from_slice(&f[40..60]);
        let flags = u16::from_be_bytes([f[60], f[61]]);
        // Optional extended-flags field, located before the name.
        let mut name_start = pos + FIXED;
        let extended_flags = if flags & FLAG_EXTENDED != 0 {
            if data.len() < pos + FIXED + 2 + 1 {
                return Err(GitError::Corrupt(
                    "index entry extended flags truncated".into(),
                ));
            }
            let ef = u16::from_be_bytes([data[name_start], data[name_start + 1]]);
            name_start += 2;
            ef
        } else {
            0
        };
        // Read path to NUL; the namelen bits are advisory.
        let mut end = name_start;
        while end < data.len() && data[end] != 0 {
            end += 1;
        }
        if end == data.len() {
            return Err(GitError::Corrupt("index entry name unterminated".into()));
        }
        let path = data[name_start..end].to_vec();
        let mut next = end + 1; // past the terminating NUL
        // Pad with NULs to an 8-byte boundary.
        next = (next + 7) & !7;
        if next > data.len() {
            return Err(GitError::Corrupt("index entry padding truncated".into()));
        }
        let entry = IndexEntry {
            ctime_sec,
            ctime_nsec,
            mtime_sec,
            mtime_nsec,
            dev,
            ino,
            mode,
            uid,
            gid,
            size,
            oid,
            flags,
            extended_flags,
            path,
        };
        Ok((entry, next))
    }

    /// Serialize fixed part + optional extended-flags field + NUL-terminated
    /// path, padded to 8 bytes. The namelen bits are recomputed from the
    /// actual path length — git does the same at write time; all other flag
    /// bits round-trip verbatim.
    fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(80);
        let push_i32 = |v: i32, out: &mut Vec<u8>| out.extend_from_slice(&v.to_be_bytes());
        push_i32(self.ctime_sec, &mut out);
        push_i32(self.ctime_nsec, &mut out);
        push_i32(self.mtime_sec, &mut out);
        push_i32(self.mtime_nsec, &mut out);
        out.extend_from_slice(&self.dev.to_be_bytes());
        out.extend_from_slice(&self.ino.to_be_bytes());
        out.extend_from_slice(&self.mode.to_be_bytes());
        out.extend_from_slice(&self.uid.to_be_bytes());
        out.extend_from_slice(&self.gid.to_be_bytes());
        out.extend_from_slice(&self.size.to_be_bytes());
        out.extend_from_slice(&self.oid);
        let flags = (self.flags & !FLAG_NAMEMASK) | namelen_flags(self.path.len());
        out.extend_from_slice(&flags.to_be_bytes());
        if flags & FLAG_EXTENDED != 0 {
            out.extend_from_slice(&self.extended_flags.to_be_bytes());
        }
        out.extend_from_slice(&self.path);
        out.push(0);
        while out.len() % 8 != 0 {
            out.push(0);
        }
        out
    }
}

/// The index as read from or to be written to `.git/index`.
#[derive(Debug)]
pub struct Index {
    entries: Vec<IndexEntry>,
}

impl Index {
    /// An empty index, version 2.
    pub fn new() -> Self {
        Index {
            entries: Vec::new(),
        }
    }

    pub fn entries(&self) -> &[IndexEntry] {
        &self.entries
    }

    pub fn entries_mut(&mut self) -> &mut [IndexEntry] {
        &mut self.entries
    }

    /// Read `.git/index` (or any path, for tests). Corrupt on any format
    /// violation or checksum mismatch; version != 2 is rejected with a clear
    /// message. Extensions are skipped by their length field — same as git's
    /// reader — never misparsed.
    pub fn read(path: &Path) -> Result<Index> {
        let data = fs::read(path).context(path, "read index")?;
        if data.len() < 12 {
            return Err(GitError::Corrupt("index file too short".into()));
        }
        let magic = &data[..4];
        let version = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let count = u32::from_be_bytes([data[8], data[9], data[10], data[11]]) as usize;
        if magic != b"DIRC" {
            return Err(GitError::Corrupt("bad index magic".into()));
        }
        if version != 2 {
            return Err(GitError::Corrupt(format!(
                "index file version {version} not supported (v1 supports version 2 only)"
            )));
        }
        let rest = &data[12..];
        let mut entries = Vec::with_capacity(count);
        let mut pos = 0usize;
        for _ in 0..count {
            let (entry, next) = IndexEntry::parse(rest, pos)
                .map_err(|e| GitError::Corrupt(format!("index entry {pos}: {e}")))?;
            entries.push(entry);
            pos = next;
        }
        // Extensions: `<4-byte sig> <4-byte len> <data>` until the 20-byte checksum.
        let mut offset = pos;
        while offset + 20 < rest.len() {
            let sig = &rest[offset..offset + 4];
            let len = u32::from_be_bytes([
                rest[offset + 4],
                rest[offset + 5],
                rest[offset + 6],
                rest[offset + 7],
            ]) as usize;
            if !sig.iter().all(|b| b.is_ascii_alphabetic()) {
                return Err(GitError::Corrupt(
                    "index extension signature invalid".into(),
                ));
            }
            offset += 8 + len;
            if offset > rest.len() {
                return Err(GitError::Corrupt("index extension overruns file".into()));
            }
        }
        if offset + 20 != rest.len() {
            return Err(GitError::Corrupt("index trailing data invalid".into()));
        }
        let want = &rest[offset..offset + 20];
        let mut hasher = Sha1::new();
        hasher.update(&data[..data.len() - 20]);
        let got = hasher.finalize();
        if want != got.as_slice() {
            return Err(GitError::Corrupt("index checksum mismatch".into()));
        }
        Ok(Index { entries })
    }

    /// Write to `path` atomically (temp + rename, like refs). Entries are
    /// sorted by path bytes then stage — git binary-searches its index, so
    /// unsorted input breaks real git.
    pub fn write(&self, path: &Path) -> Result<()> {
        let mut out = Vec::new();
        out.extend_from_slice(b"DIRC");
        out.extend_from_slice(&2u32.to_be_bytes());
        out.extend_from_slice(&(self.entries.len() as u32).to_be_bytes());
        let mut sorted = self.entries.clone();
        sorted.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.stage().cmp(&b.stage())));
        for e in &sorted {
            out.extend_from_slice(&e.serialize());
        }
        let mut hasher = Sha1::new();
        hasher.update(&out);
        out.extend_from_slice(&hasher.finalize());
        let dir = path
            .parent()
            .ok_or_else(|| {
                GitError::Corrupt(format!("index path '{}' has no parent", path.display()))
            })?
            .to_path_buf();
        fs::create_dir_all(&dir).context(&dir, "create index directory")?;
        let tmp = dir.join(format!(".tmp-index-{}", std::process::id()));
        let mut f = fs::File::create(&tmp).context(&tmp, "create temp index")?;
        f.write_all(&out).context(&tmp, "write temp index")?;
        f.sync_all().context(&tmp, "fsync temp index")?;
        fs::rename(&tmp, path).context(path, "commit index")?;
        Ok(())
    }

    /// Insert or replace the entry for `path` at `entry.stage` (stage helper
    /// for `add`).
    pub fn stage(&mut self, entry: IndexEntry) {
        let path = entry.path.clone();
        let stage = entry.stage();
        self.entries
            .retain(|e| e.path != path || e.stage() != stage);
        self.entries.push(entry);
    }

    /// Remove every stage of `path` (unstage helper for `reset`).
    pub fn unstage(&mut self, path: &[u8]) {
        self.entries.retain(|e| e.path != path);
    }
}

impl Default for Index {
    fn default() -> Self {
        Self::new()
    }
}

/// Git's advisory namelen bits for a path of `len` bytes.
fn namelen_flags(len: usize) -> u16 {
    (len.min(FLAG_NAMEMASK as usize)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "git-rs-index-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entry(path: &[u8], oid: [u8; 20], flags: u16) -> IndexEntry {
        IndexEntry {
            ctime_sec: 1,
            ctime_nsec: 2,
            mtime_sec: 3,
            mtime_nsec: 4,
            dev: 5,
            ino: 6,
            mode: 0o100644,
            uid: 7,
            gid: 8,
            size: 9,
            oid,
            flags,
            extended_flags: 0,
            path: path.to_vec(),
        }
    }

    fn blob_oid(n: u8) -> [u8; 20] {
        let mut oid = [0u8; 20];
        oid[0] = n;
        oid
    }

    #[test]
    fn round_trip_preserves_everything() {
        let dir = scratch();
        let p = dir.join("index");
        let mut idx = Index::new();
        let flags = FLAG_ASSUME_VALID | 0x2F; // unknown low bits preserved
        idx.stage(entry(b"a/b.txt", blob_oid(1), flags));
        idx.stage(entry(b"c.txt", blob_oid(2), 0));
        idx.write(&p).unwrap();
        let back = Index::read(&p).unwrap();
        let mut expected = idx.entries.clone();
        for e in &mut expected {
            e.flags = (e.flags & !FLAG_NAMEMASK) | namelen_flags(e.path.len());
        }
        assert_eq!(back.entries, expected);
        // Stage accessor sees flags bits 12-13.
        idx.stage(entry(b"conflict", blob_oid(3), FLAG_STAGE_MASK));
        idx.write(&p).unwrap();
        let back = Index::read(&p).unwrap();
        assert_eq!(back.entries.last().unwrap().stage(), 3);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sort_is_path_bytes_then_stage() {
        let dir = scratch();
        let p = dir.join("index");
        let mut idx = Index::new();
        idx.stage(entry(b"z", blob_oid(1), FLAG_STAGE_MASK));
        idx.stage(entry(b"a", blob_oid(2), FLAG_STAGE_MASK));
        idx.stage(entry(b"a", blob_oid(3), 0));
        idx.write(&p).unwrap();
        let back = Index::read(&p).unwrap();
        let paths: Vec<(String, u8)> = back
            .entries
            .iter()
            .map(|e| (String::from_utf8_lossy(&e.path).into_owned(), e.stage()))
            .collect();
        assert_eq!(
            paths,
            vec![
                ("a".to_string(), 0),
                ("a".to_string(), 3),
                ("z".to_string(), 3)
            ]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn checksum_mismatch_is_corrupt() {
        let dir = scratch();
        let p = dir.join("index");
        let mut idx = Index::new();
        idx.stage(entry(b"x", blob_oid(9), 0));
        idx.write(&p).unwrap();
        let mut data = fs::read(&p).unwrap();
        let n = data.len();
        data[n - 20] ^= 0xFF;
        fs::write(&p, data).unwrap();
        assert!(matches!(Index::read(&p), Err(GitError::Corrupt(_))));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn truncated_and_bad_magic_are_corrupt() {
        let dir = scratch();
        let p = dir.join("index");
        let mut idx = Index::new();
        idx.stage(entry(b"x", blob_oid(9), 0));
        idx.write(&p).unwrap();
        let data = fs::read(&p).unwrap();
        fs::write(&p, &data[..data.len() - 4]).unwrap();
        assert!(matches!(Index::read(&p), Err(GitError::Corrupt(_))));
        fs::write(&p, b"XXXX\x00\x00\x00\x02\x00\x00\x00\x00").unwrap();
        assert!(matches!(Index::read(&p), Err(GitError::Corrupt(_))));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn version_other_than_2_is_rejected() {
        let dir = scratch();
        let p = dir.join("index");
        fs::write(&p, b"DIRC\x00\x00\x00\x03\x00\x00\x00\x00").unwrap();
        let err = Index::read(&p).unwrap_err();
        assert!(
            matches!(&err, GitError::Corrupt(m) if m.contains("version 3")),
            "{err:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_extensions_are_skipped() {
        let dir = scratch();
        let p = dir.join("index");
        let mut idx = Index::new();
        idx.stage(entry(b"x", blob_oid(9), 0));
        idx.write(&p).unwrap();
        let mut data = fs::read(&p).unwrap();
        let n = data.len();
        // Insert a fake TREE-like extension before the checksum.
        let ext = b"TREE"
            .to_vec()
            .into_iter()
            .chain(4u32.to_be_bytes())
            .chain(vec![1, 2, 3, 4])
            .collect::<Vec<u8>>();
        data.truncate(n - 20);
        data.extend_from_slice(&ext);
        data.extend_from_slice(&[0u8; 20]); // placeholder checksum
        let mut hasher = Sha1::new();
        hasher.update(&data[..data.len() - 20]);
        let sum = hasher.finalize();
        data.truncate(data.len() - 20);
        data.extend_from_slice(&sum);
        fs::write(&p, data).unwrap();
        let back = Index::read(&p).unwrap();
        assert_eq!(back.entries.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn long_path_round_trips_with_namemask() {
        let dir = scratch();
        let p = dir.join("index");
        let mut idx = Index::new();
        let long = vec![b'a'; 0x1000];
        let mut e = entry(&long, blob_oid(1), 0);
        e.flags = namelen_flags(long.len()); // 0x0FFF
        idx.stage(e);
        idx.write(&p).unwrap();
        let back = Index::read(&p).unwrap();
        assert_eq!(back.entries[0].path, long);
        assert_eq!(back.entries[0].flags & FLAG_NAMEMASK, FLAG_NAMEMASK);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extended_entry_round_trips_before_name() {
        // git layout: 62-byte fixed part, then 2-byte extended flags, then
        // name (probed against real git 2.55 — it reads such v2 files).
        let dir = scratch();
        let p = dir.join("index");
        let mut idx = Index::new();
        let mut e = entry(b"f.txt", blob_oid(1), FLAG_EXTENDED);
        e.extended_flags = 0x1234;
        idx.stage(e);
        idx.write(&p).unwrap();
        let back = Index::read(&p).unwrap();
        assert_eq!(back.entries[0].flags & FLAG_EXTENDED, FLAG_EXTENDED);
        assert_eq!(back.entries[0].extended_flags, 0x1234);
        assert_eq!(back.entries[0].path, b"f.txt");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stage_and_unstage_helpers() {
        let mut idx = Index::new();
        idx.stage(entry(b"f", blob_oid(1), 0));
        idx.stage(entry(b"f", blob_oid(2), FLAG_STAGE_MASK));
        assert_eq!(idx.entries.len(), 2);
        idx.stage(entry(b"f", blob_oid(3), 0)); // replace stage 0
        assert_eq!(idx.entries.len(), 2);
        assert_eq!(idx.entries().iter().filter(|e| e.path == b"f").count(), 2);
        idx.unstage(b"f");
        assert!(idx.entries.is_empty());
    }
}
