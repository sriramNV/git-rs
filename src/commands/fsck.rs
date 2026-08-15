use crate::error::Result;
use crate::store::ObjectStore;
use std::path::Path;

/// Run `git-rs fsck` — check repository integrity.
pub fn run_fsck(git_dir: &Path, store: &ObjectStore) -> Result<()> {
    // Collect all refs: HEAD, refs/heads/*, reflog entries, index entries
    let mut reachable: std::collections::HashSet<[u8; 20]> = std::collections::HashSet::new();

    // Helper: resolve a ref name to an oid, adding it as reachable
    fn add_ref_oid(ref_name: &str, store: &ObjectStore, reachable: &mut HashSet<[u8; 20]>) {
        // Try to resolve the ref
        let oid = match store.resolve_ref(ref_name) {
            Ok(oid) => oid,
            Err(_) => return,
        };
        reachable.insert(oid);
    }

    // Read HEAD
    let head_path = git_dir.join("HEAD");
    if head_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&head_path) {
            for line in content.lines() {
                let line = line.trim();
                if line.starts_with("ref: ") {
                    let target = line.strip_prefix("ref: ").unwrap().trim();
                    add_ref_oid(target, store, &mut reachable);
                } else if line.len() == 40 {
                    // Detached HEAD with oid
                    let mut oid = [0u8; 20];
                    for (i, chunk) in line.as_bytes().chunks(2).enumerate() {
                        if i < 20 {
                            let _ = hex::from_hex(line[2 * i..2 * i + 2], &mut oid[i]);
                        }
                    }
                    reachable.insert(oid);
                }
            }
        }
    }

    // Read refs/heads/*
    let heads_dir = git_dir.join("refs").join("heads");
    if heads_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&heads_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "ref") {
                    if let Some(file_name) = path.file_name() {
                        let ref_name = format!("refs/heads/{}", file_name.to_string_lossy());
                        add_ref_oid(&ref_name, store, &mut reachable);
                    }
                }
            }
        }
    }

    // Read refs/tags/*
    let tags_dir = git_dir.join("refs").join("tags");
    if tags_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&tags_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "ref") {
                    if let Some(file_name) = path.file_name() {
                        let ref_name = format!("refs/tags/{}", file_name.to_string_lossy());
                        add_ref_oid(&ref_name, store, &mut reachable);
                    }
                }
            }
        }
    }

    // Read reflog entries to find additional reachable objects
    // Reflogs are in refs/logs/* directories
    let logs_dir = git_dir.join("logs");
    if logs_dir.exists() {
        if let Ok(log_entries) = std::fs::read_dir(&logs_dir) {
            for log_entry in log_entries.flatten() {
                let log_path = log_entry.path();
                if log_path.is_dir() {
                    // Read reflog file
                    if let Ok(log_content) = std::fs::read_to_string(&log_path) {
                        for line in log_content.lines() {
                            let line = line.trim();
                            // Reflog format: <old_sha> <new_sha> <ref> <count> <action> <message>
                            // We care about the new_sha (second field)
                            let parts: Vec<&str> = line.split_whitespace().collect();
                            if parts.len() >= 2 {
                                if let Ok(oid) = hex::from_hex(parts[1], &mut [0u8; 20]) {
                                    reachable.insert(oid);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Walk the commit graph from reachable commits to find all reachable objects
    // Use revwalk to traverse parents
    let mut worklist: Vec<[u8; 20]> = reachable.iter().cloned().collect();

    while let Some(oid) = worklist.pop() {
        // Look up the object
        if let Ok(object) = store.find_object(&oid) {
            match object.kind() {
                crate::object::Kind::Commit => {
                    // Get the commit and add its parents
                    if let Ok(commit) = crate::object::Commit::read_from(store, &oid) {
                        for parent_oid in commit.parents() {
                            if reachable.insert(*parent_oid) {
                                worklist.push(*parent_oid);
                            }
                        }
                    }
                }
                crate::object::Kind::Tree => {
                    // Trees reference blobs and subtrees
                    // For now, just mark the tree as reachable
                }
                _ => {}
            }
        }
    }

    // Now check all reachable objects are valid
    // Also check for any obvious corruption

    // For each reachable object, verify it can be read
    for oid in &reachable {
        if store.find_object(oid).is_err() {
            eprintln!("error: unreachable object {} (marked reachable but not found)", hex::hex_to_string(oid));
            std::process::exit(1);
        }
    }

    // Check pack integrity if packs exist
    let pack_dir = git_dir.join("objects").join("pack");
    if pack_dir.exists() {
        // Check for idx files and verify pack headers
        // For now, just verify we can list them
        let _ = std::fs::read_dir(&pack_dir);
    }

    Ok(())
}

/// Run fsck as a CLI command.
pub fn run_fsck_cmd(args: &[String]) -> Result<()> {
    let git_dir = Path::new(".git");
    let store = crate::store::ObjectStore::discover()?;

    run_fsck(git_dir, &store)?;
    Ok(())
}