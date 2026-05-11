use super::*;
use crate::status_cmd::integration_recovery_hint;
use tasker_runner::local_worktree_delivery;
use tasker_runner::repo_lock;

pub(crate) async fn merge(
    paths: &TaskerPaths,
    db_path_overridden: bool,
    command: MergeCommand,
) -> Result<()> {
    let pool = open_pool(paths, db_path_overridden).await?;
    match command {
        MergeCommand::Queue { queue } => {
            let rows = tasker_db::merge_queue_tasks(&pool, queue.as_deref()).await?;
            print_manual_merge_queue(&rows);
        }
        MergeCommand::Inspect { identifier } => {
            let detail = tasker_db::get_task_detail(&pool, &identifier)
                .await?
                .with_context(|| format!("Task {identifier} not found"))?;
            let queue = tasker_db::get_task_queue(&pool, &detail.task.task_queue_key)
                .await?
                .with_context(|| format!("Task Queue {} not found", detail.task.task_queue_key))?;
            let latest_run =
                tasker_db::get_latest_agent_run_detail_for_task(&pool, &identifier).await?;
            let latest_outcome = latest_integration_outcome_for_task(&pool, &identifier).await?;
            print_manual_merge_inspection(
                &detail,
                &queue,
                latest_run.as_ref(),
                latest_outcome.as_ref(),
            );
        }
        MergeCommand::Lock { command } => match command {
            MergeLockCommand::Acquire {
                queue,
                operation,
                task,
            } => {
                let active =
                    repo_lock::acquire_repo_operation_lock(repo_lock::AcquireRepoOperationLock {
                        data_dir: paths.data_dir.clone(),
                        queue,
                        operation,
                        task_identifier: task,
                    })?;
                println!(
                    "acquired Managed Source Repository operation lock for Task Queue {} at {}",
                    active.lock.queue,
                    active.path.display()
                );
                println!(
                    "release after operator verification: tasker merge lock release --queue {}",
                    active.lock.queue
                );
            }
            MergeLockCommand::Status { queue } => {
                if let Some(active) =
                    repo_lock::show_repo_operation_lock(repo_lock::ShowRepoOperationLock {
                        data_dir: paths.data_dir.clone(),
                        queue: queue.clone(),
                    })?
                {
                    println!("{}", repo_lock::blocked_message(&active));
                } else {
                    println!("no Managed Source Repository operation lock for Task Queue {queue}");
                }
            }
            MergeLockCommand::Release { queue } => {
                if let Some(active) =
                    repo_lock::release_repo_operation_lock(repo_lock::ReleaseRepoOperationLock {
                        data_dir: paths.data_dir.clone(),
                        queue: queue.clone(),
                    })?
                {
                    println!(
                        "released Managed Source Repository operation lock for Task Queue {} from {}",
                        active.lock.queue,
                        active.path.display()
                    );
                } else {
                    println!("no Managed Source Repository operation lock for Task Queue {queue}");
                }
            }
        },
        MergeCommand::Integrate { identifier, actor } => {
            let actor = tasker_db::Actor::operator(actor);
            let outcome =
                integrate_local_worktree(&pool, &identifier, &actor, &paths.data_dir).await?;
            println!("{}", outcome.summary);
        }
        MergeCommand::Retry {
            identifier,
            force,
            actor,
        } => {
            let actor = tasker_db::Actor::operator(actor);
            let outcome = retry_local_worktree_integration(
                &pool,
                &identifier,
                force,
                &actor,
                &paths.data_dir,
            )
            .await?;
            println!("{}", outcome.summary);
        }
        MergeCommand::Done {
            identifier,
            manual,
            actor,
        } => {
            if !manual {
                anyhow::bail!(
                    "refusing to mark Task Done without --manual confirmation that the Local Merge was performed outside Tasker"
                );
            }
            let current = tasker_db::get_task_detail(&pool, &identifier)
                .await?
                .with_context(|| format!("Task {identifier} not found"))?;
            if current.task.state != "integrating" {
                anyhow::bail!(
                    "Manual Dogfood Merge completion requires Task State integrating; current state is {}",
                    current.task.state
                );
            }
            let detail = tasker_db::transition_task_state(
                &pool,
                &identifier,
                &tasker_db::TransitionTaskState {
                    to_state: "done".to_string(),
                    agent_run_id: None,
                    repair_override: false,
                },
                &tasker_db::Actor::operator(actor),
            )
            .await?;
            println!(
                "marked manually merged Task {} Done",
                detail.task.identifier
            );
        }
    }
    Ok(())
}

pub(crate) async fn integrate_local_worktree(
    pool: &sqlx::SqlitePool,
    identifier: &str,
    actor: &tasker_db::Actor,
    data_dir: &Path,
) -> Result<local_worktree_delivery::LocalIntegrationResult> {
    let latest_run = tasker_db::get_latest_agent_run_detail_for_task(pool, identifier).await?;
    let agent_run_id = latest_run.as_ref().map(|run| run.run.id.as_str());
    local_worktree_delivery::integrate_local_worktree_for_run(
        pool,
        identifier,
        agent_run_id,
        actor,
        data_dir,
    )
    .await
}

async fn retry_local_worktree_integration(
    pool: &sqlx::SqlitePool,
    identifier: &str,
    force: bool,
    actor: &tasker_db::Actor,
    data_dir: &Path,
) -> Result<local_worktree_delivery::LocalIntegrationResult> {
    let detail = tasker_db::get_task_detail(pool, identifier)
        .await?
        .with_context(|| format!("Task {identifier} not found"))?;
    if detail.task.state != "integrating" {
        if !force {
            anyhow::bail!(
                "Integration retry requires Task State integrating; current state is {}; use --force only after operator verification",
                detail.task.state
            );
        }
        tasker_db::transition_task_state(
            pool,
            identifier,
            &tasker_db::TransitionTaskState {
                to_state: "integrating".to_string(),
                agent_run_id: None,
                repair_override: false,
            },
            actor,
        )
        .await
        .with_context(|| {
            format!(
                "forced Integration retry could not move Task {identifier} from {} to Integrating",
                detail.task.state
            )
        })?;
    }

    if !force {
        match latest_integration_outcome_for_task(pool, identifier).await? {
            Some(LatestIntegrationOutcome {
                outcome_kind,
                retryable: true,
                ..
            }) if outcome_kind == "operational_failure" => {}
            Some(LatestIntegrationOutcome {
                outcome_kind,
                retryable: false,
                ..
            }) if outcome_kind == "operational_failure" => anyhow::bail!(
                "refusing Integration retry for Task {identifier}: latest operational_failure is no longer retryable; use --force only after operator verification"
            ),
            Some(LatestIntegrationOutcome { outcome_kind, .. })
                if outcome_kind == "work_change_failure" =>
            {
                anyhow::bail!(
                    "refusing Integration retry for Task {identifier}: latest Integration Outcome is work_change_failure and requires Rework; use --force only after operator verification"
                )
            }
            Some(LatestIntegrationOutcome { outcome_kind, .. }) => anyhow::bail!(
                "refusing Integration retry for Task {identifier}: latest Integration Outcome is {outcome_kind}, not a retryable operational_failure; use --force only after operator verification"
            ),
            None => anyhow::bail!(
                "refusing Integration retry for Task {identifier}: no previous Integration Outcome found; use `tasker merge integrate` for a first integration attempt"
            ),
        }
    }

    let mut outcome = integrate_local_worktree(pool, identifier, actor, data_dir).await?;
    outcome.summary = format!(
        "retried Integration for Task {identifier} without launching a new Agent Run: {}",
        outcome.summary
    );
    Ok(outcome)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LatestIntegrationOutcome {
    outcome_kind: String,
    reason_code: String,
    retryable: bool,
    message: Option<String>,
}

async fn latest_integration_outcome_for_task(
    pool: &sqlx::SqlitePool,
    identifier: &str,
) -> Result<Option<LatestIntegrationOutcome>> {
    let row = sqlx::query_as::<_, (String, String, bool, Option<String>)>(
        r#"
        SELECT integration_outcomes.outcome_kind,
               COALESCE(integration_outcomes.reason_code, 'unknown_legacy') AS reason_code,
               integration_outcomes.retryable,
               integration_outcomes.message
        FROM integration_outcomes
        JOIN tasks ON tasks.id = integration_outcomes.task_id
        WHERE tasks.identifier = ?
        ORDER BY integration_outcomes.created_at DESC, integration_outcomes.rowid DESC
        LIMIT 1
        "#,
    )
    .bind(identifier)
    .fetch_optional(pool)
    .await
    .context("failed to load latest Integration Outcome")?;
    Ok(row.map(
        |(outcome_kind, reason_code, retryable, message)| LatestIntegrationOutcome {
            outcome_kind,
            reason_code,
            retryable,
            message,
        },
    ))
}

pub(crate) async fn validation_base_commit_for_status(
    pool: &sqlx::SqlitePool,
    identifier: &str,
    status: &str,
    provided: Option<String>,
) -> Result<Option<String>> {
    if status != "passed" {
        return Ok(None);
    }
    if let Some(commit) = provided {
        let commit = commit.trim().to_string();
        if !commit.is_empty() {
            return Ok(Some(commit));
        }
    }

    let detail = tasker_db::get_task_detail(pool, identifier)
        .await?
        .with_context(|| format!("Task {identifier} not found"))?;
    let queue = tasker_db::get_task_queue(pool, &detail.task.task_queue_key)
        .await?
        .with_context(|| format!("Task Queue {} not found", detail.task.task_queue_key))?;
    let commit = git_output(
        Path::new(&queue.managed_source_repository),
        &["rev-parse", &queue.main_branch],
    )?
    .trim()
    .to_string();
    Ok(Some(commit))
}

pub(crate) async fn preflight_integrating_transition(
    pool: &sqlx::SqlitePool,
    identifier: &str,
    actor: &tasker_db::Actor,
) -> Result<Option<String>> {
    let detail = tasker_db::get_task_detail(pool, identifier)
        .await?
        .with_context(|| format!("Task {identifier} not found"))?;
    let queue = tasker_db::get_task_queue(pool, &detail.task.task_queue_key)
        .await?
        .with_context(|| format!("Task Queue {} not found", detail.task.task_queue_key))?;
    if queue.delivery_backend != "local_worktree" {
        return Ok(None);
    }

    let inspection = inspect_pre_integrating_local_worktree(&detail);
    if actor.kind == "worker_agent" {
        inspection.reject_if_not_ready()?;
        Ok(None)
    } else {
        Ok(inspection.operator_warning())
    }
}

#[derive(Debug, Clone)]
struct PreIntegratingLocalWorktreeInspection {
    identifier: String,
    local_worktree: Option<String>,
    task_branch: Option<String>,
    checked_out_branch: Option<String>,
    status_summary: Option<String>,
    issue: Option<String>,
}

impl PreIntegratingLocalWorktreeInspection {
    fn reject_if_not_ready(&self) -> Result<()> {
        if let Some(issue) = &self.issue {
            anyhow::bail!("{}", self.guidance(issue));
        }
        Ok(())
    }

    fn operator_warning(&self) -> Option<String> {
        self.issue.as_ref().map(|issue| {
            format!(
                "{}; operator transition may continue for repair flexibility, but Worker Agents must commit intended changes on the Task Branch and verify a clean Local Worktree before requesting Integrating",
                self.guidance(issue)
            )
        })
    }

    fn guidance(&self, issue: &str) -> String {
        format!(
            "Local Worktree pre-Integrating check failed for Task {}: {issue}. Local Worktree: {}; Task Branch: {}; git status summary: {}. Commit intended changes on the Task Branch, verify the Local Worktree is clean, then request Integrating again.",
            self.identifier,
            self.local_worktree.as_deref().unwrap_or("missing Local Worktree Task Link"),
            self.task_branch.as_deref().unwrap_or("missing Task Branch Task Link"),
            self.status_summary.as_deref().unwrap_or("unavailable"),
        )
    }
}

fn inspect_pre_integrating_local_worktree(
    detail: &tasker_db::TaskDetail,
) -> PreIntegratingLocalWorktreeInspection {
    let local_worktree = detail
        .task_links
        .iter()
        .find(|link| link.kind == "local_worktree")
        .map(|link| link.target.clone());
    let task_branch = detail
        .task_links
        .iter()
        .find(|link| link.kind == "task_branch")
        .map(|link| link.target.clone());

    let mut inspection = PreIntegratingLocalWorktreeInspection {
        identifier: detail.task.identifier.clone(),
        local_worktree,
        task_branch,
        checked_out_branch: None,
        status_summary: None,
        issue: None,
    };

    let Some(local_worktree) = inspection.local_worktree.as_deref() else {
        inspection.issue = Some("missing Local Worktree Task Link".to_string());
        return inspection;
    };
    if inspection.task_branch.is_none() {
        inspection.issue = Some("missing Task Branch Task Link".to_string());
        return inspection;
    }

    let worktree = Path::new(local_worktree);
    if !worktree.exists() {
        inspection.issue = Some("Local Worktree path does not exist".to_string());
        return inspection;
    }

    match git_output(worktree, &["status", "--porcelain"]) {
        Ok(status) => {
            inspection.status_summary = Some(condense_git_status_summary(&status));
            if !status.trim().is_empty() {
                inspection.issue = Some("Local Worktree has uncommitted changes".to_string());
            }
        }
        Err(error) => {
            inspection.issue = Some(format!(
                "could not inspect Local Worktree git status: {error:#}"
            ));
            return inspection;
        }
    }

    match git_output(worktree, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        Ok(branch) => {
            let branch = branch.trim().to_string();
            inspection.checked_out_branch = Some(branch.clone());
            if let Some(expected) = inspection.task_branch.as_deref() {
                if branch != expected {
                    inspection.issue = Some(format!(
                        "Local Worktree is on branch {branch}, expected Task Branch {expected}"
                    ));
                }
            }
        }
        Err(error) => {
            inspection.issue = Some(format!(
                "could not inspect Local Worktree branch: {error:#}"
            ));
        }
    }

    inspection
}

fn condense_git_status_summary(status: &str) -> String {
    let mut lines = status.lines();
    let shown = lines
        .by_ref()
        .take(12)
        .map(str::trim_end)
        .collect::<Vec<_>>();
    if shown.is_empty() {
        "clean".to_string()
    } else {
        let remaining = lines.count();
        let mut summary = shown.join("; ");
        if remaining > 0 {
            summary.push_str(&format!("; ... and {remaining} more"));
        }
        summary
    }
}

fn print_manual_merge_queue(rows: &[tasker_db::MergeQueueTask]) {
    println!("Manual Dogfood Merge queue");
    println!("temporary helper: read-only view for Integrating Tasks; Git operations remain operator-side; Tasker Service performs no Git mutations");
    println!("Tasks: {}", rows.len());
    if rows.is_empty() {
        println!("(none)");
        return;
    }
    println!();
    for row in rows {
        let git = inspect_merge_queue_git(
            row.local_worktree.as_deref(),
            row.task_branch.as_deref(),
            &row.main_branch,
        );
        let gates_ready = row.pending_acceptance_criteria == 0
            && row.pending_validation_items == 0
            && row.failed_validation_items == 0;
        let has_task_commit = git.task_commits.unwrap_or(false);
        let clean = git.clean.unwrap_or(false);
        let ready = gates_ready && clean && has_task_commit;
        println!(
            "{} [{}] {}",
            row.task_identifier,
            if ready { "ready" } else { "attention" },
            row.title
        );
        println!("  Task Queue: {}", row.queue_key);
        println!(
            "  Task Branch: {}",
            row.task_branch.as_deref().unwrap_or("missing Task Link")
        );
        println!(
            "  Local Worktree: {}",
            row.local_worktree.as_deref().unwrap_or("missing Task Link")
        );
        println!("  Main Branch: {}", row.main_branch);
        println!(
            "  Latest Agent Run: {} ({})",
            row.latest_agent_run_id.as_deref().unwrap_or("none"),
            row.latest_agent_run_outcome.as_deref().unwrap_or("none")
        );
        println!(
            "  Structured gates: {} pending Acceptance Criteria, {} pending Validation Items, {} failed Validation Items",
            row.pending_acceptance_criteria, row.pending_validation_items, row.failed_validation_items
        );
        println!("  Local Worktree clean: {}", git.label(git.clean));
        println!("  Task Commits present: {}", git.label(git.task_commits));
        println!(
            "  Merge inspection readiness: {}",
            if ready {
                "clean and gate-satisfied"
            } else {
                "operator attention needed"
            }
        );
        if let Some(warning) = git.warning {
            println!("  Attention: {warning}");
        }
        println!("  Detail: tasker merge inspect {}", row.task_identifier);
        println!();
    }
}

#[derive(Debug, Default)]
struct MergeQueueGitInspection {
    clean: Option<bool>,
    task_commits: Option<bool>,
    warning: Option<String>,
}

impl MergeQueueGitInspection {
    fn label(&self, value: Option<bool>) -> &'static str {
        match value {
            Some(true) => "yes",
            Some(false) => "no",
            None => "unknown",
        }
    }
}

fn inspect_merge_queue_git(
    local_worktree: Option<&str>,
    task_branch: Option<&str>,
    main_branch: &str,
) -> MergeQueueGitInspection {
    let Some(local_worktree) = local_worktree else {
        return MergeQueueGitInspection {
            warning: Some("missing Local Worktree Task Link".to_string()),
            ..MergeQueueGitInspection::default()
        };
    };
    let worktree = Path::new(local_worktree);
    if !worktree.exists() {
        return MergeQueueGitInspection {
            warning: Some("Local Worktree path does not exist".to_string()),
            ..MergeQueueGitInspection::default()
        };
    }

    let clean = git_output(worktree, &["status", "--porcelain"])
        .ok()
        .map(|status| status.trim().is_empty());
    let checked_out_branch = git_output(worktree, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .map(|branch| branch.trim().to_string());
    let warning = match (checked_out_branch.as_deref(), task_branch) {
        (Some(actual), Some(expected)) if actual != expected => Some(format!(
            "checked-out branch {actual} differs from Task Branch {expected}"
        )),
        _ => None,
    };
    let commits = format!("{main_branch}..HEAD");
    let task_commits = git_output(worktree, &["log", "--oneline", &commits])
        .ok()
        .map(|log| !log.trim().is_empty());

    MergeQueueGitInspection {
        clean,
        task_commits,
        warning,
    }
}

fn print_manual_merge_inspection(
    detail: &tasker_db::TaskDetail,
    queue: &tasker_db::TaskQueue,
    latest_run: Option<&tasker_db::AgentRunDetail>,
    latest_outcome: Option<&LatestIntegrationOutcome>,
) {
    let local_worktree = detail
        .task_links
        .iter()
        .find(|link| link.kind == "local_worktree")
        .map(|link| link.target.as_str());
    let task_branch = detail
        .task_links
        .iter()
        .find(|link| link.kind == "task_branch")
        .map(|link| link.target.as_str());

    println!("Manual Dogfood Merge inspection plan");
    println!("temporary helper: Git operations remain operator-side; Tasker Service performs no Git mutations");
    println!();
    println!("Task: {}", detail.task.identifier);
    println!("title: {}", detail.task.title);
    println!("Task State: {}", detail.task.state);
    println!("Task Queue: {}", detail.task.task_queue_key);
    println!(
        "Managed Source Repository: {}",
        queue.managed_source_repository
    );
    println!("Main Branch: {}", queue.main_branch);
    println!(
        "Validated Base Commit: {}",
        detail
            .task
            .validated_base_commit
            .as_deref()
            .unwrap_or("not recorded")
    );
    println!(
        "Local Worktree: {}",
        local_worktree.unwrap_or("missing Task Link")
    );
    println!(
        "Task Branch: {}",
        task_branch.unwrap_or("missing Task Link")
    );
    println!(
        "Workpad Note: {}",
        if detail.workpad_note.is_some() {
            "present"
        } else {
            "missing"
        }
    );
    println!();
    print_local_worktree_git_inspection(local_worktree, task_branch, &queue.main_branch);
    println!();
    println!("Latest Agent Run:");
    if let Some(run) = latest_run {
        println!("  id: {}", run.run.id);
        println!("  launcher: {}", run.run.launcher_kind);
        println!(
            "  outcome: {}",
            run.run.outcome.as_deref().unwrap_or("active")
        );
        if let Some(reason) = &run.run.failure_reason {
            println!("  failure reason: {reason}");
        }
        if let Some(session) = &run.launcher_session_data {
            println!(
                "  Run Transcript: {}",
                session.transcript_path.as_deref().unwrap_or("not recorded")
            );
            println!(
                "  Launcher Session Data: present{}",
                session
                    .final_status
                    .as_deref()
                    .map(|status| format!(" (final status: {status})"))
                    .unwrap_or_default()
            );
        } else {
            println!("  Run Transcript: not recorded");
            println!("  Launcher Session Data: missing");
        }
    } else {
        println!("  (none)");
        println!("  Run Transcript: not recorded");
        println!("  Launcher Session Data: missing");
    }
    println!();
    println!("Latest Integration Outcome:");
    if let Some(outcome) = latest_outcome {
        println!("  kind: {}", outcome.outcome_kind);
        println!("  reason code: {}", outcome.reason_code);
        println!(
            "  recovery hint: {}",
            integration_recovery_hint(&outcome.reason_code)
        );
        if let Some(message) = &outcome.message {
            println!("  message: {message}");
        }
    } else {
        println!("  (none)");
    }
    println!();
    println!("Structured gates:");
    for criterion in &detail.acceptance_criteria {
        println!(
            "  Acceptance Criterion {}: [{}] {}",
            criterion.position, criterion.status, criterion.description
        );
    }
    for item in &detail.validation_items {
        println!(
            "  Validation Item {}: [{}] {}",
            item.position, item.status, item.description
        );
    }
    println!();
    println!("Suggested validation commands:");
    println!("  cargo test");
    println!("  cargo clippy --all-targets --all-features -- -D warnings");
    println!("  if TypeScript extension files changed: (cd extensions/tasker-pi && bun test && bun run build)");
    println!();
    println!("Post-merge batch validation:");
    for line in post_merge_batch_validation_guidance() {
        println!("  {line}");
    }
    println!();
    println!("Operator-side squash integration checklist:");
    for (index, line) in
        manual_squash_integration_guidance(&detail.task.task_queue_key, &detail.task.identifier)
            .iter()
            .enumerate()
    {
        println!("  {}. {line}", index + 1);
    }
}

pub(crate) fn manual_squash_integration_guidance(
    queue_key: &str,
    task_identifier: &str,
) -> Vec<String> {
    vec![
        format!(
            "Before mutating Main Branch manually, run: tasker merge lock acquire --queue {queue_key} --operation manual_integration --task {task_identifier}."
        ),
        "Inspect Tasker state, latest Agent Run, Run Transcript, and Workpad Note.".to_string(),
        "From the Local Worktree, verify a clean working tree and focused Task Commits.".to_string(),
        "Run current validation from the Local Worktree after any refresh.".to_string(),
        "From the Managed Source Repository, prefer squash integration: git merge --squash <task-branch>.".to_string(),
        format!(
            "Commit one Final Commit with a concise Conventional Commit subject that includes {task_identifier}, for example: git commit -m \"docs: update Manual Dogfood Merge guidance ({task_identifier})\"."
        ),
        "Do not use Task Branch ancestry as completion proof after a squash integration; Tasker DB state, Integration Outcomes, Audit Events, and the Final Commit are authoritative.".to_string(),
        "From the Managed Source Repository, run post-merge batch validation before marking the batch Done.".to_string(),
        format!("Release the operation lock: tasker merge lock release --queue {queue_key}"),
        format!("After validation, run: tasker merge done {task_identifier} --manual"),
    ]
}

pub(crate) fn post_merge_batch_validation_guidance() -> &'static [&'static str] {
    &[
        "After each Local Merge in a Manual Dogfood Merge batch, validate the combined Main Branch; do not rely only on per-Task Local Worktree validation.",
        "Run at least: cargo test",
        "Run at least: cargo clippy --all-targets --all-features -- -D warnings",
        "This catches overlapping CLI/API changes where individual Task Branches passed but the combined Main Branch can fail to compile.",
        "Temporary Manual Dogfood Merge guidance only; it does not replace the target Integrating implementation or automated Squash Merge.",
    ]
}

fn print_local_worktree_git_inspection(
    local_worktree: Option<&str>,
    task_branch: Option<&str>,
    main_branch: &str,
) {
    println!("Local Worktree Git inspection (read-only):");
    println!("  Git mutations: none; commands below are inspection-only");
    let Some(local_worktree) = local_worktree else {
        println!("  clean: unknown (missing Local Worktree Task Link)");
        println!("  diff from Main Branch: unavailable");
        return;
    };

    let worktree = Path::new(local_worktree);
    if !worktree.exists() {
        println!("  clean: unknown (Local Worktree path does not exist)");
        println!("  diff from Main Branch: unavailable");
        return;
    }

    match git_output(worktree, &["status", "--porcelain"]) {
        Ok(status) if status.trim().is_empty() => println!("  clean: yes"),
        Ok(status) => {
            println!("  clean: no");
            for line in status.lines() {
                println!("    {line}");
            }
        }
        Err(error) => println!("  clean: unknown ({error})"),
    }

    match git_output(worktree, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        Ok(branch) => {
            let branch = branch.trim();
            println!("  checked-out branch: {branch}");
            if let Some(expected) = task_branch {
                if branch != expected {
                    println!("  warning: checked-out branch differs from Task Branch {expected}");
                }
            }
        }
        Err(error) => println!("  checked-out branch: unknown ({error})"),
    }

    let comparison = format!("{main_branch}...HEAD");
    match git_output(worktree, &["diff", "--stat", &comparison]) {
        Ok(stat) if stat.trim().is_empty() => {
            println!("  diff from Main Branch ({comparison}): no file changes")
        }
        Ok(stat) => {
            println!("  diff from Main Branch ({comparison}):");
            for line in stat.lines() {
                println!("    {line}");
            }
        }
        Err(error) => println!("  diff from Main Branch ({comparison}): unavailable ({error})"),
    }

    let commits = format!("{main_branch}..HEAD");
    match git_output(worktree, &["log", "--oneline", &commits]) {
        Ok(log) if log.trim().is_empty() => {
            println!("  Task Commits since Main Branch ({commits}): none")
        }
        Ok(log) => {
            println!("  Task Commits since Main Branch ({commits}):");
            for line in log.lines() {
                println!("    {line}");
            }
        }
        Err(error) => {
            println!("  Task Commits since Main Branch ({commits}): unavailable ({error})")
        }
    }
}

pub(crate) fn git_output(repo: &Path, args: &[&str]) -> Result<String> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git {:?} in {}", args, repo.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
