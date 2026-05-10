use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupEntry {
    pub path: PathBuf,
    pub bytes: u64,
    pub kind: CleanupEntryKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupEntryKind {
    CargoTarget,
    RunArtifact,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupReport {
    pub entries: Vec<CleanupEntry>,
    pub deleted: bool,
}

impl CleanupReport {
    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|entry| entry.bytes).sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunPruneOptions {
    pub older_than_days: Option<u64>,
    pub keep_latest: Option<usize>,
}

pub fn cleanup_cargo_targets(worktree_root: &Path, delete: bool) -> Result<CleanupReport> {
    let mut entries = find_cargo_targets(worktree_root)?;
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    if delete {
        delete_entries(&entries)?;
    }
    Ok(CleanupReport {
        entries,
        deleted: delete,
    })
}

pub fn cleanup_run_artifacts(
    runs_dir: &Path,
    options: RunPruneOptions,
    delete: bool,
) -> Result<CleanupReport> {
    let mut candidates = find_run_artifacts(runs_dir)?;
    candidates.sort_by(|left, right| {
        modified_at(&left.path)
            .cmp(&modified_at(&right.path))
            .then_with(|| left.path.cmp(&right.path))
    });

    let newest_to_oldest = {
        let mut entries = candidates.clone();
        entries.sort_by(|left, right| {
            modified_at(&right.path)
                .cmp(&modified_at(&left.path))
                .then_with(|| left.path.cmp(&right.path))
        });
        entries
    };

    let mut selected = Vec::new();
    let cutoff = options
        .older_than_days
        .map(|days| SystemTime::now() - Duration::from_secs(days.saturating_mul(24 * 60 * 60)));
    for entry in candidates {
        let by_age = cutoff
            .map(|cutoff| modified_at(&entry.path).unwrap_or(SystemTime::UNIX_EPOCH) <= cutoff)
            .unwrap_or(false);
        let by_count = options
            .keep_latest
            .map(|keep| {
                !newest_to_oldest
                    .iter()
                    .take(keep)
                    .any(|kept| kept.path == entry.path)
            })
            .unwrap_or(false);
        let summarize_only = options.older_than_days.is_none() && options.keep_latest.is_none();
        if summarize_only || by_age || by_count {
            selected.push(entry);
        }
    }

    selected.sort_by(|left, right| left.path.cmp(&right.path));
    if delete {
        delete_entries(&selected)?;
    }
    Ok(CleanupReport {
        entries: selected,
        deleted: delete,
    })
}

fn find_cargo_targets(worktree_root: &Path) -> Result<Vec<CleanupEntry>> {
    if !worktree_root.exists() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for child in fs::read_dir(worktree_root)
        .with_context(|| format!("failed to read Worktree Root {}", worktree_root.display()))?
    {
        let child = child
            .with_context(|| format!("failed to read entry under {}", worktree_root.display()))?;
        let path = child.path();
        if !path.is_dir() {
            continue;
        }
        let target = path.join("target");
        if target.is_dir() {
            entries.push(CleanupEntry {
                bytes: directory_size(&target)?,
                path: target,
                kind: CleanupEntryKind::CargoTarget,
            });
        }
    }
    Ok(entries)
}

fn find_run_artifacts(runs_dir: &Path) -> Result<Vec<CleanupEntry>> {
    if !runs_dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for child in fs::read_dir(runs_dir)
        .with_context(|| format!("failed to read Run Transcript root {}", runs_dir.display()))?
    {
        let child =
            child.with_context(|| format!("failed to read entry under {}", runs_dir.display()))?;
        let path = child.path();
        let metadata = child
            .metadata()
            .with_context(|| format!("failed to read metadata for {}", path.display()))?;
        let bytes = if metadata.is_dir() {
            directory_size(&path)?
        } else if metadata.is_file() {
            metadata.len()
        } else {
            continue;
        };
        entries.push(CleanupEntry {
            path,
            bytes,
            kind: CleanupEntryKind::RunArtifact,
        });
    }
    Ok(entries)
}

fn delete_entries(entries: &[CleanupEntry]) -> Result<()> {
    for entry in entries {
        if entry.path.is_dir() {
            fs::remove_dir_all(&entry.path)
                .with_context(|| format!("failed to remove {}", entry.path.display()))?;
        } else if entry.path.is_file() {
            fs::remove_file(&entry.path)
                .with_context(|| format!("failed to remove {}", entry.path.display()))?;
        }
    }
    Ok(())
}

fn directory_size(path: &Path) -> Result<u64> {
    let mut total = 0_u64;
    if !path.exists() {
        return Ok(0);
    }
    for child in fs::read_dir(path)
        .with_context(|| format!("failed to read directory {}", path.display()))?
    {
        let child =
            child.with_context(|| format!("failed to read entry under {}", path.display()))?;
        let metadata = child
            .metadata()
            .with_context(|| format!("failed to read metadata for {}", child.path().display()))?;
        if metadata.is_dir() {
            total = total.saturating_add(directory_size(&child.path())?);
        } else if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

fn modified_at(path: &Path) -> Option<SystemTime> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_target_dry_run_reports_without_deleting() {
        let temp = tempfile::tempdir().expect("tempdir");
        let worktree_root = temp.path().join("worktrees");
        fs::create_dir_all(worktree_root.join("TASK-1/target/debug")).expect("target dir");
        fs::write(worktree_root.join("TASK-1/target/debug/lib.o"), "12345").expect("artifact");
        fs::create_dir_all(worktree_root.join("TASK-1/src")).expect("source dir");
        fs::write(worktree_root.join("TASK-1/src/lib.rs"), "source").expect("source");

        let report = cleanup_cargo_targets(&worktree_root, false).expect("cleanup report");

        assert!(!report.deleted);
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.total_bytes(), 5);
        assert!(worktree_root.join("TASK-1/target").is_dir());
        assert!(worktree_root.join("TASK-1/src/lib.rs").is_file());
    }

    #[test]
    fn cargo_target_delete_removes_only_rebuildable_target_tree() {
        let temp = tempfile::tempdir().expect("tempdir");
        let worktree_root = temp.path().join("worktrees");
        fs::create_dir_all(worktree_root.join("TASK-1/target/debug")).expect("target dir");
        fs::write(worktree_root.join("TASK-1/target/debug/lib.o"), "12345").expect("artifact");
        fs::create_dir_all(worktree_root.join("TASK-1/src")).expect("source dir");
        fs::write(worktree_root.join("TASK-1/src/lib.rs"), "source").expect("source");

        let report = cleanup_cargo_targets(&worktree_root, true).expect("cleanup delete");

        assert!(report.deleted);
        assert_eq!(report.entries.len(), 1);
        assert!(!worktree_root.join("TASK-1/target").exists());
        assert!(worktree_root.join("TASK-1/src/lib.rs").is_file());
    }

    #[test]
    fn run_artifact_summary_and_count_prune_leave_database_out_of_scope() {
        let temp = tempfile::tempdir().expect("tempdir");
        let runs_dir = temp.path().join(".tasker/data/runs");
        fs::create_dir_all(runs_dir.join("run-1")).expect("run 1");
        fs::write(runs_dir.join("run-1/pi.jsonl"), "old transcript").expect("run 1 transcript");
        std::thread::sleep(Duration::from_millis(5));
        fs::create_dir_all(runs_dir.join("run-2")).expect("run 2");
        fs::write(runs_dir.join("run-2/pi.jsonl"), "new transcript").expect("run 2 transcript");
        let db_path = temp.path().join(".tasker/data/tasker.db");
        fs::write(&db_path, "authoritative database rows").expect("db placeholder");

        let summary =
            cleanup_run_artifacts(&runs_dir, RunPruneOptions::default(), false).expect("summary");
        assert_eq!(summary.entries.len(), 2);
        assert!(runs_dir.join("run-1/pi.jsonl").is_file());

        let pruned = cleanup_run_artifacts(
            &runs_dir,
            RunPruneOptions {
                keep_latest: Some(1),
                older_than_days: None,
            },
            true,
        )
        .expect("prune");

        assert!(pruned.deleted);
        assert_eq!(pruned.entries.len(), 1);
        assert!(!runs_dir.join("run-1").exists());
        assert!(runs_dir.join("run-2/pi.jsonl").is_file());
        assert!(db_path.is_file());
    }
}
