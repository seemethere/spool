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
    let _repo_operation_lock = tasker_runner::repo_lock::acquire_guard(
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
