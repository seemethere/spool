use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};
use sqlx::{Row, SqlitePool};

use crate::commit_metadata;

const INTEGRATION_RETRY_MAX_ATTEMPTS: i64 = 3;
const INTEGRATION_RETRY_INITIAL_DELAY_SECONDS: i64 = 30;
const VALIDATION_COMMANDS_FILE: &str = ".tasker/validation-commands.txt";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalIntegrationResult {
    pub summary: String,
}

pub async fn integrate_local_worktree_for_run(
    pool: &SqlitePool,
    identifier: &str,
    agent_run_id: Option<&str>,
    actor: &tasker_db::Actor,
    data_dir: &Path,
) -> Result<LocalIntegrationResult> {
    let detail = tasker_db::get_task_detail(pool, identifier)
        .await?
        .with_context(|| format!("Task {identifier} not found"))?;
    if detail.task.state != "integrating" {
        anyhow::bail!(
            "Local Worktree integration requires Task State integrating; current state is {}",
            detail.task.state
        );
    }
    let queue = tasker_db::get_task_queue(pool, &detail.task.task_queue_key)
        .await?
        .with_context(|| format!("Task Queue {} not found", detail.task.task_queue_key))?;
    let task_branch = required_task_link(&detail, "task_branch")?;
    let local_worktree = required_task_link(&detail, "local_worktree")?;
    let _repo_operation_lock = crate::repo_lock::acquire_guard(
        data_dir,
        &detail.task.task_queue_key,
        "integration",
        Some(identifier),
    )?;

    let mut adapter = LocalWorktreeIntegrationAdapter {
        pool,
        task: &detail,
        queue: &queue,
        actor,
        agent_run_id,
        repo: Path::new(&queue.managed_source_repository),
        worktree: Path::new(&local_worktree),
        task_branch: &task_branch,
        auto_refreshed: false,
    };
    adapter.integrate().await
}

fn required_task_link(detail: &tasker_db::TaskDetail, kind: &str) -> Result<String> {
    detail
        .task_links
        .iter()
        .find(|link| link.kind == kind)
        .map(|link| link.target.clone())
        .with_context(|| {
            format!(
                "Task {} is missing {kind} Task Link",
                detail.task.identifier
            )
        })
}

struct LocalWorktreeIntegrationAdapter<'a> {
    pool: &'a SqlitePool,
    task: &'a tasker_db::TaskDetail,
    queue: &'a tasker_db::TaskQueue,
    actor: &'a tasker_db::Actor,
    agent_run_id: Option<&'a str>,
    repo: &'a Path,
    worktree: &'a Path,
    task_branch: &'a str,
    auto_refreshed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StaleValidatedBase {
    previous_base: String,
    current_main: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct IntegrationOutcomeRetryFields {
    retryable: bool,
    retry_attempt: Option<i64>,
    retry_delay_seconds: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IntegrationRetryPolicy {
    attempt: i64,
    retryable: bool,
    delay_seconds: Option<i64>,
}

fn retryable_operational_failure_message(reason: &str, retry: &IntegrationRetryPolicy) -> String {
    serde_json::json!({
        "reason": reason,
        "retryable": retry.retryable,
        "retry_attempt": retry.attempt,
        "max_attempts": INTEGRATION_RETRY_MAX_ATTEMPTS,
        "retry_delay_seconds": retry.delay_seconds,
    })
    .to_string()
}

fn integration_reason_code(kind: &str, message: Option<&str>) -> &'static str {
    let message = message.unwrap_or_default();
    if message.contains("Local Worktree has uncommitted changes") {
        "uncommitted_local_worktree"
    } else if message.contains("auto-refresh validation failed") {
        "auto_refresh_validation_failed"
    } else if message.contains("auto-refresh declined: no validation command source") {
        "auto_refresh_declined_missing_validation"
    } else if message.contains("auto-refresh conflict") {
        "auto_refresh_conflict"
    } else if message.contains("stale Validated Base Commit") {
        "stale_validated_base_commit"
    } else if message.contains("does not include current Main Branch") {
        "task_branch_missing_main"
    } else if message.contains("Managed Source Repository has uncommitted changes") {
        "dirty_managed_source_repository"
    } else if message.contains("Git lock exists") {
        "repo_operation_lock_held"
    } else if message.contains("Squash Merge failed") {
        "merge_conflict"
    } else if kind == "success" {
        "success"
    } else if kind == "no_changes" {
        "no_changes"
    } else if kind == "work_change_failure" {
        "unknown_work_change_failure"
    } else {
        "unknown_operational_failure"
    }
}

fn is_stale_validated_base_error(error: &anyhow::Error) -> bool {
    error.to_string().contains("stale Validated Base Commit")
}

impl<'a> LocalWorktreeIntegrationAdapter<'a> {
    async fn integrate(&mut self) -> Result<LocalIntegrationResult> {
        if let Err(error) = self.validate_operational_safety() {
            let retry = self.next_retry_policy().await?;
            let message = retryable_operational_failure_message(&error.to_string(), &retry);
            self.record_outcome(
                "operational_failure",
                integration_reason_code("operational_failure", Some(&error.to_string())),
                None,
                None,
                Some(message),
                IntegrationOutcomeRetryFields {
                    retryable: retry.retryable,
                    retry_attempt: Some(retry.attempt),
                    retry_delay_seconds: retry.delay_seconds,
                },
            )
            .await?;
            let retry_summary = retry
                .delay_seconds
                .map(|seconds| format!("; retry attempt {} scheduled in {seconds}s", retry.attempt))
                .unwrap_or_else(|| format!("; retry attempt {} reached max attempts {}; operator intervention required", retry.attempt, INTEGRATION_RETRY_MAX_ATTEMPTS));
            return Ok(LocalIntegrationResult {
                summary: format!(
                    "operational Delivery Failure for Task {}; left in Integrating{retry_summary}: {error:#}",
                    self.task.task.identifier
                ),
            });
        }
        if let Err(error) = self.validate_work_change_safety() {
            let stale_candidate = if is_stale_validated_base_error(&error) {
                self.stale_validated_base_candidate()?
            } else {
                None
            };
            if let Some(stale) = stale_candidate {
                match self.auto_refresh_stale_validated_base(&stale).await {
                    Ok(summary) => {
                        self.auto_refreshed = true;
                        self.validate_work_change_safety().with_context(|| {
                            format!("auto-refresh completed but refreshed Task Branch still failed integration preflight: {summary}")
                        })?;
                    }
                    Err(refresh_error) => {
                        let message = refresh_error.to_string();
                        self.record_outcome(
                            "work_change_failure",
                            integration_reason_code("work_change_failure", Some(&message)),
                            None,
                            Some(stale.current_main),
                            Some(message.clone()),
                            IntegrationOutcomeRetryFields::default(),
                        )
                        .await?;
                        self.transition("rework").await?;
                        return Ok(LocalIntegrationResult {
                            summary: format!(
                                "work-change Delivery Failure for Task {}; moved to Rework: {message}",
                                self.task.task.identifier
                            ),
                        });
                    }
                }
            } else {
                self.record_outcome(
                    "work_change_failure",
                    integration_reason_code("work_change_failure", Some(&error.to_string())),
                    None,
                    None,
                    Some(error.to_string()),
                    IntegrationOutcomeRetryFields::default(),
                )
                .await?;
                self.transition("rework").await?;
                return Ok(LocalIntegrationResult {
                    summary: format!(
                        "work-change Delivery Failure for Task {}; moved to Rework: {error:#}",
                        self.task.task.identifier
                    ),
                });
            }
        }

        let pre_merge_head = git_output(
            self.repo,
            &["rev-parse", &self.queue.main_branch],
            "read Main Branch commit",
        )?
        .trim()
        .to_string();
        if git_status(
            self.repo,
            &[
                "diff",
                "--quiet",
                &format!("{}..{}", self.queue.main_branch, self.task_branch),
                "--",
            ],
        )?
        .success()
        {
            let cleanup = self.cleanup_after_success();
            let cleanup_message = cleanup
                .as_ref()
                .err()
                .map(|error| format!("cleanup needs operator repair: {error:#}"));
            self.record_outcome(
                "no_changes",
                cleanup_message
                    .as_ref()
                    .map_or("no_changes", |_| "cleanup_failure"),
                None,
                Some(pre_merge_head.clone()),
                cleanup_message.clone(),
                IntegrationOutcomeRetryFields::default(),
            )
            .await?;
            self.transition("done").await?;
            let mut summary = format!(
                "No-Change Integration recorded for Task {}; moved to Done",
                self.task.task.identifier
            );
            if let Some(message) = cleanup_message {
                summary.push_str(&format!("; {message}"));
            }
            return Ok(LocalIntegrationResult { summary });
        }

        if let Err(error) = run_git(
            self.repo,
            &["merge", "--squash", "--no-commit", self.task_branch],
            "Squash Merge",
        ) {
            let _ = self.rollback_to(&pre_merge_head);
            self.record_outcome(
                "work_change_failure",
                "merge_conflict",
                None,
                Some(pre_merge_head.clone()),
                Some(format!("Squash Merge failed: {error:#}")),
                IntegrationOutcomeRetryFields::default(),
            )
            .await?;
            self.transition("rework").await?;
            return Ok(LocalIntegrationResult {
                summary: format!(
                    "work-change Delivery Failure for Task {}; moved to Rework: Squash Merge failed: {error:#}",
                    self.task.task.identifier
                ),
            });
        }

        let message = commit_metadata::final_commit_message(
            &self.task.task.identifier,
            &self.task.task.title,
            &self.task.task.task_queue_key,
            self.agent_run_id,
        );
        let parsed_trailers = commit_metadata::parse_tasker_commit_trailers(&message);
        debug_assert_eq!(
            parsed_trailers.task_identifier.as_deref(),
            Some(self.task.task.identifier.as_str())
        );
        debug_assert_eq!(
            parsed_trailers.task_queue.as_deref(),
            Some(self.task.task.task_queue_key.as_str())
        );
        if let Err(error) = run_git(self.repo, &["commit", "-m", &message], "Final Commit") {
            let _ = self.rollback_to(&pre_merge_head);
            self.record_outcome(
                "work_change_failure",
                "unknown_work_change_failure",
                None,
                Some(pre_merge_head.clone()),
                Some(format!("Final Commit failed: {error:#}")),
                IntegrationOutcomeRetryFields::default(),
            )
            .await?;
            self.transition("rework").await?;
            return Ok(LocalIntegrationResult {
                summary: format!(
                    "work-change Delivery Failure for Task {}; moved to Rework: Final Commit failed: {error:#}",
                    self.task.task.identifier
                ),
            });
        }

        let final_commit = git_output(self.repo, &["rev-parse", "HEAD"], "read Final Commit")?
            .trim()
            .to_string();
        let cleanup = self.cleanup_after_success();
        let cleanup_message = cleanup
            .as_ref()
            .err()
            .map(|error| format!("cleanup needs operator repair: {error:#}"));
        self.record_outcome(
            "success",
            cleanup_message.as_ref().map_or_else(
                || {
                    if self.auto_refreshed {
                        "auto_refresh_success"
                    } else {
                        "success"
                    }
                },
                |_| "cleanup_failure",
            ),
            Some(final_commit.clone()),
            Some(pre_merge_head),
            cleanup_message.clone(),
            IntegrationOutcomeRetryFields::default(),
        )
        .await
        .with_context(|| {
            format!(
                "Final Commit {final_commit} was created but Tasker could not record the successful Integration Outcome; operator repair required"
            )
        })?;
        self.transition("done").await.with_context(|| {
            format!(
                "Final Commit {final_commit} was created but Tasker could not move the Task to Done; operator repair required"
            )
        })?;
        let mut summary = format!(
            "Integrated Task {} as Final Commit {}; moved to Done",
            self.task.task.identifier, final_commit
        );
        if let Some(message) = cleanup_message {
            summary.push_str(&format!("; {message}"));
        }
        Ok(LocalIntegrationResult { summary })
    }

    fn stale_validated_base_candidate(&self) -> Result<Option<StaleValidatedBase>> {
        let Some(previous_base) = self.task.task.validated_base_commit.clone() else {
            return Ok(None);
        };
        ensure_clean_git(self.worktree, "Local Worktree")?;
        let worktree_branch = git_output(
            self.worktree,
            &["branch", "--show-current"],
            "read Local Worktree branch",
        )?;
        if worktree_branch.trim() != self.task_branch {
            return Ok(None);
        }
        let current_main = git_output(
            self.repo,
            &["rev-parse", &self.queue.main_branch],
            "read Main Branch commit",
        )?
        .trim()
        .to_string();
        if previous_base == current_main {
            return Ok(None);
        }
        if git_status(
            self.repo,
            &[
                "merge-base",
                "--is-ancestor",
                &self.queue.main_branch,
                self.task_branch,
            ],
        )?
        .success()
        {
            return Ok(None);
        }
        let task_commit_count: i64 = git_output(
            self.repo,
            &[
                "rev-list",
                "--count",
                &format!("{}..{}", self.queue.main_branch, self.task_branch),
            ],
            "count Task Commits",
        )?
        .trim()
        .parse()
        .context("failed to parse Task Commit count")?;
        if task_commit_count == 0 {
            return Ok(None);
        }
        Ok(Some(StaleValidatedBase {
            previous_base,
            current_main,
        }))
    }

    async fn auto_refresh_stale_validated_base(
        &self,
        stale: &StaleValidatedBase,
    ) -> Result<String> {
        let validation_commands = self.load_validation_commands()?;
        if validation_commands.is_empty() {
            anyhow::bail!(
                "auto-refresh declined: no validation command source found at {}",
                self.repo.join(VALIDATION_COMMANDS_FILE).display()
            );
        }
        if let Err(error) = run_git(
            self.worktree,
            &["rebase", &self.queue.main_branch],
            "auto-refresh Task Branch by rebasing onto Main Branch",
        ) {
            let _ = run_git(
                self.worktree,
                &["rebase", "--abort"],
                "abort auto-refresh rebase",
            );
            anyhow::bail!(
                "auto-refresh conflict while rebasing Task Branch {} from Validated Base Commit {} onto current Main Branch {}: {error:#}",
                self.task_branch,
                stale.previous_base,
                stale.current_main
            );
        }
        for command in &validation_commands {
            run_shell_command(self.worktree, command).with_context(|| {
                format!("auto-refresh validation failed for command `{command}`")
            })?;
        }
        tasker_db::record_task_validated_base_commit(
            self.pool,
            &self.task.task.identifier,
            &stale.current_main,
            self.actor,
        )
        .await?;
        Ok(format!(
            "auto-refreshed stale Validated Base Commit {} to {}",
            stale.previous_base, stale.current_main
        ))
    }

    fn load_validation_commands(&self) -> Result<Vec<String>> {
        let path = self.repo.join(VALIDATION_COMMANDS_FILE);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let text = std::fs::read_to_string(&path).with_context(|| {
            format!("failed to read validation commands from {}", path.display())
        })?;
        Ok(text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(ToString::to_string)
            .collect())
    }

    fn validate_operational_safety(&self) -> Result<()> {
        run_git(
            self.repo,
            &["rev-parse", "--show-toplevel"],
            "validate Managed Source Repository",
        )?;
        run_git(
            self.worktree,
            &["rev-parse", "--is-inside-work-tree"],
            "validate Local Worktree",
        )?;
        ensure_no_git_lock(self.repo)?;
        ensure_no_git_lock(self.worktree)?;
        let branch = git_output(
            self.repo,
            &["branch", "--show-current"],
            "read Managed Source Repository branch",
        )?;
        if branch.trim() != self.queue.main_branch {
            anyhow::bail!(
                "Managed Source Repository is on branch {}, expected Main Branch {}",
                branch.trim(),
                self.queue.main_branch
            );
        }
        ensure_clean_git(self.repo, "Managed Source Repository")?;
        let source_common_dir = git_common_dir(self.repo)?;
        let worktree_common_dir = git_common_dir(self.worktree)?;
        if source_common_dir != worktree_common_dir {
            anyhow::bail!(
                "Local Worktree is not attached to the configured Managed Source Repository"
            );
        }
        if !git_status(
            self.repo,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{}", self.task_branch),
            ],
        )?
        .success()
        {
            anyhow::bail!("Task Branch {} does not exist", self.task_branch);
        }
        Ok(())
    }

    fn validate_work_change_safety(&self) -> Result<()> {
        ensure_clean_git(self.worktree, "Local Worktree")?;
        let worktree_branch = git_output(
            self.worktree,
            &["branch", "--show-current"],
            "read Local Worktree branch",
        )?;
        if worktree_branch.trim() != self.task_branch {
            anyhow::bail!(
                "Local Worktree is on branch {}, expected Task Branch {}",
                worktree_branch.trim(),
                self.task_branch
            );
        }
        let current_main = git_output(
            self.repo,
            &["rev-parse", &self.queue.main_branch],
            "read Main Branch commit",
        )?
        .trim()
        .to_string();
        if !git_status(
            self.repo,
            &[
                "merge-base",
                "--is-ancestor",
                &self.queue.main_branch,
                self.task_branch,
            ],
        )?
        .success()
            && self.task.task.validated_base_commit.as_deref() != Some(current_main.as_str())
        {
            if self.task.task.validated_base_commit.is_some() {
                anyhow::bail!(
                    "Task Branch {} does not include current Main Branch {} and has stale Validated Base Commit {} (current Main Branch is {})",
                    self.task_branch,
                    self.queue.main_branch,
                    self.task
                        .task
                        .validated_base_commit
                        .as_deref()
                        .unwrap_or("not recorded"),
                    current_main
                );
            }
            anyhow::bail!(
                "Task Branch {} does not include current Main Branch {} and Validated Base Commit is missing (current Main Branch is {})",
                self.task_branch,
                self.queue.main_branch,
                current_main
            );
        }
        Ok(())
    }

    async fn record_outcome(
        &self,
        kind: &str,
        reason_code: &str,
        final_commit: Option<String>,
        pre_merge_head: Option<String>,
        message: Option<String>,
        retry: IntegrationOutcomeRetryFields,
    ) -> Result<()> {
        tasker_db::record_integration_outcome(
            self.pool,
            &tasker_db::RecordIntegrationOutcomeInput {
                task_identifier: self.task.task.identifier.clone(),
                agent_run_id: self.agent_run_id.map(ToString::to_string),
                outcome_kind: kind.to_string(),
                reason_code: reason_code.to_string(),
                final_commit,
                pre_merge_head,
                message,
                retryable: retry.retryable,
                retry_attempt: retry.retry_attempt,
                retry_delay_seconds: retry.retry_delay_seconds,
            },
            self.actor,
        )
        .await?;
        Ok(())
    }

    async fn next_retry_policy(&self) -> Result<IntegrationRetryPolicy> {
        let previous_attempt: Option<i64> = sqlx::query(
            r#"
            SELECT retry_attempt
            FROM integration_outcomes
            WHERE task_id = ?
              AND outcome_kind = 'operational_failure'
              AND retry_attempt IS NOT NULL
            ORDER BY created_at DESC, rowid DESC
            LIMIT 1
            "#,
        )
        .bind(&self.task.task.id)
        .fetch_optional(self.pool)
        .await
        .context("failed to load Integration retry attempt")?
        .and_then(|row| {
            row.try_get::<Option<i64>, _>("retry_attempt")
                .ok()
                .flatten()
        });
        let attempt = previous_attempt.unwrap_or(0) + 1;
        let retryable = attempt < INTEGRATION_RETRY_MAX_ATTEMPTS;
        let delay_seconds = if retryable {
            Some(INTEGRATION_RETRY_INITIAL_DELAY_SECONDS * (1_i64 << (attempt - 1)))
        } else {
            None
        };
        Ok(IntegrationRetryPolicy {
            attempt,
            retryable,
            delay_seconds,
        })
    }

    async fn transition(&self, to_state: &str) -> Result<()> {
        tasker_db::transition_task_state(
            self.pool,
            &self.task.task.identifier,
            &tasker_db::TransitionTaskState {
                to_state: to_state.to_string(),
                agent_run_id: None,
                repair_override: false,
            },
            self.actor,
        )
        .await?;
        Ok(())
    }

    fn rollback_to(&self, pre_merge_head: &str) -> Result<()> {
        let branch = git_output(
            self.repo,
            &["branch", "--show-current"],
            "read Managed Source Repository branch",
        )?;
        if branch.trim() != self.queue.main_branch {
            anyhow::bail!(
                "refusing rollback because Managed Source Repository is no longer on Main Branch"
            );
        }
        run_git(
            self.repo,
            &["reset", "--hard", pre_merge_head],
            "roll back Main Branch",
        )
    }

    fn cleanup_after_success(&self) -> Result<()> {
        if self.queue.done_worktree_retention {
            return Ok(());
        }
        if self.worktree.exists() {
            let worktree = self.worktree.display().to_string();
            run_git(
                self.repo,
                &["worktree", "remove", "--force", &worktree],
                "remove Local Worktree",
            )?;
        }
        if git_status(
            self.repo,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{}", self.task_branch),
            ],
        )?
        .success()
        {
            run_git(
                self.repo,
                &["branch", "-D", self.task_branch],
                "delete Task Branch",
            )?;
        }
        Ok(())
    }
}

fn ensure_clean_git(repo: &Path, label: &str) -> Result<()> {
    let status = git_output(repo, &["status", "--porcelain"], "check Git cleanliness")?;
    if !status.trim().is_empty() {
        anyhow::bail!("{label} has uncommitted changes");
    }
    Ok(())
}

fn ensure_no_git_lock(repo: &Path) -> Result<()> {
    let common_dir = git_common_dir(repo)?;
    if common_dir.join("index.lock").exists() {
        anyhow::bail!(
            "Git lock exists at {}",
            common_dir.join("index.lock").display()
        );
    }
    let git_dir = git_output(repo, &["rev-parse", "--git-dir"], "read Git dir")?;
    let git_dir = PathBuf::from(git_dir.trim());
    let git_dir = if git_dir.is_absolute() {
        git_dir
    } else {
        repo.join(git_dir)
    };
    if git_dir.join("index.lock").exists() {
        anyhow::bail!(
            "Git lock exists at {}",
            git_dir.join("index.lock").display()
        );
    }
    Ok(())
}

fn git_status(repo: &Path, args: &[&str]) -> Result<std::process::ExitStatus> {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .with_context(|| format!("failed to run git {:?} in {}", args, repo.display()))
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

fn run_git(repo: &Path, args: &[&str], action: &str) -> Result<()> {
    git_output(repo, args, action).map(|_| ())
}

fn run_shell_command(repo: &Path, command: &str) -> Result<()> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(repo)
        .output()
        .with_context(|| {
            format!(
                "failed to run validation command `{command}` in {}",
                repo.display()
            )
        })?;
    if !output.status.success() {
        anyhow::bail!(
            "validation command `{}` failed with status {}: {}{}{}",
            command,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim(),
            if output.stderr.is_empty() || output.stdout.is_empty() {
                ""
            } else {
                "\n"
            },
            String::from_utf8_lossy(&output.stdout).trim()
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
        .with_context(|| format!("failed to {action}: git {:?} in {}", args, repo.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "failed to {action}: git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::fs;

    #[tokio::test]
    async fn local_worktree_integration_squash_merges_and_cleans_successful_task() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let (repo, worktree) = seed_integrating_local_task(&pool, temp.path(), true, false).await;

        let result = integrate_local_worktree_for_run(
            &pool,
            "TASK-1",
            None,
            &tasker_db::Actor::operator("tester"),
            temp.path(),
        )
        .await
        .expect("integrate");

        assert!(result.summary.contains("Final Commit"));
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("task")
            .expect("task");
        assert_eq!(detail.task.state, "done");
        assert!(git_output(
            &repo,
            &["show", "--stat", "--oneline", "HEAD"],
            "show final commit"
        )
        .expect("show final commit")
        .contains("feature.txt"));
        let commit_message = git_output(
            &repo,
            &["log", "-1", "--pretty=%B"],
            "show final commit message",
        )
        .expect("show final commit message");
        assert!(commit_message.contains("Tasker-Task: TASK-1"));
        assert!(commit_message.contains("Tasker-Queue: TASK"));
        assert!(!commit_message.contains("Brief"));
        assert!(!commit_message.contains("Workpad"));
        assert_eq!(
            commit_metadata::parse_tasker_commit_trailers(&commit_message),
            commit_metadata::TaskerCommitTrailers {
                task_identifier: Some("TASK-1".to_string()),
                task_queue: Some("TASK".to_string()),
                agent_run_id: None,
            }
        );
        let message_file = temp.path().join("final-commit-message.txt");
        fs::write(&message_file, &commit_message).expect("message file");
        let parsed_by_git = git_output(
            &repo,
            &[
                "interpret-trailers",
                "--parse",
                "--no-divider",
                message_file.to_str().expect("utf8"),
            ],
            "parse trailers",
        )
        .expect("git interpret-trailers");
        assert!(parsed_by_git.contains("Tasker-Task: TASK-1"));
        assert!(parsed_by_git.contains("Tasker-Queue: TASK"));
        assert!(!worktree.exists());
        assert!(!Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                "refs/heads/tasker/TASK-1"
            ])
            .status()
            .expect("branch status")
            .success());
        let outcomes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM integration_outcomes WHERE outcome_kind = 'success' AND reason_code = 'success'",
        )
        .fetch_one(&pool)
        .await
        .expect("outcome count");
        assert_eq!(outcomes, 1);
    }

    #[tokio::test]
    async fn local_worktree_integration_records_no_change_without_commit() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let (repo, _worktree) = seed_integrating_local_task(&pool, temp.path(), false, false).await;
        let before = git_output(&repo, &["rev-parse", "HEAD"], "read HEAD").expect("head");

        let result = integrate_local_worktree_for_run(
            &pool,
            "TASK-1",
            None,
            &tasker_db::Actor::operator("tester"),
            temp.path(),
        )
        .await
        .expect("integrate");

        assert!(result.summary.contains("No-Change Integration"));
        let after = git_output(&repo, &["rev-parse", "HEAD"], "read HEAD").expect("head");
        assert_eq!(before, after);
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("task")
            .expect("task");
        assert_eq!(detail.task.state, "done");
        let outcomes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM integration_outcomes WHERE outcome_kind = 'no_changes' AND reason_code = 'no_changes'",
        )
        .fetch_one(&pool)
        .await
        .expect("outcome count");
        assert_eq!(outcomes, 1);
    }

    #[tokio::test]
    async fn local_worktree_integration_dirty_managed_source_repository_stays_integrating() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let (repo, _worktree) = seed_integrating_local_task(&pool, temp.path(), true, false).await;
        fs::write(repo.join("operator-scratch.txt"), "dirty\n").expect("dirty repo");

        let result = integrate_local_worktree_for_run(
            &pool,
            "TASK-1",
            None,
            &tasker_db::Actor::operator("tester"),
            temp.path(),
        )
        .await
        .expect("integrate");

        assert!(result.summary.contains("operational Delivery Failure"));
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("task")
            .expect("task");
        assert_eq!(detail.task.state, "integrating");
        let outcomes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM integration_outcomes WHERE outcome_kind = 'operational_failure' AND reason_code = 'dirty_managed_source_repository'",
        )
        .fetch_one(&pool)
        .await
        .expect("outcome count");
        assert_eq!(outcomes, 1);
    }

    #[tokio::test]
    async fn local_worktree_integration_dirty_local_worktree_moves_to_rework() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let (_repo, worktree) = seed_integrating_local_task(&pool, temp.path(), true, false).await;
        fs::write(worktree.join("dirty.txt"), "dirty\n").expect("dirty worktree");

        let result = integrate_local_worktree_for_run(
            &pool,
            "TASK-1",
            None,
            &tasker_db::Actor::operator("tester"),
            temp.path(),
        )
        .await
        .expect("integrate");

        assert!(result.summary.contains("work-change Delivery Failure"));
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("task")
            .expect("task");
        assert_eq!(detail.task.state, "rework");
        let outcomes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM integration_outcomes WHERE outcome_kind = 'work_change_failure' AND reason_code = 'uncommitted_local_worktree'",
        )
        .fetch_one(&pool)
        .await
        .expect("outcome count");
        assert_eq!(outcomes, 1);
    }

    #[tokio::test]
    async fn local_worktree_integration_allows_current_validated_base_without_branch_ancestry() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let (repo, _worktree) = seed_integrating_local_task(&pool, temp.path(), true, false).await;
        fs::write(repo.join("main-only.txt"), "main moved\n").expect("main change");
        git(&repo, &["add", "main-only.txt"]);
        git(&repo, &["commit", "-m", "move main"]);
        let current_main = git_output(&repo, &["rev-parse", "main"], "read main head")
            .expect("main head")
            .trim()
            .to_string();
        tasker_db::update_validation_item_status(
            &pool,
            "TASK-1",
            1,
            &tasker_db::UpdateRequirementStatus {
                status: "passed".to_string(),
                waiver_reason: None,
                validated_base_commit: Some(current_main.clone()),
            },
            &tasker_db::Actor::operator("tester"),
        )
        .await
        .expect("record validated base");

        let result = integrate_local_worktree_for_run(
            &pool,
            "TASK-1",
            None,
            &tasker_db::Actor::operator("tester"),
            temp.path(),
        )
        .await
        .expect("integrate");

        assert!(result.summary.contains("Final Commit"));
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("task")
            .expect("task");
        assert_eq!(detail.task.state, "done");
        assert_eq!(
            detail.task.validated_base_commit.as_deref(),
            Some(current_main.as_str())
        );
    }

    #[tokio::test]
    async fn local_worktree_integration_stale_validated_base_auto_refresh_succeeds_without_agent_run(
    ) {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let (repo, _worktree) = seed_integrating_local_task(&pool, temp.path(), true, false).await;
        let old_main = git_output(&repo, &["rev-parse", "main"], "read main head")
            .expect("main head")
            .trim()
            .to_string();
        tasker_db::update_validation_item_status(
            &pool,
            "TASK-1",
            1,
            &tasker_db::UpdateRequirementStatus {
                status: "passed".to_string(),
                waiver_reason: None,
                validated_base_commit: Some(old_main),
            },
            &tasker_db::Actor::operator("tester"),
        )
        .await
        .expect("record stale base");
        fs::create_dir_all(repo.join(".tasker")).expect("tasker dir");
        fs::write(
            repo.join(".tasker/validation-commands.txt"),
            "test -f feature.txt\n",
        )
        .expect("validation commands");
        fs::write(repo.join("main-only.txt"), "main moved\n").expect("main change");
        git(
            &repo,
            &["add", ".tasker/validation-commands.txt", "main-only.txt"],
        );
        git(&repo, &["commit", "-m", "move main"]);
        let current_main = git_output(&repo, &["rev-parse", "main"], "read main head")
            .expect("main head")
            .trim()
            .to_string();
        let runs_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs")
            .fetch_one(&pool)
            .await
            .expect("run count before");

        let result = integrate_local_worktree_for_run(
            &pool,
            "TASK-1",
            None,
            &tasker_db::Actor::operator("tester"),
            temp.path(),
        )
        .await
        .expect("integrate");

        assert!(result.summary.contains("Final Commit"));
        let runs_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs")
            .fetch_one(&pool)
            .await
            .expect("run count after");
        assert_eq!(runs_before, runs_after);
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("task")
            .expect("task");
        assert_eq!(detail.task.state, "done");
        assert_eq!(
            detail.task.validated_base_commit.as_deref(),
            Some(current_main.as_str())
        );
        let reason_code: String = sqlx::query_scalar(
        "SELECT reason_code FROM integration_outcomes ORDER BY created_at DESC, rowid DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("reason code");
        assert_eq!(reason_code, "auto_refresh_success");
    }

    #[tokio::test]
    async fn local_worktree_integration_stale_validated_base_conflict_moves_to_rework() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let (repo, _worktree) = seed_integrating_local_task(&pool, temp.path(), true, false).await;
        record_current_main_as_stale_base(&pool, &repo).await;
        fs::create_dir_all(repo.join(".tasker")).expect("tasker dir");
        fs::write(repo.join(".tasker/validation-commands.txt"), "true\n").expect("commands");
        fs::write(repo.join("feature.txt"), "main feature\n").expect("conflict");
        git(
            &repo,
            &["add", ".tasker/validation-commands.txt", "feature.txt"],
        );
        git(&repo, &["commit", "-m", "conflict on main"]);

        let result = integrate_local_worktree_for_run(
            &pool,
            "TASK-1",
            None,
            &tasker_db::Actor::operator("tester"),
            temp.path(),
        )
        .await
        .expect("integrate");

        assert!(result.summary.contains("auto-refresh conflict"));
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("task")
            .expect("task");
        assert_eq!(detail.task.state, "rework");
        let reason_code: String = sqlx::query_scalar(
        "SELECT reason_code FROM integration_outcomes ORDER BY created_at DESC, rowid DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("reason code");
        assert_eq!(reason_code, "auto_refresh_conflict");
    }

    #[tokio::test]
    async fn local_worktree_integration_stale_validated_base_validation_failure_moves_to_rework() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let (repo, _worktree) = seed_integrating_local_task(&pool, temp.path(), true, false).await;
        record_current_main_as_stale_base(&pool, &repo).await;
        fs::create_dir_all(repo.join(".tasker")).expect("tasker dir");
        fs::write(repo.join(".tasker/validation-commands.txt"), "false\n").expect("commands");
        fs::write(repo.join("main-only.txt"), "main moved\n").expect("main change");
        git(
            &repo,
            &["add", ".tasker/validation-commands.txt", "main-only.txt"],
        );
        git(&repo, &["commit", "-m", "move main"]);

        let result = integrate_local_worktree_for_run(
            &pool,
            "TASK-1",
            None,
            &tasker_db::Actor::operator("tester"),
            temp.path(),
        )
        .await
        .expect("integrate");

        assert!(result.summary.contains("auto-refresh validation failed"));
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("task")
            .expect("task");
        assert_eq!(detail.task.state, "rework");
        let reason_code: String = sqlx::query_scalar(
        "SELECT reason_code FROM integration_outcomes ORDER BY created_at DESC, rowid DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("reason code");
        assert_eq!(reason_code, "auto_refresh_validation_failed");
    }

    #[tokio::test]
    async fn local_worktree_integration_stale_validated_base_missing_validation_declines_to_rework()
    {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let (repo, _worktree) = seed_integrating_local_task(&pool, temp.path(), true, false).await;
        record_current_main_as_stale_base(&pool, &repo).await;
        fs::write(repo.join("main-only.txt"), "main moved\n").expect("main change");
        git(&repo, &["add", "main-only.txt"]);
        git(&repo, &["commit", "-m", "move main"]);

        let result = integrate_local_worktree_for_run(
            &pool,
            "TASK-1",
            None,
            &tasker_db::Actor::operator("tester"),
            temp.path(),
        )
        .await
        .expect("integrate");

        assert!(result.summary.contains("no validation command source"));
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("task")
            .expect("task");
        assert_eq!(detail.task.state, "rework");
        let reason_code: String = sqlx::query_scalar(
        "SELECT reason_code FROM integration_outcomes ORDER BY created_at DESC, rowid DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("reason code");
        assert_eq!(reason_code, "auto_refresh_declined_missing_validation");
    }

    #[tokio::test]
    async fn local_worktree_integration_stale_validated_base_dirty_worktree_skips_auto_refresh() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let (repo, worktree) = seed_integrating_local_task(&pool, temp.path(), true, false).await;
        record_current_main_as_stale_base(&pool, &repo).await;
        fs::create_dir_all(repo.join(".tasker")).expect("tasker dir");
        fs::write(repo.join(".tasker/validation-commands.txt"), "true\n").expect("commands");
        fs::write(repo.join("main-only.txt"), "main moved\n").expect("main change");
        git(
            &repo,
            &["add", ".tasker/validation-commands.txt", "main-only.txt"],
        );
        git(&repo, &["commit", "-m", "move main"]);
        fs::write(worktree.join("dirty.txt"), "dirty\n").expect("dirty worktree");

        let result = integrate_local_worktree_for_run(
            &pool,
            "TASK-1",
            None,
            &tasker_db::Actor::operator("tester"),
            temp.path(),
        )
        .await
        .expect("integrate");

        assert!(result
            .summary
            .contains("Local Worktree has uncommitted changes"));
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("task")
            .expect("task");
        assert_eq!(detail.task.state, "rework");
        assert!(worktree.join("dirty.txt").exists());
        let reason_code: String = sqlx::query_scalar(
        "SELECT reason_code FROM integration_outcomes ORDER BY created_at DESC, rowid DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("reason code");
        assert_eq!(reason_code, "uncommitted_local_worktree");
    }

    #[tokio::test]
    async fn local_worktree_integration_stale_task_branch_moves_to_rework() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let (repo, _worktree) = seed_integrating_local_task(&pool, temp.path(), true, false).await;
        fs::write(repo.join("main-only.txt"), "main moved\n").expect("main change");
        git(&repo, &["add", "main-only.txt"]);
        git(&repo, &["commit", "-m", "move main"]);

        let result = integrate_local_worktree_for_run(
            &pool,
            "TASK-1",
            None,
            &tasker_db::Actor::operator("tester"),
            temp.path(),
        )
        .await
        .expect("integrate");

        assert!(result.summary.contains("work-change Delivery Failure"));
        assert!(result
            .summary
            .contains("does not include current Main Branch"));
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("task")
            .expect("task");
        assert_eq!(detail.task.state, "rework");
        let reason_code: String = sqlx::query_scalar(
        "SELECT reason_code FROM integration_outcomes ORDER BY created_at DESC, rowid DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("reason code");
        assert_eq!(reason_code, "task_branch_missing_main");
    }

    #[tokio::test]
    async fn local_worktree_integration_commit_failure_rolls_back_main_branch() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let (repo, _worktree) = seed_integrating_local_task(&pool, temp.path(), true, false).await;
        let pre_head = git_output(&repo, &["rev-parse", "HEAD"], "read HEAD").expect("head");
        let hooks = repo.join(".git/hooks");
        fs::write(hooks.join("pre-commit"), "#!/bin/sh\nexit 1\n").expect("hook");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(hooks.join("pre-commit"))
                .expect("hook metadata")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(hooks.join("pre-commit"), permissions).expect("chmod hook");
        }

        let result = integrate_local_worktree_for_run(
            &pool,
            "TASK-1",
            None,
            &tasker_db::Actor::operator("tester"),
            temp.path(),
        )
        .await
        .expect("integrate");

        assert!(result.summary.contains("Final Commit failed"));
        let after_head = git_output(&repo, &["rev-parse", "HEAD"], "read HEAD").expect("head");
        assert_eq!(pre_head, after_head);
        assert!(git_output(&repo, &["status", "--porcelain"], "status")
            .expect("status")
            .trim()
            .is_empty());
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("task")
            .expect("task");
        assert_eq!(detail.task.state, "rework");
    }

    async fn seed_integrating_local_task(
        pool: &SqlitePool,
        root: &Path,
        with_feature_commit: bool,
        done_worktree_retention: bool,
    ) -> (PathBuf, PathBuf) {
        let repo = root.join("repo");
        let worktrees = root.join("worktrees");
        let worktree = worktrees.join("TASK-1");
        init_git_repo(&repo);
        tasker_db::create_task_queue(
            pool,
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
        .expect("queue");
        tasker_db::create_task(
            pool,
            &tasker_db::CreateTask {
                queue_key: "TASK".to_string(),
                title: "Integrate me".to_string(),
                brief: "Brief".to_string(),
                priority: "normal".to_string(),
                state: "ready".to_string(),
                review_required: false,
                acceptance_criteria: vec!["accepted".to_string()],
                validation_items: vec!["validated".to_string()],
                tags: vec![],
                conflict_hints: vec![],
                blocking_task_identifiers: vec![],
            },
            &tasker_db::Actor::operator("tester"),
        )
        .await
        .expect("task");
        run_git(
            &repo,
            &["branch", "tasker/TASK-1", "main"],
            "create task branch",
        )
        .expect("branch");
        fs::create_dir_all(&worktrees).expect("worktrees");
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                worktree.to_str().expect("utf8"),
                "tasker/TASK-1",
            ],
            "add worktree",
        )
        .expect("worktree");
        if with_feature_commit {
            fs::write(worktree.join("feature.txt"), "feature\n").expect("feature");
            run_git(&worktree, &["add", "feature.txt"], "add feature").expect("add feature");
            run_git(
                &worktree,
                &["commit", "-m", "add feature"],
                "commit feature",
            )
            .expect("commit feature");
        }
        let actor = tasker_db::Actor::operator("tester");
        tasker_db::upsert_task_link(
            pool,
            "TASK-1",
            &tasker_db::UpsertTaskLink {
                kind: "local_worktree".to_string(),
                target: worktree.display().to_string(),
                label: Some("Local Worktree".to_string()),
                is_primary: true,
            },
            &actor,
        )
        .await
        .expect("worktree link");
        tasker_db::upsert_task_link(
            pool,
            "TASK-1",
            &tasker_db::UpsertTaskLink {
                kind: "task_branch".to_string(),
                target: "tasker/TASK-1".to_string(),
                label: Some("Task Branch".to_string()),
                is_primary: false,
            },
            &actor,
        )
        .await
        .expect("branch link");
        tasker_db::update_acceptance_criterion_status(
            pool,
            "TASK-1",
            1,
            &tasker_db::UpdateRequirementStatus {
                status: "satisfied".to_string(),
                waiver_reason: None,
                validated_base_commit: None,
            },
            &actor,
        )
        .await
        .expect("criterion");
        tasker_db::update_validation_item_status(
            pool,
            "TASK-1",
            1,
            &tasker_db::UpdateRequirementStatus {
                status: "passed".to_string(),
                waiver_reason: None,
                validated_base_commit: None,
            },
            &actor,
        )
        .await
        .expect("validation");
        tasker_db::transition_task_state(
            pool,
            "TASK-1",
            &tasker_db::TransitionTaskState {
                to_state: "in_progress".to_string(),
                agent_run_id: None,
                repair_override: false,
            },
            &actor,
        )
        .await
        .expect("in progress");
        tasker_db::transition_task_state(
            pool,
            "TASK-1",
            &tasker_db::TransitionTaskState {
                to_state: "integrating".to_string(),
                agent_run_id: None,
                repair_override: false,
            },
            &actor,
        )
        .await
        .expect("integrating");
        (repo, worktree)
    }

    async fn record_current_main_as_stale_base(pool: &SqlitePool, repo: &Path) {
        let old_main = git_output(repo, &["rev-parse", "main"], "read main head")
            .expect("main head")
            .trim()
            .to_string();
        tasker_db::update_validation_item_status(
            pool,
            "TASK-1",
            1,
            &tasker_db::UpdateRequirementStatus {
                status: "passed".to_string(),
                waiver_reason: None,
                validated_base_commit: Some(old_main),
            },
            &tasker_db::Actor::operator("tester"),
        )
        .await
        .expect("record stale base");
    }

    fn git(repo: &Path, args: &[&str]) {
        run_git(repo, args, "test git command").expect("git command");
    }

    fn init_git_repo(repo: &Path) {
        fs::create_dir_all(repo).expect("repo dir");
        run_git(repo, &["init", "-b", "main"], "init repo").expect("git init");
        run_git(
            repo,
            &["config", "user.email", "tasker@example.test"],
            "config email",
        )
        .expect("email");
        run_git(repo, &["config", "user.name", "Tasker Test"], "config name").expect("name");
        fs::write(repo.join("README.md"), "test repo\n").expect("readme");
        run_git(repo, &["add", "README.md"], "add readme").expect("add readme");
        run_git(repo, &["commit", "-m", "initial"], "initial commit").expect("initial commit");
    }
}
