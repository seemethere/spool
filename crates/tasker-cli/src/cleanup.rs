use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};
use sqlx::{FromRow, SqlitePool};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalWorktreeCleanupEntry {
    pub identifier: String,
    pub state: String,
    pub local_worktree: Option<PathBuf>,
    pub task_branch: Option<String>,
    pub safe_to_delete: bool,
    pub reasons: Vec<String>,
    pub actions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalWorktreeCleanupReport {
    pub queue_key: String,
    pub managed_source_repository: PathBuf,
    pub worktree_root: PathBuf,
    pub done_worktree_retention: bool,
    pub deleted: bool,
    pub entries: Vec<LocalWorktreeCleanupEntry>,
}

#[derive(Debug, Clone, FromRow)]
struct LocalWorktreeCleanupRow {
    identifier: String,
    state: String,
    local_worktree: Option<String>,
    task_branch: Option<String>,
    active_agent_runs: i64,
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

pub async fn cleanup_local_worktrees(
    pool: &SqlitePool,
    queue: &tasker_db::TaskQueue,
    delete: bool,
) -> Result<LocalWorktreeCleanupReport> {
    let repo = PathBuf::from(&queue.managed_source_repository);
    let worktree_root = PathBuf::from(&queue.worktree_root);
    let rows = sqlx::query_as::<_, LocalWorktreeCleanupRow>(
        r#"
        SELECT
            tasks.identifier,
            tasks.state,
            (
                SELECT task_links.target FROM task_links
                WHERE task_links.task_id = tasks.id AND task_links.kind = 'local_worktree'
                ORDER BY task_links.is_primary DESC, task_links.created_at DESC, task_links.id DESC
                LIMIT 1
            ) AS local_worktree,
            (
                SELECT task_links.target FROM task_links
                WHERE task_links.task_id = tasks.id AND task_links.kind = 'task_branch'
                ORDER BY task_links.is_primary DESC, task_links.created_at DESC, task_links.id DESC
                LIMIT 1
            ) AS task_branch,
            (
                SELECT COUNT(*) FROM agent_runs
                WHERE agent_runs.task_id = tasks.id AND agent_runs.outcome IS NULL
            ) AS active_agent_runs
        FROM tasks
        WHERE tasks.task_queue_id = ?
          AND (
              tasks.state IN ('done', 'canceled', 'in_progress', 'rework', 'integrating')
              OR EXISTS (
                  SELECT 1 FROM task_links
                  WHERE task_links.task_id = tasks.id
                    AND task_links.kind IN ('local_worktree', 'task_branch')
              )
          )
        ORDER BY tasks.identifier
        "#,
    )
    .bind(&queue.id)
    .fetch_all(pool)
    .await
    .context("failed to load Local Worktree cleanup candidates")?;

    let mut entries = Vec::new();
    for row in rows {
        entries.push(inspect_local_worktree_row(
            &repo,
            &worktree_root,
            queue,
            row,
        )?);
    }

    if delete {
        for entry in &mut entries {
            if !entry.safe_to_delete {
                continue;
            }
            if let Some(worktree) = &entry.local_worktree {
                if worktree.exists() {
                    run_git(
                        &repo,
                        &["worktree", "remove", worktree.to_string_lossy().as_ref()],
                        "remove Local Worktree",
                    )?;
                    entry
                        .actions
                        .push(format!("removed Local Worktree {}", worktree.display()));
                }
            }
            if let Some(branch) = &entry.task_branch {
                if git_success(
                    &repo,
                    &[
                        "show-ref",
                        "--verify",
                        "--quiet",
                        &format!("refs/heads/{branch}"),
                    ],
                )? {
                    run_git(&repo, &["branch", "-D", branch], "delete Task Branch")?;
                    entry.actions.push(format!("deleted Task Branch {branch}"));
                }
            }
        }
    }

    Ok(LocalWorktreeCleanupReport {
        queue_key: queue.key.clone(),
        managed_source_repository: repo,
        worktree_root,
        done_worktree_retention: queue.done_worktree_retention,
        deleted: delete,
        entries,
    })
}

fn inspect_local_worktree_row(
    repo: &Path,
    worktree_root: &Path,
    queue: &tasker_db::TaskQueue,
    row: LocalWorktreeCleanupRow,
) -> Result<LocalWorktreeCleanupEntry> {
    let local_worktree = row.local_worktree.as_ref().map(PathBuf::from);
    let mut reasons = Vec::new();

    let is_terminal_cleanup_state = matches!(row.state.as_str(), "done" | "canceled");
    if !is_terminal_cleanup_state {
        reasons.push(format!("Task State is {}", row.state));
    }
    if queue.done_worktree_retention && row.state == "done" {
        reasons.push("Done Worktree Retention is enabled for this Task Queue".to_string());
    }
    if row.active_agent_runs > 0 {
        reasons.push(format!("{} active Agent Run(s)", row.active_agent_runs));
    }

    let Some(worktree) = &local_worktree else {
        reasons.push("missing Local Worktree Task Link".to_string());
        return Ok(LocalWorktreeCleanupEntry {
            identifier: row.identifier,
            state: row.state,
            local_worktree,
            task_branch: row.task_branch,
            safe_to_delete: false,
            reasons,
            actions: Vec::new(),
        });
    };

    if !worktree.starts_with(worktree_root) {
        reasons.push(format!(
            "Local Worktree path is outside configured Worktree Root {}",
            worktree_root.display()
        ));
    }
    if !worktree.exists() {
        reasons.push("Local Worktree path is missing".to_string());
    } else if !worktree.is_dir() {
        reasons.push("Local Worktree path is not a directory".to_string());
    } else {
        let status = git_output(
            worktree,
            &["status", "--porcelain"],
            "check Local Worktree cleanliness",
        )?;
        if !status.trim().is_empty() {
            reasons.push("Local Worktree has uncommitted changes".to_string());
        }
        if let Some(branch) = &row.task_branch {
            let actual_branch = git_output(
                worktree,
                &["branch", "--show-current"],
                "read Local Worktree branch",
            )?;
            if actual_branch.trim() != branch {
                reasons.push(format!(
                    "Local Worktree branch {} does not match Task Branch link {}",
                    actual_branch.trim(),
                    branch
                ));
            }
        }
    }

    let Some(branch) = &row.task_branch else {
        reasons.push("missing Task Branch Task Link".to_string());
        return Ok(LocalWorktreeCleanupEntry {
            identifier: row.identifier,
            state: row.state,
            local_worktree,
            task_branch: row.task_branch,
            safe_to_delete: false,
            reasons,
            actions: Vec::new(),
        });
    };

    if !git_success(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )? {
        reasons.push("Task Branch is missing".to_string());
    }

    Ok(LocalWorktreeCleanupEntry {
        identifier: row.identifier,
        state: row.state,
        local_worktree,
        task_branch: row.task_branch,
        safe_to_delete: reasons.is_empty(),
        reasons,
        actions: Vec::new(),
    })
}

fn run_git(repo: &Path, args: &[&str], action: &str) -> Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git {:?} in {}", args, repo.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "failed to {action}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn git_output(repo: &Path, args: &[&str], action: &str) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git {:?} in {}", args, repo.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "failed to {action}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn git_success(repo: &Path, args: &[&str]) -> Result<bool> {
    Ok(Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .with_context(|| format!("failed to run git {:?} in {}", args, repo.display()))?
        .success())
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

    #[tokio::test]
    async fn local_worktree_dry_run_classifies_terminal_and_active_states() {
        let setup = LocalWorktreeTestSetup::new(false).await;
        setup.task_with_worktree("done-safe", "done").await;
        setup.task_with_worktree("canceled-safe", "canceled").await;
        setup.task_with_worktree("active-done", "done").await;
        setup.insert_active_run("active-done").await;
        setup
            .task_with_worktree("in-progress-attention", "in_progress")
            .await;
        setup.task_with_worktree("rework-attention", "rework").await;
        setup
            .task_with_worktree("integrating-attention", "integrating")
            .await;

        let report = cleanup_local_worktrees(&setup.pool, &setup.queue, false)
            .await
            .expect("cleanup report");

        assert!(!report.deleted);
        assert!(entry(&report, "done-safe").safe_to_delete);
        assert!(entry(&report, "canceled-safe").safe_to_delete);
        assert_reason(&report, "active-done", "1 active Agent Run(s)");
        assert_reason(
            &report,
            "in-progress-attention",
            "Task State is in_progress",
        );
        assert_reason(&report, "rework-attention", "Task State is rework");
        assert_reason(
            &report,
            "integrating-attention",
            "Task State is integrating",
        );
        assert!(setup.worktrees.join("done-safe").exists());
        assert!(branch_exists(&setup.repo, "tasker/done-safe"));
    }

    #[tokio::test]
    async fn local_worktree_delete_removes_only_safe_terminal_artifacts() {
        let setup = LocalWorktreeTestSetup::new(false).await;
        setup.task_with_worktree("done-safe", "done").await;
        setup.task_with_worktree("dirty-done", "done").await;
        fs::write(setup.worktrees.join("dirty-done/dirty.txt"), "dirty").expect("dirty file");
        setup.task_with_worktree("rework-attention", "rework").await;

        let report = cleanup_local_worktrees(&setup.pool, &setup.queue, true)
            .await
            .expect("cleanup delete");

        assert!(report.deleted);
        assert!(!setup.worktrees.join("done-safe").exists());
        assert!(!branch_exists(&setup.repo, "tasker/done-safe"));
        assert!(setup.worktrees.join("dirty-done").exists());
        assert!(branch_exists(&setup.repo, "tasker/dirty-done"));
        assert!(setup.worktrees.join("rework-attention").exists());
        assert!(branch_exists(&setup.repo, "tasker/rework-attention"));
        assert_eq!(
            entry(&report, "done-safe").actions,
            vec![
                format!(
                    "removed Local Worktree {}",
                    setup.worktrees.join("done-safe").display()
                ),
                "deleted Task Branch tasker/done-safe".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn local_worktree_dry_run_reports_missing_dirty_and_retained_artifacts() {
        let setup = LocalWorktreeTestSetup::new(true).await;
        setup.task_with_worktree("retained-done", "done").await;
        setup.task_with_worktree("dirty-canceled", "canceled").await;
        fs::write(setup.worktrees.join("dirty-canceled/dirty.txt"), "dirty").expect("dirty file");
        setup.task_with_worktree("missing-path", "done").await;
        fs::remove_dir_all(setup.worktrees.join("missing-path")).expect("remove worktree path");
        setup.task_with_worktree("missing-branch", "canceled").await;
        run_git(
            &setup.repo,
            &[
                "worktree",
                "remove",
                setup
                    .worktrees
                    .join("missing-branch")
                    .to_string_lossy()
                    .as_ref(),
            ],
            "remove test worktree",
        )
        .expect("remove test worktree");
        run_git(
            &setup.repo,
            &["branch", "-D", "tasker/missing-branch"],
            "delete branch",
        )
        .expect("delete test branch");
        setup.task_missing_links("missing-links", "done").await;

        let report = cleanup_local_worktrees(&setup.pool, &setup.queue, false)
            .await
            .expect("cleanup report");

        assert_reason(
            &report,
            "retained-done",
            "Done Worktree Retention is enabled for this Task Queue",
        );
        assert_reason(
            &report,
            "dirty-canceled",
            "Local Worktree has uncommitted changes",
        );
        assert_reason(&report, "missing-path", "Local Worktree path is missing");
        assert_reason(&report, "missing-branch", "Task Branch is missing");
        assert_reason(&report, "missing-links", "missing Local Worktree Task Link");
    }

    struct LocalWorktreeTestSetup {
        _temp: tempfile::TempDir,
        pool: SqlitePool,
        queue: tasker_db::TaskQueue,
        repo: PathBuf,
        worktrees: PathBuf,
    }

    impl LocalWorktreeTestSetup {
        async fn new(done_worktree_retention: bool) -> Self {
            let temp = tempfile::tempdir().expect("tempdir");
            let repo = temp.path().join("repo");
            let worktrees = temp.path().join("worktrees");
            fs::create_dir_all(&repo).expect("repo dir");
            fs::create_dir_all(&worktrees).expect("worktrees dir");
            init_git_repo(&repo);

            let pool = tasker_db::connect(&temp.path().join("tasker.db"))
                .await
                .expect("connect");
            tasker_db::run_migrations(&pool).await.expect("migrate");
            let queue = tasker_db::create_task_queue(
                &pool,
                &tasker_db::CreateTaskQueue {
                    key: "TASK".to_string(),
                    name: "Tasker".to_string(),
                    managed_source_repository: repo.display().to_string(),
                    main_branch: "main".to_string(),
                    worktree_root: worktrees.display().to_string(),
                    branch_template: "tasker/{task_identifier}".to_string(),
                    done_worktree_retention,
                    queue_concurrency_limit: None,
                },
                &tasker_db::Actor::operator("tester"),
            )
            .await
            .expect("create queue");

            Self {
                _temp: temp,
                pool,
                queue,
                repo,
                worktrees,
            }
        }

        async fn task_with_worktree(&self, identifier: &str, state: &str) {
            self.task_missing_links(identifier, state).await;
            let branch = format!("tasker/{identifier}");
            let worktree = self.worktrees.join(identifier);
            run_git(
                &self.repo,
                &[
                    "worktree",
                    "add",
                    "-b",
                    &branch,
                    worktree.to_string_lossy().as_ref(),
                    "main",
                ],
                "create test worktree",
            )
            .expect("create worktree");
            tasker_db::upsert_task_link(
                &self.pool,
                identifier,
                &tasker_db::UpsertTaskLink {
                    kind: "local_worktree".to_string(),
                    target: worktree.display().to_string(),
                    label: Some("Local Worktree".to_string()),
                    is_primary: true,
                },
                &tasker_db::Actor::operator("tester"),
            )
            .await
            .expect("link worktree");
            tasker_db::upsert_task_link(
                &self.pool,
                identifier,
                &tasker_db::UpsertTaskLink {
                    kind: "task_branch".to_string(),
                    target: branch,
                    label: Some("Task Branch".to_string()),
                    is_primary: false,
                },
                &tasker_db::Actor::operator("tester"),
            )
            .await
            .expect("link branch");
        }

        async fn insert_active_run(&self, identifier: &str) {
            let task_id: String = sqlx::query_scalar("SELECT id FROM tasks WHERE identifier = ?")
                .bind(identifier)
                .fetch_one(&self.pool)
                .await
                .expect("task id");
            sqlx::query(
                r#"
                INSERT INTO agent_runs (
                    id,
                    task_id,
                    task_queue_id,
                    worker_actor_kind,
                    worker_actor_id,
                    worker_actor_display_name,
                    worker_id,
                    launcher_kind,
                    lease_expires_at
                ) VALUES (?, ?, ?, 'worker_agent', 'worker', 'Worker', 'worker', 'fake', '2099-01-01 00:00:00')
                "#,
            )
            .bind(format!("run-{identifier}"))
            .bind(task_id)
            .bind(&self.queue.id)
            .execute(&self.pool)
            .await
            .expect("insert active Agent Run");
        }

        async fn task_missing_links(&self, identifier: &str, state: &str) {
            tasker_db::create_task(
                &self.pool,
                &tasker_db::CreateTask {
                    queue_key: "TASK".to_string(),
                    title: identifier.to_string(),
                    brief: "brief".to_string(),
                    priority: "normal".to_string(),
                    state: "ready".to_string(),
                    review_required: false,
                    acceptance_criteria: vec!["criterion".to_string()],
                    validation_items: vec!["validation".to_string()],
                    tags: vec![],
                    conflict_hints: vec![],
                    blocking_task_identifiers: vec![],
                },
                &tasker_db::Actor::operator("tester"),
            )
            .await
            .expect("create task");
            sqlx::query("UPDATE tasks SET identifier = ?, state = ? WHERE title = ?")
                .bind(identifier)
                .bind(state)
                .bind(identifier)
                .execute(&self.pool)
                .await
                .expect("set identifier and state");
        }
    }

    fn init_git_repo(repo: &Path) {
        run_git(repo, &["init", "-b", "main"], "init git").expect("git init");
        run_git(
            repo,
            &["config", "user.name", "Tasker Test"],
            "config user name",
        )
        .expect("git config user.name");
        run_git(
            repo,
            &["config", "user.email", "tasker@example.test"],
            "config user email",
        )
        .expect("git config user.email");
        fs::write(repo.join("README.md"), "test\n").expect("readme");
        run_git(repo, &["add", "README.md"], "add readme").expect("git add");
        run_git(repo, &["commit", "-m", "initial"], "commit readme").expect("git commit");
    }

    fn branch_exists(repo: &Path, branch: &str) -> bool {
        git_success(
            repo,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .expect("check branch")
    }

    fn entry<'a>(
        report: &'a LocalWorktreeCleanupReport,
        identifier: &str,
    ) -> &'a LocalWorktreeCleanupEntry {
        report
            .entries
            .iter()
            .find(|entry| entry.identifier == identifier)
            .expect("entry")
    }

    fn assert_reason(report: &LocalWorktreeCleanupReport, identifier: &str, reason: &str) {
        assert!(
            entry(report, identifier)
                .reasons
                .iter()
                .any(|actual| actual == reason),
            "expected {identifier} to include reason {reason:?}; got {:?}",
            entry(report, identifier).reasons
        );
    }
}
