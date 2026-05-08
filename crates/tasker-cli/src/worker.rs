use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, Context, Result};
use sqlx::SqlitePool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkOnceRequest {
    pub queue: String,
    pub launcher: String,
    pub actor: String,
    pub fake_outcome: String,
    pub lease_seconds: i64,
    pub retry_hold_seconds: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkOnceOutcome {
    NoEligibleTask {
        queue: String,
    },
    Finished {
        task_identifier: String,
        run_id: String,
        outcome: String,
    },
}

pub async fn run_worker_once(
    pool: &SqlitePool,
    request: WorkOnceRequest,
) -> Result<WorkOnceOutcome> {
    if request.launcher != "fake" {
        bail!("only the fake Agent Launcher is available in this milestone");
    }

    let actor = tasker_db::Actor {
        kind: "worker_agent".to_string(),
        id: request.actor.clone(),
        display_name: request.actor.clone(),
    };
    let claim = tasker_db::claim_next(
        pool,
        &tasker_db::ClaimNextInput {
            queue_key: request.queue.clone(),
            worker_id: request.actor.clone(),
            launcher_kind: request.launcher,
            lease_seconds: request.lease_seconds,
        },
        &actor,
    )
    .await?;

    let Some(claimed) = claim else {
        return Ok(WorkOnceOutcome::NoEligibleTask {
            queue: request.queue,
        });
    };

    prepare_local_worktree(pool, &claimed.task, &actor).await?;
    tasker_db::heartbeat_run(pool, &claimed.run.id, request.lease_seconds, &actor).await?;
    let fake_note = fake_workpad_note(
        &claimed.task.task.identifier,
        &claimed.run.id,
        &request.fake_outcome,
    );
    tasker_db::update_workpad_note(pool, &claimed.task.task.identifier, &fake_note, &actor).await?;
    let finished = tasker_db::finish_run(
        pool,
        &claimed.run.id,
        &tasker_db::FinishRunInput {
            outcome: request.fake_outcome,
            failure_reason: None,
            retry_hold_seconds: request.retry_hold_seconds,
        },
        &actor,
    )
    .await?;

    Ok(WorkOnceOutcome::Finished {
        task_identifier: claimed.task.task.identifier,
        run_id: finished.id,
        outcome: finished.outcome.unwrap_or_else(|| "unknown".to_string()),
    })
}

async fn prepare_local_worktree(
    pool: &SqlitePool,
    task: &tasker_db::TaskDetail,
    actor: &tasker_db::Actor,
) -> Result<()> {
    let queue = tasker_db::get_task_queue(pool, &task.task.task_queue_key)
        .await?
        .with_context(|| format!("Task Queue {} not found", task.task.task_queue_key))?;
    let branch = queue
        .branch_template
        .replace("{task_identifier}", &task.task.identifier);
    let worktree_path = PathBuf::from(&queue.worktree_root).join(&task.task.identifier);

    setup_local_worktree(
        Path::new(&queue.managed_source_repository),
        &queue.main_branch,
        &branch,
        &worktree_path,
    )?;

    tasker_db::upsert_task_link(
        pool,
        &task.task.identifier,
        &tasker_db::UpsertTaskLink {
            kind: "local_worktree".to_string(),
            target: worktree_path.display().to_string(),
            label: Some("Local Worktree".to_string()),
            is_primary: true,
        },
        actor,
    )
    .await?;
    tasker_db::upsert_task_link(
        pool,
        &task.task.identifier,
        &tasker_db::UpsertTaskLink {
            kind: "task_branch".to_string(),
            target: branch,
            label: Some("Task Branch".to_string()),
            is_primary: false,
        },
        actor,
    )
    .await?;
    Ok(())
}

fn setup_local_worktree(
    managed_source_repository: &Path,
    main_branch: &str,
    task_branch: &str,
    worktree_path: &Path,
) -> Result<()> {
    ensure_git_repository(managed_source_repository)?;
    ensure_clean_repository(managed_source_repository)?;
    validate_task_branch(managed_source_repository, task_branch)?;
    if let Some(parent) = worktree_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    if !git_branch_exists(managed_source_repository, task_branch)? {
        run_git(
            managed_source_repository,
            &["branch", task_branch, main_branch],
            "create Task Branch",
        )?;
    }
    if worktree_path.exists() {
        ensure_existing_worktree(managed_source_repository, worktree_path, task_branch)?;
    } else {
        let worktree = worktree_path.display().to_string();
        run_git(
            managed_source_repository,
            &["worktree", "add", &worktree, task_branch],
            "create Local Worktree",
        )?;
    }
    Ok(())
}

fn ensure_git_repository(path: &Path) -> Result<()> {
    run_git(
        path,
        &["rev-parse", "--show-toplevel"],
        "validate Managed Source Repository",
    )?;
    Ok(())
}

fn ensure_clean_repository(path: &Path) -> Result<()> {
    let output = git_output(
        path,
        &["status", "--porcelain"],
        "check Managed Source Repository cleanliness",
    )?;
    if !output.trim().is_empty() {
        bail!("Managed Source Repository has unexpected uncommitted changes");
    }
    Ok(())
}

fn validate_task_branch(repo: &Path, branch: &str) -> Result<()> {
    if branch.trim().is_empty() || branch.starts_with('-') {
        bail!("Task Branch must be a non-option Git branch name");
    }
    run_git(
        repo,
        &["check-ref-format", "--branch", branch],
        "validate Task Branch",
    )?;
    Ok(())
}

fn ensure_existing_worktree(
    managed_source_repository: &Path,
    worktree_path: &Path,
    task_branch: &str,
) -> Result<()> {
    run_git(
        worktree_path,
        &["rev-parse", "--is-inside-work-tree"],
        "validate existing Local Worktree",
    )?;
    let branch = git_output(
        worktree_path,
        &["branch", "--show-current"],
        "read existing Local Worktree branch",
    )?;
    if branch.trim() != task_branch {
        bail!(
            "existing Local Worktree is on branch {}, expected {}",
            branch.trim(),
            task_branch
        );
    }
    let source_common_dir = git_common_dir(managed_source_repository)?;
    let worktree_common_dir = git_common_dir(worktree_path)?;
    if source_common_dir != worktree_common_dir {
        bail!(
            "existing Local Worktree is not attached to the configured Managed Source Repository"
        );
    }
    Ok(())
}

fn git_common_dir(repo: &Path) -> Result<PathBuf> {
    let common_dir = git_output(
        repo,
        &["rev-parse", "--git-common-dir"],
        "read Git common dir",
    )?;
    let path = PathBuf::from(common_dir.trim());
    let absolute = if path.is_absolute() {
        path
    } else {
        repo.join(path)
    };
    absolute
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", absolute.display()))
}

fn git_branch_exists(repo: &Path, branch: &str) -> Result<bool> {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["show-ref", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .status()
        .context("failed to check Task Branch")?;
    Ok(status.success())
}

fn run_git(repo: &Path, args: &[&str], action: &str) -> Result<()> {
    git_output(repo, args, action).map(|_| ())
}

fn git_output(repo: &Path, args: &[&str], action: &str) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git to {action}"))?;
    if !output.status.success() {
        bail!(
            "failed to {action}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn fake_workpad_note(task_identifier: &str, run_id: &str, outcome: &str) -> String {
    format!(
        "Fake Agent Launcher processed Task {task_identifier} in Agent Run {run_id}.\nOutcome: {outcome}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_worker_prepares_local_worktree_and_records_task_links() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let worktrees = temp.path().join("worktrees");
        init_git_repo(&repo);
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        tasker_db::create_task_queue(
            &pool,
            &tasker_db::CreateTaskQueue {
                key: "TASK".to_string(),
                name: "Tasker".to_string(),
                managed_source_repository: repo.display().to_string(),
                main_branch: "main".to_string(),
                worktree_root: worktrees.display().to_string(),
                branch_template: "tasker/{task_identifier}".to_string(),
                done_worktree_retention: false,
                queue_concurrency_limit: None,
            },
            &tasker_db::Actor::operator("tester"),
        )
        .await
        .expect("create queue");
        tasker_db::create_task(
            &pool,
            &tasker_db::CreateTask {
                queue_key: "TASK".to_string(),
                title: "Test worktree".to_string(),
                brief: "Prepare worktree".to_string(),
                priority: "normal".to_string(),
                state: "ready".to_string(),
                review_required: false,
                acceptance_criteria: vec!["Worktree exists".to_string()],
                validation_items: vec!["Task Links recorded".to_string()],
                tags: vec![],
            },
            &tasker_db::Actor::operator("tester"),
        )
        .await
        .expect("create task");

        let outcome = run_worker_once(
            &pool,
            WorkOnceRequest {
                queue: "TASK".to_string(),
                launcher: "fake".to_string(),
                actor: "worker".to_string(),
                fake_outcome: "completed".to_string(),
                lease_seconds: 90,
                retry_hold_seconds: None,
            },
        )
        .await
        .expect("run worker");

        assert!(matches!(outcome, WorkOnceOutcome::Finished { .. }));
        assert!(worktrees.join("TASK-1").is_dir());
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("load task")
            .expect("task exists");
        assert!(detail
            .task_links
            .iter()
            .any(|link| link.kind == "local_worktree" && link.is_primary));
        assert!(detail
            .task_links
            .iter()
            .any(|link| link.kind == "task_branch" && link.target == "tasker/TASK-1"));
    }

    #[test]
    fn local_worktree_setup_rejects_dirty_managed_source_repository() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        init_git_repo(&repo);
        fs::write(repo.join("dirty.txt"), "dirty").expect("dirty file");

        let error = setup_local_worktree(
            &repo,
            "main",
            "tasker/TASK-1",
            &temp.path().join("worktrees/TASK-1"),
        )
        .expect_err("dirty repo fails");

        assert!(error.to_string().contains("unexpected uncommitted changes"));
    }

    #[test]
    fn local_worktree_setup_rejects_existing_plain_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let worktree = temp.path().join("worktrees/TASK-1");
        init_git_repo(&repo);
        fs::create_dir_all(&worktree).expect("plain dir");

        let error = setup_local_worktree(&repo, "main", "tasker/TASK-1", &worktree)
            .expect_err("plain dir fails");

        assert!(error
            .to_string()
            .contains("validate existing Local Worktree"));
    }

    #[test]
    fn local_worktree_setup_rejects_unrelated_existing_worktree_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let unrelated = temp.path().join("worktrees/TASK-1");
        init_git_repo(&repo);
        init_git_repo(&unrelated);
        git(&unrelated, &["checkout", "-b", "tasker/TASK-1"]);

        let error = setup_local_worktree(&repo, "main", "tasker/TASK-1", &unrelated)
            .expect_err("unrelated repo fails");

        assert!(error
            .to_string()
            .contains("not attached to the configured Managed Source Repository"));
    }

    #[test]
    fn local_worktree_setup_rejects_option_like_branch_names() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        init_git_repo(&repo);

        let error = setup_local_worktree(
            &repo,
            "main",
            "-bad-branch",
            &temp.path().join("worktrees/TASK-1"),
        )
        .expect_err("option-like branch fails");

        assert!(error.to_string().contains("non-option Git branch"));
    }

    #[test]
    fn fake_workpad_note_is_deterministic() {
        assert_eq!(
            fake_workpad_note("TASK-1", "run-1", "completed"),
            "Fake Agent Launcher processed Task TASK-1 in Agent Run run-1.\nOutcome: completed\n"
        );
    }

    fn init_git_repo(repo: &Path) {
        fs::create_dir_all(repo).expect("repo dir");
        git(repo, &["init", "-b", "main"]);
        git(repo, &["config", "user.email", "tasker@example.test"]);
        git(repo, &["config", "user.name", "Tasker Test"]);
        fs::write(repo.join("README.md"), "test repo\n").expect("readme");
        git(repo, &["add", "README.md"]);
        git(repo, &["commit", "-m", "initial"]);
    }

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
