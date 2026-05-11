use super::*;

pub(crate) async fn queue(
    paths: &TaskerPaths,
    db_path_overridden: bool,
    command: QueueCommand,
) -> Result<()> {
    let pool = open_pool(paths, db_path_overridden).await?;

    match command {
        QueueCommand::Create {
            key,
            name,
            managed_source_repository,
            main_branch,
            worktree_root,
            branch_template,
            done_worktree_retention,
            queue_concurrency_limit,
            actor,
        } => {
            println!(
                "warning: Local Worktree Delivery may mutate Managed Source Repository {}",
                managed_source_repository.display()
            );
            let input = tasker_db::CreateTaskQueue {
                key,
                name,
                managed_source_repository: managed_source_repository.display().to_string(),
                main_branch,
                worktree_root: worktree_root.display().to_string(),
                branch_template,
                done_worktree_retention,
                queue_concurrency_limit,
            };
            let queue =
                tasker_db::create_task_queue(&pool, &input, &tasker_db::Actor::operator(actor))
                    .await?;
            output::print_queue(&queue)?;
        }
        QueueCommand::Show { key } => {
            let queue = tasker_db::get_task_queue(&pool, &key)
                .await?
                .with_context(|| format!("Task Queue {key} not found"))?;
            output::print_queue(&queue)?;
        }
        QueueCommand::Update {
            key,
            queue_concurrency_limit,
            clear_queue_concurrency_limit,
            actor,
        } => {
            if queue_concurrency_limit.is_none() && !clear_queue_concurrency_limit {
                anyhow::bail!(
                    "queue update requires --queue-concurrency-limit or --clear-queue-concurrency-limit"
                );
            }
            let limit = if clear_queue_concurrency_limit {
                None
            } else {
                queue_concurrency_limit
            };
            let queue = tasker_db::update_task_queue_concurrency_limit(
                &pool,
                &key,
                &tasker_db::UpdateQueueConcurrencyLimit {
                    queue_concurrency_limit: limit,
                },
                &tasker_db::Actor::operator(actor),
            )
            .await?;
            output::print_queue(&queue)?;
        }
        QueueCommand::Audit { key } => {
            let events = tasker_db::list_task_queue_audit_events(&pool, &key).await?;
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
        QueueCommand::List => {
            let queues = tasker_db::list_task_queues(&pool).await?;
            for queue in queues {
                println!("{}\t{}", queue.key, queue.name);
            }
        }
    }

    Ok(())
}
