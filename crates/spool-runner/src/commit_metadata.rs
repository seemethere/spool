#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpoolCommitTrailers {
    pub task_identifier: Option<String>,
    pub task_queue: Option<String>,
    pub agent_run_id: Option<String>,
}

pub const SPOOL_TASK_TRAILER: &str = "Spool-Task";
pub const SPOOL_QUEUE_TRAILER: &str = "Spool-Queue";
pub const SPOOL_AGENT_RUN_TRAILER: &str = "Spool-Agent-Run";

const BRIEF_EXCERPT_CHAR_LIMIT: usize = 360;
const REQUIREMENT_CHAR_LIMIT: usize = 160;
const REQUIREMENT_ITEM_LIMIT: usize = 2;

pub fn final_commit_message(
    task_identifier: &str,
    task_title: &str,
    task_queue: &str,
    agent_run_id: Option<&str>,
) -> String {
    final_commit_message_with_context(
        task_identifier,
        task_title,
        task_queue,
        agent_run_id,
        None,
        &[],
        &[],
    )
}

pub fn final_commit_message_for_task(
    task: &spool_db::TaskDetail,
    agent_run_id: Option<&str>,
) -> String {
    let acceptance_criteria = task
        .acceptance_criteria
        .iter()
        .map(|criterion| criterion.description.as_str())
        .collect::<Vec<_>>();
    let validation_items = task
        .validation_items
        .iter()
        .map(|item| item.description.as_str())
        .collect::<Vec<_>>();

    final_commit_message_with_context(
        &task.task.identifier,
        &task.task.title,
        &task.task.task_queue_key,
        agent_run_id,
        Some(task.task.brief.as_str()),
        &acceptance_criteria,
        &validation_items,
    )
}

pub fn final_commit_message_with_context(
    task_identifier: &str,
    task_title: &str,
    task_queue: &str,
    agent_run_id: Option<&str>,
    task_brief: Option<&str>,
    acceptance_criteria: &[&str],
    validation_items: &[&str],
) -> String {
    let title = bounded_clean_text(task_title, REQUIREMENT_CHAR_LIMIT)
        .unwrap_or_else(|| "Task".to_string());
    let mut message = format!("{task_identifier}: {title}\n\n");

    let context_lines = task_context_lines(task_brief, acceptance_criteria, validation_items);
    if !context_lines.is_empty() {
        message.push_str("Task context:\n");
        for line in context_lines {
            message.push_str(&line);
            message.push('\n');
        }
        message.push('\n');
    }

    message.push_str(&format!(
        "{}: {task_identifier}\n{}: {task_queue}",
        SPOOL_TASK_TRAILER, SPOOL_QUEUE_TRAILER
    ));
    if let Some(agent_run_id) = agent_run_id {
        message.push_str(&format!("\n{SPOOL_AGENT_RUN_TRAILER:}: {agent_run_id}"));
    }
    message
}

fn task_context_lines(
    task_brief: Option<&str>,
    acceptance_criteria: &[&str],
    validation_items: &[&str],
) -> Vec<String> {
    let mut lines = Vec::new();

    if let Some(brief) = task_brief.and_then(|brief| bounded_brief_excerpt(brief)) {
        lines.push(format!("- Brief: {brief}"));
    }

    for (index, criterion) in acceptance_criteria
        .iter()
        .filter_map(|criterion| bounded_clean_text(criterion, REQUIREMENT_CHAR_LIMIT))
        .take(REQUIREMENT_ITEM_LIMIT)
        .enumerate()
    {
        lines.push(format!("- Acceptance {}: {criterion}", index + 1));
    }

    for (index, item) in validation_items
        .iter()
        .filter_map(|item| bounded_clean_text(item, REQUIREMENT_CHAR_LIMIT))
        .take(REQUIREMENT_ITEM_LIMIT)
        .enumerate()
    {
        lines.push(format!("- Validation {}: {item}", index + 1));
    }

    lines
}

fn bounded_brief_excerpt(brief: &str) -> Option<String> {
    let summary = brief
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with('#'))
        .collect::<Vec<_>>()
        .join(" ");
    bounded_clean_text(&summary, BRIEF_EXCERPT_CHAR_LIMIT)
}

fn bounded_clean_text(text: &str, char_limit: usize) -> Option<String> {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let redacted = redact_sensitive_tokens(&collapsed);
    let cleaned = redacted.trim();
    if cleaned.is_empty() || looks_like_sensitive_line(cleaned) {
        return None;
    }

    let mut output = String::new();
    for ch in cleaned.chars().take(char_limit) {
        output.push(ch);
    }
    if cleaned.chars().count() > char_limit {
        output.push('…');
    }
    Some(output)
}

fn redact_sensitive_tokens(text: &str) -> String {
    text.split_whitespace()
        .map(|token| {
            let lower = token.to_ascii_lowercase();
            if (lower.contains("password")
                || lower.contains("secret")
                || lower.contains("api_key")
                || lower.contains("apikey")
                || lower.contains("token"))
                && (lower.contains('=') || lower.contains(':'))
            {
                "[redacted]"
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn looks_like_sensitive_line(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("-----begin ")
        || lower.contains("authorization: bearer")
        || lower.contains("bearer ")
        || lower.contains("private key")
}

pub fn parse_spool_commit_trailers(message: &str) -> SpoolCommitTrailers {
    let mut trailers = SpoolCommitTrailers::default();
    for line in message.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            SPOOL_TASK_TRAILER => trailers.task_identifier = Some(value.to_string()),
            SPOOL_QUEUE_TRAILER => trailers.task_queue = Some(value.to_string()),
            SPOOL_AGENT_RUN_TRAILER => trailers.agent_run_id = Some(value.to_string()),
            _ => {}
        }
    }
    trailers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spool_commit_metadata_trailers_parse_complete_partial_and_unrelated_messages() {
        let complete = "feat: deliver task\n\nSpool-Task: SPOOL-92\nSpool-Queue: SPOOL\nSpool-Agent-Run: run-123\n";
        assert_eq!(
            parse_spool_commit_trailers(complete),
            SpoolCommitTrailers {
                task_identifier: Some("SPOOL-92".to_string()),
                task_queue: Some("SPOOL".to_string()),
                agent_run_id: Some("run-123".to_string()),
            }
        );

        let partial = "fix: partial\n\nSpool-Task: SPOOL-93\nReviewed-by: Operator\n";
        assert_eq!(
            parse_spool_commit_trailers(partial),
            SpoolCommitTrailers {
                task_identifier: Some("SPOOL-93".to_string()),
                task_queue: None,
                agent_run_id: None,
            }
        );

        assert_eq!(
            parse_spool_commit_trailers("docs: unrelated\n\nReviewed-by: Operator"),
            SpoolCommitTrailers::default()
        );
    }

    #[test]
    fn generated_spool_commit_metadata_is_trailer_compatible_and_minimal() {
        let message = final_commit_message(
            "SPOOL-92",
            "Add structured Spool metadata trailers to Final Commit messages",
            "SPOOL",
            Some("5d019294-398e-4f89-ad70-9b434b10dadb"),
        );

        assert!(message.starts_with("SPOOL-92: Add structured Spool metadata trailers"));
        assert!(message.contains("\n\nSpool-Task: SPOOL-92\n"));
        assert!(message.contains("Spool-Queue: SPOOL\n"));
        assert!(message.contains("Spool-Agent-Run: 5d019294-398e-4f89-ad70-9b434b10dadb"));
        assert!(!message.contains("Workpad"));
        assert!(!message.contains("Run Transcript"));
        assert!(!message.contains("prompt"));
        assert_eq!(
            parse_spool_commit_trailers(&message).task_identifier,
            Some("SPOOL-92".to_string())
        );
    }

    #[test]
    fn generated_spool_commit_metadata_includes_bounded_sanitized_task_context() {
        let message = final_commit_message_with_context(
            "SPOOL-174",
            "Enrich Final Commit messages with safe Task context",
            "SPOOL",
            Some("run-174"),
            Some(
                "# Task Brief\n\nCurrent Final Commit messages are hard to grok when browsing Git history.\n\npassword=hunter2 should not leak.",
            ),
            &[
                "Automatic Local Worktree Delivery Final Commit messages include enough sanitized Task context to understand the Task without opening Spool first",
                "Commit message format preserves machine-parseable Spool trailers",
                "A third criterion is omitted to keep the message compact",
            ],
            &[
                "cargo test -p spool-runner commit_metadata local_worktree_delivery passes",
                "git interpret-trailers can still parse generated Final Commit trailers in tests",
            ],
        );

        assert!(message.contains("Task context:\n"));
        assert!(message.contains("- Brief: Current Final Commit messages are hard to grok"));
        assert!(message.contains("[redacted]"));
        assert!(!message.contains("hunter2"));
        assert!(message.contains("- Acceptance 1: Automatic Local Worktree Delivery"));
        assert!(message.contains("- Acceptance 2: Commit message format preserves"));
        assert!(!message.contains("A third criterion"));
        assert!(message.contains("- Validation 1: cargo test -p spool-runner"));
        assert!(message.contains("\n\nSpool-Task: SPOOL-174\n"));
        assert_eq!(
            parse_spool_commit_trailers(&message),
            SpoolCommitTrailers {
                task_identifier: Some("SPOOL-174".to_string()),
                task_queue: Some("SPOOL".to_string()),
                agent_run_id: Some("run-174".to_string()),
            }
        );
    }
}
