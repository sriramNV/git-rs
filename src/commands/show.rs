//! `git-rs show [<rev>]` — commit header (sha, author, date, message) plus
//! a `--stat`-style change summary, byte-identical to `git show --stat`
//! (probed against git 2.55).
//!
//! v1: the default is header + stat (git's default also includes the full
//! patch — D-015); the date is the author date formatted in the commit's
//! timezone; tagged revisions peel to their commit (git prints the tag
//! object header first; v1 does not, D-015); a tag pointing at a non-
//! commit resolves to nothing (`ambiguous argument`, like git).

use std::collections::HashMap;

use crate::error::{GitError, Result};
use crate::object::{Commit, Ident};
use crate::refs::Refs;
use crate::revwalk::{hex, object_name_error, resolve_rev, unborn_fatal};
use crate::store::{Kind, ObjectStore};

/// `git-rs show [<rev>]`
pub fn run_show(args: &[String]) -> Result<()> {
    for arg in args {
        if arg.starts_with('-') {
            return Err(GitError::Invalid(format!("show: unknown option '{arg}'")));
        }
    }
    let rev = args.first().map(String::as_str).unwrap_or("HEAD");

    let refs = Refs::discover()?;
    let store = ObjectStore::discover()?;
    let Some(sha) = resolve_rev(&refs, &store, rev)? else {
        if rev == "HEAD" {
            return Err(unborn_fatal(&refs));
        }
        return Err(object_name_error(rev));
    };

    let (kind, content) = store.read_object(&hex(&sha))?;
    if kind != Kind::Commit {
        return Err(GitError::Corrupt(format!("{} is not a commit", hex(&sha))));
    }
    let commit = Commit::parse(&content)?;

    println!("commit {}", hex(&sha));
    println!("Author: {} <{}>", commit.author.name, commit.author.email);
    println!("Date:   {}", fmt_date(&commit.author));
    println!();
    for line in String::from_utf8_lossy(&commit.message).lines() {
        println!("    {line}");
    }
    println!();

    let old_tree = match commit.parents.first() {
        Some(p) => Some(commit_tree(&store, p)?),
        None => None,
    };
    print_stat(&store, old_tree.as_deref(), &hex(&commit.tree))
}

/// The tree sha a commit points at.
fn commit_tree(store: &ObjectStore, sha: &[u8; 20]) -> Result<String> {
    let (kind, content) = store.read_object(&hex(sha))?;
    if kind != Kind::Commit {
        return Err(GitError::Corrupt(format!("{} is not a commit", hex(sha))));
    }
    Ok(hex(&Commit::parse(&content)?.tree))
}

/// Format a commit timestamp like git's `show_date` default
/// (`Thu Aug 13 10:00:20 2026 +0530`): English weekday/month names, the
/// date computed in the ident's timezone.
fn fmt_date(ident: &Ident) -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // tz is signed HHMM-as-int (530 = +05:30 = 330 minutes).
    let off_min = (ident.tz / 100) * 60 + (ident.tz % 100);
    let local = ident.ts + off_min as i64 * 60;
    let days = local.div_euclid(86400);
    let rem = local.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    let wd = WEEKDAYS[(days + 4).rem_euclid(7) as usize];
    let sign = if ident.tz < 0 { '-' } else { '+' };
    format!(
        "{wd} {} {d:02} {:02}:{:02}:{:02} {y} {sign}{:04}",
        MONTHS[(m - 1) as usize],
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60,
        ident.tz.abs()
    )
}

/// Days since epoch → `(year, month 1-12, day 1-31)` (Howard Hinnant's
/// civil-from-days algorithm; no date crate allowed).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

/// One file's stat line data.
struct StatFile {
    path: Vec<u8>,
    ins: u64,
    del: u64,
    binary: bool,
    /// Blob sizes; the missing side is 0.
    old_size: u64,
    new_size: u64,
}

/// Print git's diffstat block: per-file lines plus the
/// `N files changed, X insertions(+), Y deletions(-)` summary.
fn print_stat(store: &ObjectStore, old_tree: Option<&str>, new_tree: &str) -> Result<()> {
    let old = tree_blobs(store, old_tree)?;
    let new = tree_blobs(store, Some(new_tree))?;

    let mut paths: Vec<&Vec<u8>> = old.keys().chain(new.keys()).collect();
    paths.sort();
    paths.dedup();

    let mut stats: Vec<StatFile> = Vec::new();
    for p in paths {
        let old_oid = old.get(p);
        let new_oid = new.get(p);
        let mut s = StatFile {
            path: (*p).clone(),
            ins: 0,
            del: 0,
            binary: false,
            old_size: 0,
            new_size: 0,
        };
        let old_bytes = match old_oid {
            Some(o) => {
                let (_, content) = store.read_object(&hex_oid(o))?;
                s.old_size = content.len() as u64;
                Some(content)
            }
            None => None,
        };
        let new_bytes = match new_oid {
            Some(o) => {
                let (_, content) = store.read_object(&hex_oid(o))?;
                s.new_size = content.len() as u64;
                Some(content)
            }
            None => None,
        };
        s.binary = match (&old_bytes, &new_bytes) {
            (Some(a), Some(b)) => crate::diff::is_binary(a) || crate::diff::is_binary(b),
            (Some(a), None) => crate::diff::is_binary(a),
            (None, Some(b)) => crate::diff::is_binary(b),
            (None, None) => false,
        };
        if !s.binary {
            match (&old_bytes, &new_bytes) {
                (Some(a), Some(b)) => {
                    let old_lines = crate::diff::split_lines(a);
                    let new_lines = crate::diff::split_lines(b);
                    for hunk in crate::diff::diff_lines(&old_lines, &new_lines) {
                        for line in &hunk.lines {
                            match line {
                                crate::diff::DiffLine::Add(_) => s.ins += 1,
                                crate::diff::DiffLine::Delete(_) => s.del += 1,
                                crate::diff::DiffLine::Context(_) => {}
                            }
                        }
                    }
                }
                (Some(a), None) => s.del = crate::diff::split_lines(a).len() as u64,
                (None, Some(b)) => s.ins = crate::diff::split_lines(b).len() as u64,
                (None, None) => {}
            }
        }
        stats.push(s);
    }

    let width = stats.iter().map(|s| s.path.len()).max().unwrap_or(0);
    let mut ins = 0u64;
    let mut del = 0u64;
    for s in &stats {
        ins += s.ins;
        del += s.del;
        let name = String::from_utf8_lossy(&s.path);
        if s.binary {
            println!(" {name:<width$} | Bin {} -> {} bytes", s.old_size, s.new_size);
        } else if s.ins + s.del > 0 {
            let sym = format!("{}{}", "+".repeat(s.ins as usize), "-".repeat(s.del as usize));
            println!(" {name:<width$} | {} {sym}", s.ins + s.del);
        } else {
            // Mode-only change: filename alone, no bar (git shows the name).
            println!(" {name:<width$} | 0");
        }
    }
    let has_binary = stats.iter().any(|s| s.binary);
    let files_txt = if stats.len() == 1 {
        " 1 file changed".to_string()
    } else {
        format!(" {} files changed", stats.len())
    };
    let ins_txt = if ins == 1 {
        "1 insertion(+)".to_string()
    } else {
        format!("{ins} insertions(+)")
    };
    let del_txt = if del == 1 {
        "1 deletion(-)".to_string()
    } else {
        format!("{del} deletions(-)")
    };
    let mut summary = files_txt;
    if ins > 0 || has_binary {
        summary.push_str(&format!(", {ins_txt}"));
    }
    if del > 0 || has_binary {
        summary.push_str(&format!(", {del_txt}"));
    }
    println!("{summary}");
    Ok(())
}

/// Path → blob oid map for every blob under a tree (empty when no tree).
fn tree_blobs(
    store: &ObjectStore,
    tree: Option<&str>,
) -> Result<HashMap<Vec<u8>, [u8; 20]>> {
    let mut out = HashMap::new();
    let Some(tree) = tree else {
        return Ok(out);
    };
    let (kind, content) = store.read_object(tree)?;
    if kind != Kind::Tree {
        return Err(GitError::Corrupt(format!("{tree} is not a tree")));
    }
    let t = crate::object::Tree::parse(&content)?;
    collect_tree(store, &t, &mut vec![], &mut out)?;
    Ok(out)
}

fn collect_tree(
    store: &ObjectStore,
    tree: &crate::object::Tree,
    prefix: &mut Vec<u8>,
    out: &mut HashMap<Vec<u8>, [u8; 20]>,
) -> Result<()> {
    for e in &tree.entries {
        if e.is_dir() {
            let len = prefix.len();
            prefix.extend_from_slice(&e.name);
            prefix.push(b'/');
            let (kind, content) = store.read_object(&hex_oid(&e.oid))?;
            if kind != Kind::Tree {
                return Err(GitError::Corrupt(format!("{} is not a tree", hex_oid(&e.oid))));
            }
            let sub = crate::object::Tree::parse(&content)?;
            collect_tree(store, &sub, prefix, out)?;
            prefix.truncate(len);
        } else {
            let mut path = prefix.clone();
            path.extend_from_slice(&e.name);
            out.insert(path, e.oid);
        }
    }
    Ok(())
}

fn hex_oid(oid: &[u8; 20]) -> String {
    hex(oid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_known_dates() {
        // 1970-01-01 (Thu).
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2000-03-01.
        assert_eq!(civil_from_days(11017), (2000, 3, 1));
        // 2026-08-13.
        let days = (1786610047 + 5 * 3600 + 30 * 60) / 86400;
        let (y, m, d) = civil_from_days(days);
        assert_eq!((y, m, d), (2026, 8, 13));
        // Leap day.
        assert_eq!(civil_from_days(22024), (2030, 4, 20));
    }

    #[test]
    fn fmt_date_matches_git_format() {
        // 2026-08-13 10:00:47 +0530 = UTC 04:30:47 = ts 1786595447.
        let ident = Ident::new("A", "a@e.co", 1786595447, 530).unwrap();
        assert_eq!(fmt_date(&ident), "Thu Aug 13 10:00:47 2026 +0530");
        let ident = Ident::new("A", "a@e.co", 1786610047, 530).unwrap();
        assert_eq!(fmt_date(&ident), "Thu Aug 13 14:04:07 2026 +0530");
        let ident = Ident::new("A", "a@e.co", 0, 0).unwrap();
        assert_eq!(fmt_date(&ident), "Thu Jan 01 00:00:00 1970 +0000");
    }
}