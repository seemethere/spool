use std::io::{Result, Write};

pub fn write_task_detail(mut writer: impl Write, detail: &tasker_db::TaskDetail) -> Result<()> {
    writeln!(writer, "Task: {}", detail.task.identifier)?;
    writeln!(writer, "title: {}", detail.task.title)?;
    writeln!(writer, "Task Queue: {}", detail.task.task_queue_key)?;
    writeln!(writer, "Task State: {}", detail.task.state)?;
    writeln!(writer, "Priority: {}", detail.task.priority)?;
    writeln!(writer, "review required: {}", detail.task.review_required)?;
    if !detail.tags.is_empty() {
        writeln!(writer, "tags: {}", detail.tags.join(", "))?;
    }
    writeln!(writer, "\nTask Brief:\n{}", detail.task.brief)?;
    writeln!(writer, "\nAcceptance Criteria:")?;
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
    writeln!(writer, "\nValidation Items:")?;
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
    writeln!(writer, "\nWorkpad Note:")?;
    if let Some(note) = &detail.workpad_note {
        writeln!(writer, "{}", note.body)?;
    } else {
        writeln!(writer, "(none)")?;
    }
    Ok(())
}

pub fn write_queue(mut writer: impl Write, queue: &tasker_db::TaskQueue) -> Result<()> {
    writeln!(writer, "key: {}", queue.key)?;
    writeln!(writer, "name: {}", queue.name)?;
    writeln!(writer, "delivery backend: {}", queue.delivery_backend)?;
    writeln!(
        writer,
        "managed source repository: {}",
        queue.managed_source_repository
    )?;
    writeln!(writer, "main branch: {}", queue.main_branch)?;
    writeln!(writer, "worktree root: {}", queue.worktree_root)?;
    writeln!(writer, "branch template: {}", queue.branch_template)?;
    writeln!(
        writer,
        "done worktree retention: {}",
        queue.done_worktree_retention
    )?;
    match queue.queue_concurrency_limit {
        Some(limit) => writeln!(writer, "Queue Concurrency Limit: {limit}"),
        None => writeln!(writer, "Queue Concurrency Limit: none"),
    }
}

pub fn print_task_detail(detail: &tasker_db::TaskDetail) -> Result<()> {
    write_task_detail(std::io::stdout(), detail)
}

pub fn print_queue(queue: &tasker_db::TaskQueue) -> Result<()> {
    write_queue(std::io::stdout(), queue)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_output_includes_concurrency_limit() {
        let queue = tasker_db::TaskQueue {
            id: "queue-id".to_string(),
            key: "TASK".to_string(),
            name: "Tasker".to_string(),
            delivery_backend: "local_worktree".to_string(),
            managed_source_repository: "/repo".to_string(),
            main_branch: "main".to_string(),
            worktree_root: "/worktrees".to_string(),
            branch_template: "tasker/{task_identifier}".to_string(),
            done_worktree_retention: false,
            queue_concurrency_limit: Some(1),
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
        };
        let mut out = Vec::new();

        write_queue(&mut out, &queue).expect("write queue");
        let text = String::from_utf8(out).expect("utf8");

        assert!(text.contains("key: TASK"));
        assert!(text.contains("Queue Concurrency Limit: 1"));
    }
}
