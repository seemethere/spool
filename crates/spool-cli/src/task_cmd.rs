use super::*;
use crate::merge_cmd::{preflight_integrating_transition, validation_base_commit_for_status};
use std::collections::{HashMap, HashSet};

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
            bootstrap::reject_batch_only_fields(&parsed, "task create --from-file")?;
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
        TaskCommand::Batch { command } => match command {
            TaskBatchCommand::Lint { queue, from_files } => {
                let batch = validate_task_batch(&pool, &queue, &from_files).await?;
                println!("valid file-backed Task batch");
                println!("queue: {queue}");
                println!("tasks: {}", batch.items.len());
                print_batch_plan(&batch);
            }
            TaskBatchCommand::Create {
                queue,
                from_files,
                actor,
            } => {
                let mut batch = validate_task_batch(&pool, &queue, &from_files).await?;
                println!("creating file-backed Task batch");
                print_batch_plan(&batch);
                let actor = spool_db::Actor::operator(actor);
                let mut created_by_key: HashMap<String, String> = HashMap::new();
                for index in batch.creation_order {
                    let item = &mut batch.items[index];
                    for key in &item.blocking_task_keys {
                        let identifier = created_by_key.get(key).with_context(|| {
                            format!("same-batch Blocking Task {key} was not created")
                        })?;
                        item.task.blocking_task_identifiers.push(identifier.clone());
                    }
                    let detail = spool_db::create_task(&pool, &item.task, &actor).await?;
                    println!(
                        "created Task: {}{}",
                        detail.task.identifier,
                        item.batch_key
                            .as_ref()
                            .map(|key| format!(" (batch_key: {key})"))
                            .unwrap_or_default()
                    );
                    if let Some(key) = &item.batch_key {
                        created_by_key.insert(key.clone(), detail.task.identifier);
                    }
                }
            }
        },
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

#[derive(Debug)]
struct BatchItem {
    source: PathBuf,
    task: spool_db::CreateTask,
    batch_key: Option<String>,
    blocking_task_keys: Vec<String>,
}

#[derive(Debug)]
struct ValidatedTaskBatch {
    items: Vec<BatchItem>,
    creation_order: Vec<usize>,
}

async fn validate_task_batch(
    pool: &sqlx::SqlitePool,
    queue: &str,
    from_files: &[PathBuf],
) -> Result<ValidatedTaskBatch> {
    if from_files.is_empty() {
        anyhow::bail!("task batch requires at least one --from-file");
    }
    spool_db::get_task_queue(pool, queue)
        .await?
        .with_context(|| format!("Task Queue {queue} not found"))?;

    let mut items = Vec::new();
    for file in from_files {
        let parsed = bootstrap::parse_bootstrap_task_file_with_warnings(queue, file)?;
        spool_db::validate_create_task(&parsed.task)?;
        for warning in &parsed.warnings {
            eprintln!("warning: {}: {warning}", file.display());
        }
        items.push(BatchItem {
            source: file.clone(),
            task: parsed.task,
            batch_key: parsed.batch_key.map(|key| key.trim().to_string()),
            blocking_task_keys: parsed
                .blocking_task_keys
                .into_iter()
                .map(|key| key.trim().to_string())
                .collect(),
        });
    }

    let mut key_to_index = HashMap::new();
    for (index, item) in items.iter().enumerate() {
        if let Some(key) = &item.batch_key {
            if key.is_empty() {
                anyhow::bail!("{}: batch_key must not be blank", item.source.display());
            }
            if let Some(previous) = key_to_index.insert(key.clone(), index) {
                anyhow::bail!(
                    "duplicate batch_key {key} in {} and {}",
                    items[previous].source.display(),
                    item.source.display()
                );
            }
        }
    }

    for item in &items {
        for identifier in &item.task.blocking_task_identifiers {
            let detail = spool_db::get_task_detail(pool, identifier)
                .await?
                .with_context(|| {
                    format!(
                        "{}: existing Blocking Task {identifier} not found",
                        item.source.display()
                    )
                })?;
            if detail.task.task_queue_key != queue {
                anyhow::bail!(
                    "{}: existing Blocking Task {identifier} must be in Task Queue {queue}",
                    item.source.display()
                );
            }
        }
        for key in &item.blocking_task_keys {
            if key.is_empty() {
                anyhow::bail!(
                    "{}: blocking_task_keys must not contain blanks",
                    item.source.display()
                );
            }
            if !key_to_index.contains_key(key) {
                anyhow::bail!(
                    "{}: same-batch Blocking Task key {key} not found",
                    item.source.display()
                );
            }
        }
    }

    let creation_order = topological_creation_order(&items, &key_to_index)?;
    Ok(ValidatedTaskBatch {
        items,
        creation_order,
    })
}

fn topological_creation_order(
    items: &[BatchItem],
    key_to_index: &HashMap<String, usize>,
) -> Result<Vec<usize>> {
    fn visit(
        index: usize,
        items: &[BatchItem],
        key_to_index: &HashMap<String, usize>,
        visiting: &mut HashSet<usize>,
        visited: &mut HashSet<usize>,
        order: &mut Vec<usize>,
        stack: &mut Vec<String>,
    ) -> Result<()> {
        if visited.contains(&index) {
            return Ok(());
        }
        if !visiting.insert(index) {
            let current = display_batch_node(&items[index]);
            stack.push(current.clone());
            anyhow::bail!(
                "cycle in same-batch Blocking Task graph: {}",
                stack.join(" blocks ")
            );
        }
        stack.push(display_batch_node(&items[index]));
        for key in &items[index].blocking_task_keys {
            let dependency_index = key_to_index[key];
            visit(
                dependency_index,
                items,
                key_to_index,
                visiting,
                visited,
                order,
                stack,
            )?;
        }
        stack.pop();
        visiting.remove(&index);
        visited.insert(index);
        order.push(index);
        Ok(())
    }

    let mut order = Vec::new();
    let mut visited = HashSet::new();
    for index in 0..items.len() {
        visit(
            index,
            items,
            key_to_index,
            &mut HashSet::new(),
            &mut visited,
            &mut order,
            &mut Vec::new(),
        )?;
    }
    Ok(order)
}

fn display_batch_node(item: &BatchItem) -> String {
    item.batch_key
        .clone()
        .unwrap_or_else(|| item.source.display().to_string())
}

fn print_batch_plan(batch: &ValidatedTaskBatch) {
    println!("dependency direction: blocked Task -> Blocking Task");
    println!("creation order: blockers before blocked Tasks");
    for index in &batch.creation_order {
        let item = &batch.items[*index];
        let key = item.batch_key.as_deref().unwrap_or("<none>");
        let existing = if item.task.blocking_task_identifiers.is_empty() {
            "<none>".to_string()
        } else {
            item.task.blocking_task_identifiers.join(", ")
        };
        let same_batch = if item.blocking_task_keys.is_empty() {
            "<none>".to_string()
        } else {
            item.blocking_task_keys.join(", ")
        };
        println!(
            "- {} (batch_key: {key}) blocks-on existing: {existing}; same-batch: {same_batch}",
            item.task.title
        );
    }
}
