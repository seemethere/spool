use super::*;
use crate::merge_cmd::{preflight_integrating_transition, validation_base_commit_for_status};

pub(crate) async fn task(
    paths: &SpoolPaths,
    db_path_overridden: bool,
    command: TaskCommand,
) -> Result<()> {
    if let TaskCommand::Lint { file } = &command {
        let input = bootstrap::lint_bootstrap_task_file(file)?;
        println!("valid file-backed task definition");
        println!("title: {}", input.title);
        println!("priority: {}", input.priority);
        println!("state: {}", input.state);
        return Ok(());
    }

    let pool = open_pool(paths, db_path_overridden).await?;

    match command {
        TaskCommand::Create {
            bootstrap,
            queue,
            from_file,
            file,
            actor,
        } => {
            let file = match (from_file, file) {
                (Some(file), None) => file,
                (None, Some(file)) if bootstrap => file,
                (None, Some(_)) => {
                    anyhow::bail!(
                        "task create --file is the Bootstrap Task Creation compatibility path and requires --bootstrap; prefer task create --from-file"
                    );
                }
                (None, None) => {
                    anyhow::bail!(
                        "task create requires a file-backed Task definition; use --from-file <task.md>"
                    );
                }
                (Some(_), Some(_)) => {
                    anyhow::bail!("task create accepts only one of --from-file or --file");
                }
            };
            let parsed = bootstrap::parse_bootstrap_task_file_with_warnings(&queue, &file)?;
            for warning in &parsed.warnings {
                eprintln!("warning: {warning}");
            }
            let input = parsed.task;
            let detail =
                spool_db::create_task(&pool, &input, &spool_db::Actor::operator(actor)).await?;
            println!("created Task: {}", detail.task.identifier);
            println!("title: {}", detail.task.title);
            println!("state: {}", detail.task.state);
        }
        TaskCommand::Lint { .. } => unreachable!("lint returns before opening the Task Backend"),
        TaskCommand::Show { identifier } => {
            let detail = spool_db::get_task_detail(&pool, &identifier)
                .await?
                .with_context(|| format!("Task {identifier} not found"))?;
            output::print_task_detail(&detail)?;
        }
        TaskCommand::Retry {
            identifier,
            reason,
            actor,
        } => {
            let detail = spool_db::retry_task(
                &pool,
                &identifier,
                &spool_db::RetryTaskInput { reason },
                &spool_db::Actor::operator(actor),
            )
            .await?;
            println!("retried Task {} to Ready", detail.task.identifier);
        }
        TaskCommand::Transition {
            identifier,
            to,
            actor_kind,
            actor,
            agent_run_id,
            repair_override,
        } => {
            let to_state = bootstrap::normalize_label(&to);
            let actor = spool_db::Actor {
                kind: actor_kind,
                id: actor.clone(),
                display_name: actor,
            };
            if to_state == "integrating" {
                if let Some(warning) =
                    preflight_integrating_transition(&pool, &identifier, &actor).await?
                {
                    eprintln!("warning: {warning}");
                }
            }
            let detail = spool_db::transition_task_state(
                &pool,
                &identifier,
                &spool_db::TransitionTaskState {
                    to_state,
                    agent_run_id,
                    repair_override,
                },
                &actor,
            )
            .await?;
            println!(
                "transitioned Task {} to {}",
                detail.task.identifier, detail.task.state
            );
        }
        TaskCommand::ReviewDecision {
            identifier,
            decision,
            feedback,
            feedback_file,
            actor_kind,
            actor,
        } => {
            let feedback = match (feedback, feedback_file) {
                (Some(feedback), None) => Some(feedback),
                (None, Some(file)) => Some(
                    fs::read_to_string(&file)
                        .with_context(|| format!("failed to read {}", file.display()))?,
                ),
                (None, None) => None,
                (Some(_), Some(_)) => unreachable!("clap enforces feedback conflict"),
            };
            let actor = spool_db::Actor {
                kind: actor_kind,
                id: actor.clone(),
                display_name: actor,
            };
            let detail = spool_db::record_review_decision(
                &pool,
                &identifier,
                &spool_db::RecordReviewDecision {
                    decision: bootstrap::normalize_label(&decision),
                    feedback,
                },
                &actor,
            )
            .await?;
            println!(
                "recorded Review Decision for Task {}: {}",
                detail.task.identifier, detail.task.state
            );
        }
        TaskCommand::Criterion { command } => match command {
            RequirementCommand::Set {
                identifier,
                position,
                status,
                waiver_reason,
                validated_base_commit: _,
                actor,
            } => {
                let input = spool_db::UpdateRequirementStatus {
                    status: bootstrap::normalize_label(&status),
                    waiver_reason,
                    validated_base_commit: None,
                };
                let detail = spool_db::update_acceptance_criterion_status(
                    &pool,
                    &identifier,
                    position,
                    &input,
                    &spool_db::Actor::operator(actor),
                )
                .await?;
                println!(
                    "updated Acceptance Criterion {position} for Task {}",
                    detail.task.identifier
                );
            }
        },
        TaskCommand::Validation { command } => match command {
            RequirementCommand::Set {
                identifier,
                position,
                status,
                waiver_reason,
                validated_base_commit,
                actor,
            } => {
                let status = bootstrap::normalize_label(&status);
                let validated_base_commit = validation_base_commit_for_status(
                    &pool,
                    &identifier,
                    &status,
                    validated_base_commit,
                )
                .await?;
                let input = spool_db::UpdateRequirementStatus {
                    status,
                    waiver_reason,
                    validated_base_commit,
                };
                let detail = spool_db::update_validation_item_status(
                    &pool,
                    &identifier,
                    position,
                    &input,
                    &spool_db::Actor::operator(actor),
                )
                .await?;
                println!(
                    "updated Validation Item {position} for Task {}",
                    detail.task.identifier
                );
            }
        },
        TaskCommand::Workpad { command } => match command {
            WorkpadCommand::Set {
                identifier,
                file,
                actor,
            } => {
                let body = fs::read_to_string(&file)
                    .with_context(|| format!("failed to read {}", file.display()))?;
                let detail = spool_db::update_workpad_note(
                    &pool,
                    &identifier,
                    &body,
                    &spool_db::Actor::operator(actor),
                )
                .await?;
                println!("updated Workpad Note for Task {}", detail.task.identifier);
            }
        },
        TaskCommand::Blocker { command } => match command {
            BlockerCommand::Add {
                identifier,
                blocking_identifier,
                actor,
            } => {
                let detail = spool_db::add_blocking_task_relationship(
                    &pool,
                    &identifier,
                    &blocking_identifier,
                    &spool_db::Actor::operator(actor),
                )
                .await?;
                println!(
                    "added Blocking Task {} to Task {}",
                    blocking_identifier.trim().to_ascii_uppercase(),
                    detail.task.identifier
                );
            }
            BlockerCommand::Remove {
                identifier,
                blocking_identifier,
                actor,
            } => {
                let detail = spool_db::remove_blocking_task_relationship(
                    &pool,
                    &identifier,
                    &blocking_identifier,
                    &spool_db::Actor::operator(actor),
                )
                .await?;
                println!(
                    "removed Blocking Task {} from Task {}",
                    blocking_identifier.trim().to_ascii_uppercase(),
                    detail.task.identifier
                );
            }
            BlockerCommand::List { identifier } => {
                let detail = spool_db::get_task_detail(&pool, &identifier)
                    .await?
                    .with_context(|| format!("Task {identifier} not found"))?;
                if detail.blocking_tasks.is_empty() {
                    println!("Blocking Tasks: none");
                } else {
                    println!("Blocking Tasks:");
                    for task in detail.blocking_tasks {
                        let status = if task.resolved {
                            "resolved"
                        } else {
                            "unresolved"
                        };
                        println!(
                            "  {} [{}] {} ({status})",
                            task.identifier, task.state, task.title
                        );
                    }
                }
            }
        },
        TaskCommand::Audit { identifier } => {
            let events = spool_db::list_task_audit_events(&pool, &identifier).await?;
            for event in events {
                println!(
                    "{}\t{}\t{} ({})\t{}",
                    event.created_at,
                    event.event_type,
                    event.actor_display_name,
                    event.actor_kind,
                    event.payload_json
                );
            }
        }
    }

    Ok(())
}
