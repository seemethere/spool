use super::*;
use std::{io::Write, path::Path, process::Command as ProcessCommand};

pub(crate) async fn review_packet(
    paths: &TaskerPaths,
    db_path_overridden: bool,
    identifier: String,
) -> Result<()> {
    let pool = open_pool(paths, db_path_overridden).await?;
    let bundle = tasker_db::get_task_context_bundle(&pool, &identifier)
        .await?
        .with_context(|| format!("Task {identifier} not found"))?;
    write_review_packet(std::io::stdout(), &bundle)?;
    Ok(())
}

pub(crate) fn write_review_packet(
    mut writer: impl Write,
    bundle: &tasker_db::TaskContextBundle,
) -> std::io::Result<()> {
    let detail = &bundle.task;
    writeln!(writer, "Review Packet")?;
    writeln!(writer, "Task: {}", detail.task.identifier)?;
    writeln!(writer, "title: {}", detail.task.title)?;
    writeln!(writer, "Task Queue: {}", detail.task.task_queue_key)?;
    writeln!(writer, "Task State: {}", detail.task.state)?;
    writeln!(writer, "Priority: {}", detail.task.priority)?;
    writeln!(writer, "review required: {}", detail.task.review_required)?;
    writeln!(
        writer,
        "Validated Base Commit: {}",
        detail
            .task
            .validated_base_commit
            .as_deref()
            .unwrap_or("not recorded")
    )?;
    writeln!(
        writer,
        "Managed Source Repository: {}",
        bundle.queue.managed_source_repository
    )?;
    writeln!(writer, "Main Branch: {}", bundle.queue.main_branch)?;

    writeln!(writer, "\nTask Brief:")?;
    write_text_or_none(&mut writer, &detail.task.brief)?;

    writeln!(writer, "\nAcceptance Criteria:")?;
    if detail.acceptance_criteria.is_empty() {
        writeln!(writer, "(none)")?;
    } else {
        for criterion in &detail.acceptance_criteria {
            writeln!(
                writer,
                "  {}. [{}] {}",
                criterion.position, criterion.status, criterion.description
            )?;
            if let Some(reason) = &criterion.waiver_reason {
                writeln!(writer, "     waiver: {reason}")?;
            }
        }
    }

    writeln!(writer, "\nValidation Items:")?;
    if detail.validation_items.is_empty() {
        writeln!(writer, "(none)")?;
    } else {
        for item in &detail.validation_items {
            writeln!(
                writer,
                "  {}. [{}] {}",
                item.position, item.status, item.description
            )?;
            if let Some(reason) = &item.waiver_reason {
                writeln!(writer, "     waiver: {reason}")?;
            }
        }
    }

    writeln!(writer, "\nWorkpad Note:")?;
    match &detail.workpad_note {
        Some(note) => write_text_or_none(&mut writer, &note.body)?,
        None => writeln!(writer, "(none)")?,
    }

    writeln!(writer, "\nTask Links:")?;
    if detail.task_links.is_empty() {
        writeln!(writer, "(none)")?;
    } else {
        for link in &detail.task_links {
            let primary = if link.is_primary { " primary" } else { "" };
            let label = link.label.as_deref().unwrap_or("");
            writeln!(
                writer,
                "  [{}{}] {} {}",
                link.kind, primary, link.target, label
            )?;
        }
    }

    write_local_worktree_summary(&mut writer, bundle)?;
    write_agent_run_summary(&mut writer, &bundle.agent_runs)?;
    write_integration_summary(&mut writer, bundle.latest_integration_outcome.as_ref())?;
    write_blocking_context(&mut writer, detail)?;
    writeln!(
        writer,
        "\nPrivacy boundary: raw Run Transcripts, raw Launcher Session Data payloads, prompts, secrets, and unrelated Task Queue data are omitted."
    )?;
    Ok(())
}

fn write_text_or_none(mut writer: impl Write, text: &str) -> std::io::Result<()> {
    if text.trim().is_empty() {
        writeln!(writer, "(none)")
    } else {
        writeln!(writer, "{}", text.trim())
    }
}

fn write_local_worktree_summary(
    mut writer: impl Write,
    bundle: &tasker_db::TaskContextBundle,
) -> std::io::Result<()> {
    writeln!(writer, "\nLocal Worktree Delivery Summary:")?;
    writeln!(
        writer,
        "  delivery backend: {}",
        bundle.local_workflow.delivery_backend
    )?;
    writeln!(
        writer,
        "  Local Worktree: {}",
        bundle
            .local_workflow
            .local_worktree
            .as_deref()
            .unwrap_or("missing Task Link")
    )?;
    writeln!(
        writer,
        "  Task Branch: {}",
        bundle
            .local_workflow
            .task_branch
            .as_deref()
            .unwrap_or("missing Task Link")
    )?;
    let Some(local_worktree) = bundle.local_workflow.local_worktree.as_deref() else {
        writeln!(
            writer,
            "  diff summary: unavailable (missing Local Worktree Task Link)"
        )?;
        writeln!(writer, "  path guidance: inspect Task Links before review")?;
        return Ok(());
    };
    let worktree = Path::new(local_worktree);
    if !worktree.exists() {
        writeln!(writer, "  exists: no")?;
        writeln!(
            writer,
            "  diff summary: unavailable (Local Worktree path does not exist)"
        )?;
        return Ok(());
    }
    writeln!(writer, "  exists: yes")?;
    match git_output(worktree, &["status", "--porcelain"]) {
        Ok(status) if status.trim().is_empty() => writeln!(writer, "  clean: yes")?,
        Ok(status) => writeln!(
            writer,
            "  clean: no ({} changed path(s))",
            status.lines().count()
        )?,
        Err(error) => writeln!(writer, "  clean: unknown ({error})")?,
    }
    match git_output(worktree, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        Ok(branch) => writeln!(writer, "  checked-out branch: {}", branch.trim())?,
        Err(error) => writeln!(writer, "  checked-out branch: unknown ({error})")?,
    }
    let comparison = bundle
        .task
        .task
        .validated_base_commit
        .as_deref()
        .unwrap_or(&bundle.local_workflow.main_branch);
    let diff_range = format!("{comparison}...HEAD");
    match git_output(worktree, &["diff", "--stat", &diff_range]) {
        Ok(stat) if stat.trim().is_empty() => {
            writeln!(writer, "  diff summary ({diff_range}): no file changes")?
        }
        Ok(stat) => {
            writeln!(writer, "  diff summary ({diff_range}):")?;
            for line in stat.lines().take(20) {
                writeln!(writer, "    {line}")?;
            }
        }
        Err(error) => writeln!(
            writer,
            "  diff summary ({diff_range}): unavailable ({error})"
        )?,
    }
    let commits = format!("{}..HEAD", bundle.local_workflow.main_branch);
    match git_output(worktree, &["log", "--oneline", &commits]) {
        Ok(log) if log.trim().is_empty() => writeln!(writer, "  Task Commits ({commits}): none")?,
        Ok(log) => {
            writeln!(writer, "  Task Commits ({commits}):")?;
            for line in log.lines().take(10) {
                writeln!(writer, "    {line}")?;
            }
        }
        Err(error) => writeln!(writer, "  Task Commits ({commits}): unavailable ({error})")?,
    }
    writeln!(writer, "  path guidance: cd {local_worktree}")?;
    Ok(())
}

fn write_agent_run_summary(
    mut writer: impl Write,
    runs: &[tasker_db::TaskContextAgentRun],
) -> std::io::Result<()> {
    writeln!(writer, "\nRecent Agent Runs:")?;
    if runs.is_empty() {
        writeln!(writer, "(none)")?;
        return Ok(());
    }
    for run in runs {
        writeln!(
            writer,
            "  {} launcher={} outcome={} worker={} created_at={}",
            run.id,
            run.launcher_kind,
            run.outcome.as_deref().unwrap_or("active"),
            run.worker_actor_display_name,
            run.created_at
        )?;
        if let Some(code) = &run.failure_reason_code {
            writeln!(writer, "     failure reason code: {code}")?;
        }
        if run.duration_ms.is_some()
            || run.tool_call_count.is_some()
            || run.total_tokens.is_some()
            || run.repeated_read_count.is_some()
        {
            writeln!(
                writer,
                "     metrics: duration_ms={} tool_calls={} repeated_reads={} tokens={}",
                display_opt(run.duration_ms),
                display_opt(run.tool_call_count),
                display_opt(run.repeated_read_count),
                display_opt(run.total_tokens)
            )?;
        }
    }
    Ok(())
}

fn write_integration_summary(
    mut writer: impl Write,
    outcome: Option<&tasker_db::TaskContextIntegrationOutcome>,
) -> std::io::Result<()> {
    writeln!(writer, "\nLatest Integration Outcome:")?;
    let Some(outcome) = outcome else {
        writeln!(writer, "(none)")?;
        return Ok(());
    };
    writeln!(writer, "  kind: {}", outcome.outcome_kind)?;
    writeln!(
        writer,
        "  reason code: {}",
        outcome.reason_code.as_deref().unwrap_or("not recorded")
    )?;
    if let Some(final_commit) = &outcome.final_commit {
        writeln!(writer, "  Final Commit: {final_commit}")?;
    }
    if let Some(message) = &outcome.message {
        writeln!(writer, "  message: {message}")?;
    }
    writeln!(writer, "  created_at: {}", outcome.created_at)
}

fn write_blocking_context(
    mut writer: impl Write,
    detail: &tasker_db::TaskDetail,
) -> std::io::Result<()> {
    writeln!(writer, "\nBlocking and Follow-up Context:")?;
    if detail.blocking_tasks.is_empty() && detail.blocked_tasks.is_empty() {
        writeln!(writer, "(none)")?;
        return Ok(());
    }
    for task in &detail.blocking_tasks {
        let status = if task.resolved {
            "resolved"
        } else {
            "unresolved"
        };
        writeln!(
            writer,
            "  Blocking Task: {} [{}] {} ({status})",
            task.identifier, task.state, task.title
        )?;
    }
    for task in &detail.blocked_tasks {
        writeln!(
            writer,
            "  Related blocked Task: {} [{}] {}",
            task.identifier, task.state, task.title
        )?;
    }
    Ok(())
}

fn display_opt(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn git_output(repo: &Path, args: &[&str]) -> std::io::Result<String> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn review_packet_includes_human_review_context_and_local_diff_summary() {
        let fixture = GitFixture::new();
        let bundle = sample_bundle(&fixture);

        let mut out = Vec::new();
        write_review_packet(&mut out, &bundle).expect("render packet");
        let text = String::from_utf8(out).expect("utf8");

        assert!(text.contains("Review Packet"));
        assert!(text.contains("Task: TASK-1"));
        assert!(text.contains("Task State: human_review"));
        assert!(text.contains("Acceptance Criteria:"));
        assert!(text.contains("1. [satisfied] Implement deterministic builder"));
        assert!(text.contains("Validation Items:"));
        assert!(text.contains("1. [passed] cargo test -p tasker-cli review_packet"));
        assert!(text.contains("Workpad Note:"));
        assert!(text.contains("Reviewer attention: validate CLI preview."));
        assert!(text.contains("[local_worktree primary]"));
        assert!(text.contains("Local Worktree Delivery Summary:"));
        assert!(text.contains("diff summary (main...HEAD):"));
        assert!(text.contains("review.txt"));
        assert!(text.contains("Task Commits (main..HEAD):"));
        assert!(text.contains("Recent Agent Runs:"));
        assert!(text.contains("Latest Integration Outcome:"));
        assert!(
            text.lines().count() < 90,
            "packet should stay concise: {text}"
        );
    }

    #[test]
    fn review_packet_omits_raw_transcript_and_launcher_payload_content() {
        let fixture = GitFixture::new();
        let mut bundle = sample_bundle(&fixture);
        bundle.agent_runs[0].session_id = Some("session-secret-marker".to_string());
        bundle.agent_runs[0].efficiency_hints_json =
            Some("RAW_LAUNCHER_PAYLOAD_SECRET".to_string());
        bundle.agent_runs[0].failure_reason = Some("sanitized failure summary".to_string());

        let mut out = Vec::new();
        write_review_packet(&mut out, &bundle).expect("render packet");
        let text = String::from_utf8(out).expect("utf8");

        assert!(!text.contains("RAW_LAUNCHER_PAYLOAD_SECRET"));
        assert!(!text.contains("session-secret-marker"));
        assert!(!text.contains("raw_json"));
        assert!(!text.contains("transcript body"));
        assert!(text.contains("raw Run Transcripts"));
    }

    struct GitFixture {
        _temp: TempDir,
        repo: std::path::PathBuf,
    }

    impl GitFixture {
        fn new() -> Self {
            let temp = TempDir::new().expect("tempdir");
            let repo = temp.path().join("worktree");
            std::fs::create_dir_all(&repo).expect("repo dir");
            git(&repo, &["init", "-b", "main"]);
            git(&repo, &["config", "user.email", "test@example.invalid"]);
            git(&repo, &["config", "user.name", "Tasker Test"]);
            std::fs::write(repo.join("README.md"), "base\n").expect("base file");
            git(&repo, &["add", "README.md"]);
            git(&repo, &["commit", "-m", "base"]);
            git(&repo, &["checkout", "-b", "tasker/TASK-1"]);
            std::fs::write(repo.join("review.txt"), "review packet\n").expect("review file");
            git(&repo, &["add", "review.txt"]);
            git(&repo, &["commit", "-m", "add review packet"]);
            Self { _temp: temp, repo }
        }
    }

    fn git(repo: &Path, args: &[&str]) {
        let output = ProcessCommand::new("git")
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

    fn sample_bundle(fixture: &GitFixture) -> tasker_db::TaskContextBundle {
        let task_id = "task-1".to_string();
        let now = "2026-05-11 00:00:00".to_string();
        tasker_db::TaskContextBundle {
            task: tasker_db::TaskDetail {
                task: tasker_db::Task {
                    id: task_id.clone(),
                    task_queue_id: "queue-1".to_string(),
                    task_queue_key: "TASK".to_string(),
                    identifier: "TASK-1".to_string(),
                    sequence: 1,
                    title: "Build Review Packet".to_string(),
                    brief: "Prepare a concise local review summary.".to_string(),
                    priority: "normal".to_string(),
                    state: "human_review".to_string(),
                    review_required: true,
                    validated_base_commit: None,
                    created_at: now.clone(),
                    updated_at: now.clone(),
                },
                acceptance_criteria: vec![tasker_db::AcceptanceCriterion {
                    id: "ac-1".to_string(),
                    task_id: task_id.clone(),
                    position: 1,
                    description: "Implement deterministic builder".to_string(),
                    status: "satisfied".to_string(),
                    waiver_reason: None,
                }],
                validation_items: vec![tasker_db::ValidationItem {
                    id: "vi-1".to_string(),
                    task_id: task_id.clone(),
                    position: 1,
                    description: "cargo test -p tasker-cli review_packet".to_string(),
                    status: "passed".to_string(),
                    waiver_reason: None,
                }],
                tags: vec!["review-sessions".to_string()],
                workpad_note: Some(tasker_db::WorkpadNote {
                    id: "note-1".to_string(),
                    task_id: task_id.clone(),
                    body: "Summary: ready for review.\nReviewer attention: validate CLI preview."
                        .to_string(),
                    created_at: now.clone(),
                    updated_at: now.clone(),
                }),
                task_links: vec![
                    tasker_db::TaskLink {
                        id: "link-1".to_string(),
                        task_id: task_id.clone(),
                        kind: "local_worktree".to_string(),
                        target: fixture.repo.display().to_string(),
                        label: Some("Local Worktree".to_string()),
                        is_primary: true,
                        created_at: now.clone(),
                        updated_at: now.clone(),
                    },
                    tasker_db::TaskLink {
                        id: "link-2".to_string(),
                        task_id: task_id.clone(),
                        kind: "task_branch".to_string(),
                        target: "tasker/TASK-1".to_string(),
                        label: Some("Task Branch".to_string()),
                        is_primary: false,
                        created_at: now.clone(),
                        updated_at: now.clone(),
                    },
                ],
                conflict_hints: Vec::new(),
                conflict_overlaps: Vec::new(),
                blocking_tasks: Vec::new(),
                blocked_tasks: Vec::new(),
                latest_rework_reason_code: None,
                latest_rework_reason: None,
            },
            queue: tasker_db::TaskContextQueue {
                key: "TASK".to_string(),
                name: "Tasker".to_string(),
                delivery_backend: "local_worktree".to_string(),
                main_branch: "main".to_string(),
                managed_source_repository: fixture.repo.display().to_string(),
                worktree_root: fixture.repo.parent().unwrap().display().to_string(),
                branch_template: "tasker/{task_identifier}".to_string(),
                queue_concurrency_limit: Some(1),
            },
            local_workflow: tasker_db::TaskLocalWorkflowContext {
                local_worktree: Some(fixture.repo.display().to_string()),
                task_branch: Some("tasker/TASK-1".to_string()),
                main_branch: "main".to_string(),
                managed_source_repository: fixture.repo.display().to_string(),
                worktree_root: fixture.repo.parent().unwrap().display().to_string(),
                branch_template: "tasker/{task_identifier}".to_string(),
                delivery_backend: "local_worktree".to_string(),
            },
            advisory_hints: tasker_db::TaskContextAdvisoryHints {
                note: "advisory only".to_string(),
                task_conflict_hints: Vec::new(),
                likely_files_or_paths: Vec::new(),
            },
            agent_runs: vec![tasker_db::TaskContextAgentRun {
                id: "run-1".to_string(),
                worker_actor_kind: "worker_agent".to_string(),
                worker_actor_id: "worker".to_string(),
                worker_actor_display_name: "worker".to_string(),
                worker_id: "worker".to_string(),
                launcher_kind: "pi".to_string(),
                lease_expires_at: now.clone(),
                last_heartbeat_at: Some(now.clone()),
                outcome: Some("completed".to_string()),
                failure_reason: None,
                failure_reason_code: None,
                created_at: now.clone(),
                finished_at: Some(now.clone()),
                is_active: false,
                session_id: None,
                model: Some("test-model".to_string()),
                provider: Some("test-provider".to_string()),
                final_status: Some("completed".to_string()),
                duration_ms: Some(1234),
                tool_call_count: Some(5),
                tool_error_count: Some(0),
                repeated_failed_tool_attempt_count: Some(0),
                repeated_read_count: Some(1),
                repeated_tasker_context_fetch_count: Some(1),
                total_tokens: Some(1000),
                max_context_tokens: Some(5000),
                efficiency_hints_json: None,
            }],
            latest_failure: None,
            latest_integration_outcome: Some(tasker_db::TaskContextIntegrationOutcome {
                id: "outcome-1".to_string(),
                agent_run_id: Some("run-1".to_string()),
                outcome_kind: "work_change_failure".to_string(),
                reason_code: Some("stale_validated_base_commit".to_string()),
                final_commit: None,
                pre_merge_head: Some("abc123".to_string()),
                message: Some("Revalidate against Main Branch.".to_string()),
                retryable: false,
                retry_attempt: None,
                next_retry_at: None,
                created_at: now,
            }),
        }
    }
}
