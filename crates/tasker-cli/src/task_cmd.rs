use super::*;
use crate::merge_cmd::{preflight_integrating_transition, validation_base_commit_for_status};

pub(crate) async fn task(
    paths: &TaskerPaths,
    db_path_overridden: bool,
    command: TaskCommand,
) -> Result<()> {
    let pool = open_pool(paths, db_path_overridden).await?;

    match command {
        TaskCommand::Create {
            bootstrap,
            queue,
            file,
            actor,
        } => {
            if !bootstrap {
                anyhow::bail!("task create currently requires --bootstrap");
            }
            let input = bootstrap::parse_bootstrap_task_file(&queue, &file)?;
            let detail =
                tasker_db::create_task(&pool, &input, &tasker_db::Actor::operator(actor)).await?;
            println!("created Task: {}", detail.task.identifier);
            println!("title: {}", detail.task.title);
            println!("state: {}", detail.task.state);
        }
        TaskCommand::Show { identifier } => {
            let detail = tasker_db::get_task_detail(&pool, &identifier)
                .await?
                .with_context(|| format!("Task {identifier} not found"))?;
            output::print_task_detail(&detail)?;
        }
        TaskCommand::Retry {
            identifier,
            reason,
            actor,
        } => {
            let detail = tasker_db::retry_task(
                &pool,
                &identifier,
                &tasker_db::RetryTaskInput { reason },
                &tasker_db::Actor::operator(actor),
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
            let actor = tasker_db::Actor {
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
            let detail = tasker_db::transition_task_state(
                &pool,
                &identifier,
                &tasker_db::TransitionTaskState {
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
        TaskCommand::Criterion { command } => match command {
            RequirementCommand::Set {
                identifier,
                position,
                status,
                waiver_reason,
                validated_base_commit: _,
                actor,
            } => {
                let input = tasker_db::UpdateRequirementStatus {
                    status: bootstrap::normalize_label(&status),
                    waiver_reason,
                    validated_base_commit: None,
                };
                let detail = tasker_db::update_acceptance_criterion_status(
                    &pool,
                    &identifier,
                    position,
                    &input,
                    &tasker_db::Actor::operator(actor),
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
                let input = tasker_db::UpdateRequirementStatus {
                    status,
                    waiver_reason,
                    validated_base_commit,
                };
                let detail = tasker_db::update_validation_item_status(
                    &pool,
                    &identifier,
                    position,
                    &input,
                    &tasker_db::Actor::operator(actor),
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
                let detail = tasker_db::update_workpad_note(
                    &pool,
                    &identifier,
                    &body,
                    &tasker_db::Actor::operator(actor),
                )
                .await?;
                println!("updated Workpad Note for Task {}", detail.task.identifier);
            }
        },
        TaskCommand::Audit { identifier } => {
            let events = tasker_db::list_task_audit_events(&pool, &identifier).await?;
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
