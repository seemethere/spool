use super::*;

#[derive(Debug, Serialize)]
pub(crate) struct StatusTelemetry<'a> {
    queues: Vec<QueueTelemetry<'a>>,
}

#[derive(Debug, Serialize)]
struct QueueTelemetry<'a> {
    queue_key: &'a str,
    queue_name: &'a str,
    queue_concurrency_limit: Option<i64>,
    active_agent_runs: i64,
    available_claim_slots: Option<i64>,
    ready_tasks: i64,
    integrating_tasks: i64,
    active_integrating_agent_runs: i64,
    active_retry_holds: i64,
    capacity_saturated: bool,
    state_counts: Vec<StateCount<'a>>,
    active_runs: Vec<&'a tasker_db::ActiveAgentRunStatus>,
    ready_task_summaries: Vec<&'a tasker_db::TaskStatusSummary>,
    rework_task_summaries: Vec<&'a tasker_db::TaskStatusSummary>,
    integrating_task_summaries: Vec<&'a tasker_db::TaskStatusSummary>,
    advisory_conflict_hints: Vec<&'a tasker_db::TaskConflictGroup>,
    retry_holds: Vec<&'a tasker_db::ActiveRetryHoldStatus>,
    integration_retries: Vec<&'a tasker_db::IntegrationRetryStatus>,
}

#[derive(Debug, Serialize)]
struct StateCount<'a> {
    state: &'a str,
    task_count: i64,
}

pub(crate) fn build_status_telemetry<'a>(
    rows: &'a [tasker_db::QueueStatus],
    active_runs: &'a [tasker_db::ActiveAgentRunStatus],
    active_holds: &'a [tasker_db::ActiveRetryHoldStatus],
    status_tasks: &'a [tasker_db::TaskStatusSummary],
    conflict_groups: &'a [tasker_db::TaskConflictGroup],
    integration_retries: &'a [tasker_db::IntegrationRetryStatus],
) -> StatusTelemetry<'a> {
    let mut queues = Vec::new();
    let mut index = 0;
    while index < rows.len() {
        let row = &rows[index];
        let mut state_counts = Vec::new();
        while index < rows.len()
            && rows[index].queue_key == row.queue_key
            && rows[index].queue_name == row.queue_name
        {
            state_counts.push(StateCount {
                state: &rows[index].state,
                task_count: rows[index].task_count,
            });
            index += 1;
        }
        let queue_active_runs: Vec<_> = active_runs
            .iter()
            .filter(|run| run.queue_key == row.queue_key)
            .collect();
        queues.push(QueueTelemetry {
            queue_key: &row.queue_key,
            queue_name: &row.queue_name,
            queue_concurrency_limit: row.queue_concurrency_limit,
            active_agent_runs: row.active_agent_runs,
            available_claim_slots: row
                .queue_concurrency_limit
                .map(|limit| (limit - row.active_agent_runs).max(0)),
            ready_tasks: row.ready_tasks,
            integrating_tasks: row.integrating_tasks,
            active_integrating_agent_runs: row.active_integrating_agent_runs,
            active_retry_holds: row.active_retry_holds,
            capacity_saturated: row
                .queue_concurrency_limit
                .map(|limit| row.active_agent_runs >= limit)
                .unwrap_or(false),
            active_runs: queue_active_runs,
            ready_task_summaries: status_tasks
                .iter()
                .filter(|task| task.queue_key == row.queue_key && task.state == "ready")
                .collect(),
            rework_task_summaries: status_tasks
                .iter()
                .filter(|task| task.queue_key == row.queue_key && task.state == "rework")
                .collect(),
            integrating_task_summaries: status_tasks
                .iter()
                .filter(|task| task.queue_key == row.queue_key && task.state == "integrating")
                .collect(),
            advisory_conflict_hints: conflict_groups
                .iter()
                .filter(|group| group.queue_key == row.queue_key)
                .collect(),
            retry_holds: active_holds
                .iter()
                .filter(|hold| hold.queue_key == row.queue_key)
                .collect(),
            integration_retries: integration_retries
                .iter()
                .filter(|retry| retry.queue_key == row.queue_key)
                .collect(),
            state_counts,
        });
    }
    StatusTelemetry { queues }
}

pub(crate) async fn status(
    paths: &TaskerPaths,
    db_path_overridden: bool,
    json: bool,
) -> Result<()> {
    let pool = open_pool(paths, db_path_overridden).await?;
    let rows = tasker_db::status_by_queue_and_state(&pool).await?;
    if rows.is_empty() {
        if json {
            println!("{{\n  \"queues\": []\n}}");
        } else {
            println!("No Task Queues found");
        }
        return Ok(());
    }

    let active_runs = tasker_db::active_agent_runs_for_status(&pool).await?;
    let active_holds = tasker_db::active_retry_holds_for_status(&pool).await?;
    let status_tasks =
        tasker_db::tasks_for_status_by_states(&pool, &["ready", "rework", "integrating"]).await?;
    let conflict_groups = tasker_db::task_conflict_groups_for_status(&pool).await?;
    let integration_retries = tasker_db::integration_retries_for_status(&pool).await?;

    if json {
        serde_json::to_writer_pretty(
            std::io::stdout(),
            &build_status_telemetry(
                &rows,
                &active_runs,
                &active_holds,
                &status_tasks,
                &conflict_groups,
                &integration_retries,
            ),
        )?;
        println!();
        return Ok(());
    }

    let mut current_queue: Option<String> = None;
    for row in rows {
        let queue_header = format!("{}\t{}", row.queue_key, row.queue_name);
        if current_queue.as_ref() != Some(&queue_header) {
            if current_queue.is_some() {
                println!();
            }
            println!("Task Queue: {queue_header}");
            match row.queue_concurrency_limit {
                Some(limit) => {
                    let available = (limit - row.active_agent_runs).max(0);
                    println!(
                        "  active Agent Runs: {} / Queue Concurrency Limit {limit}",
                        row.active_agent_runs
                    );
                    println!("  available claim slots: {available}");
                    if available == 0 && row.ready_tasks > 0 {
                        println!(
                            "  claim status: limit reached; Ready Tasks cannot be claimed until active Agent Runs finish"
                        );
                    }
                }
                None => {
                    println!(
                        "  active Agent Runs: {} / Queue Concurrency Limit none",
                        row.active_agent_runs
                    );
                    println!("  available claim slots: unlimited");
                }
            }
            println!("  Ready Tasks: {}", row.ready_tasks);
            println!("  Integrating Tasks: {}", row.integrating_tasks);
            println!(
                "  active Agent Runs on Integrating Tasks: {}",
                row.active_integrating_agent_runs
            );
            let queue_active_runs: Vec<_> = active_runs
                .iter()
                .filter(|run| run.queue_key.as_str() == row.queue_key.as_str())
                .collect();
            for run in &queue_active_runs {
                println!(
                    "    {}\tstate={}\t{}\tlauncher={}\tworker={}\tlease_expires_at={}",
                    display::task_label(&run.task_identifier, &run.task_title, 64),
                    run.task_state,
                    run.agent_run_id,
                    run.launcher_kind,
                    run.worker_id,
                    run.lease_expires_at
                );
            }
            for state in ["ready", "rework", "integrating"] {
                let queue_tasks: Vec<_> = status_tasks
                    .iter()
                    .filter(|task| task.queue_key == row.queue_key && task.state == state)
                    .collect();
                if !queue_tasks.is_empty() {
                    println!("  {state} Task summaries:");
                    for task in queue_tasks {
                        println!(
                            "    {}\tpriority={}\tlocal_worktree={}",
                            display::task_label(&task.identifier, &task.title, 64),
                            task.priority,
                            local_worktree_status(
                                task.local_worktree.as_deref(),
                                task.task_branch.as_deref(),
                                &task.main_branch,
                            )
                        );
                        if task.unresolved_blocking_task_count > 0 {
                            println!(
                                "      Blocked by: {}",
                                task.blocking_task_identifiers.as_deref().unwrap_or("")
                            );
                        }
                        if state == "rework" {
                            println!(
                                "      Rework reason: code={} reason={}",
                                task.latest_rework_reason_code
                                    .as_deref()
                                    .unwrap_or("unknown_legacy"),
                                task.latest_rework_reason.as_deref().unwrap_or("")
                            );
                        }
                    }
                }
            }
            if let Some(limit) = row.queue_concurrency_limit {
                if row.active_agent_runs >= limit {
                    let active_integrating = queue_active_runs
                        .iter()
                        .filter(|run| run.task_state == "integrating")
                        .count();
                    if active_integrating > 0 {
                        println!(
                            "  capacity saturated: active Agent Runs count against Queue Concurrency Limit, including {active_integrating} Integrating run(s). Unblock by waiting for completion or lease expiry, inspecting/finishing stuck runs with `tasker run show`/`tasker run fail`, or raising/clearing the Queue Concurrency Limit only if local resources permit."
                        );
                    } else {
                        println!(
                            "  capacity saturated: active Agent Runs count against Queue Concurrency Limit. Unblock by waiting for completion or lease expiry, inspecting/finishing stuck runs with `tasker run show`/`tasker run fail`, or raising/clearing the Queue Concurrency Limit only if local resources permit."
                        );
                    }
                }
            }
            let queue_conflict_groups: Vec<_> = conflict_groups
                .iter()
                .filter(|group| group.queue_key.as_str() == row.queue_key.as_str())
                .collect();
            if !queue_conflict_groups.is_empty() {
                println!("  advisory Task conflict hints:");
                for group in queue_conflict_groups {
                    println!(
                        "    {}\t{} Task(s): {}",
                        group.target, group.task_count, group.tasks
                    );
                }
            }
            let queue_integration_retries: Vec<_> = integration_retries
                .iter()
                .filter(|retry| retry.queue_key.as_str() == row.queue_key.as_str())
                .collect();
            if !queue_integration_retries.is_empty() {
                println!("  Integration retry waits:");
                for retry in queue_integration_retries {
                    println!(
                        "    {}\treason_code={}\tattempt={}\tnext_retry_at={}\tretryable={}\thint={}\treason={}",
                        display::task_label(&retry.task_identifier, &retry.task_title, 64),
                        retry.reason_code,
                        retry.retry_attempt.unwrap_or_default(),
                        retry
                            .next_retry_at
                            .as_deref()
                            .unwrap_or("operator intervention required"),
                        retry.retryable,
                        integration_recovery_hint(&retry.reason_code),
                        retry.reason.as_deref().unwrap_or("")
                    );
                }
            }
            println!("  active Retry Holds: {}", row.active_retry_holds);
            for hold in active_holds
                .iter()
                .filter(|hold| hold.queue_key.as_str() == row.queue_key.as_str())
            {
                println!(
                    "    {}\thold_until={}\tfailure_code={}\treason={}",
                    hold.task_identifier,
                    hold.hold_until,
                    hold.failure_reason_code.as_deref().unwrap_or("-"),
                    hold.reason
                );
            }
            current_queue = Some(queue_header);
        }
        println!("  {}: {}", row.state, row.task_count);
    }

    Ok(())
}

fn local_worktree_status(
    local_worktree: Option<&str>,
    task_branch: Option<&str>,
    main_branch: &str,
) -> String {
    let Some(path) = local_worktree else {
        return "missing Task Link".to_string();
    };
    let worktree = Path::new(path);
    if !worktree.exists() {
        return format!("missing ({path})");
    }

    let cleanliness = match git_output(worktree, &["status", "--porcelain"]) {
        Ok(status) if status.trim().is_empty() => "clean".to_string(),
        Ok(_) => "dirty".to_string(),
        Err(_) => "exists, git status unavailable".to_string(),
    };
    let branch = git_output(worktree, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .map(|branch| branch.trim().to_string());
    let branch_note = match (branch.as_deref(), task_branch) {
        (Some(actual), Some(expected)) if actual != expected => {
            format!(" branch={actual} expected={expected}")
        }
        (Some(actual), _) => format!(" branch={actual}"),
        (None, Some(expected)) => format!(" branch=unknown expected={expected}"),
        (None, None) => " branch=unknown".to_string(),
    };
    let includes_main = match git_status(
        worktree,
        &["merge-base", "--is-ancestor", main_branch, "HEAD"],
    ) {
        Ok(status) if status.success() => "yes",
        Ok(_) => "no",
        Err(_) => "unknown",
    };

    format!("{cleanliness}{branch_note} includes_main={includes_main} ({path})")
}

pub(crate) fn integration_recovery_hint(reason_code: &str) -> &'static str {
    match reason_code {
        "dirty_managed_source_repository" => "clean or intentionally resolve Managed Source Repository changes, then retry integration",
        "repo_operation_lock_held" => "wait for or release the Managed Source Repository operation lock after verification",
        "uncommitted_local_worktree" => "commit or discard Local Worktree changes and move through Rework validation",
        "stale_validated_base_commit" => "rebase or revalidate against current Main Branch",
        "auto_refresh_conflict" => "resolve auto-refresh rebase conflicts in Rework",
        "auto_refresh_validation_failed" => "fix validation failures after auto-refresh and revalidate",
        "auto_refresh_declined_missing_validation" => "configure validation commands before retrying integration",
        "task_branch_missing_main" => "merge/rebase Main Branch into the Task Branch or record a current Validated Base Commit",
        "merge_conflict" => "resolve conflicts in Rework before integrating again",
        "cleanup_failure" => "remove retained Local Worktree or Task Branch manually after verifying integration",
        "unknown_legacy" => "inspect the human-readable Integration Outcome message",
        _ => "inspect the Integration Outcome message and local repository state",
    }
}

pub(crate) async fn monitor(
    paths: &TaskerPaths,
    db_path_overridden: bool,
    queue: Option<String>,
    refresh_seconds: u64,
    plain: bool,
    once: bool,
) -> Result<()> {
    let mut config = TaskerConfig::load_or_default(paths)?;
    if db_path_overridden {
        config.database.path = paths.db_path.clone();
    }
    let pool = tasker_db::connect(&config.database.path).await?;
    tasker_db::check_migration_compatibility(&pool).await?;
    monitor::run_monitor(
        &pool,
        monitor::MonitorOptions {
            queue,
            refresh_seconds,
            plain,
            once,
            config_path: paths.config_path.clone(),
            data_dir: paths.data_dir.clone(),
            db_path: config.database.path,
        },
    )
    .await
}
