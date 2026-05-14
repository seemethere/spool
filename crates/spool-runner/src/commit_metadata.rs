#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpoolCommitTrailers {
    pub task_identifier: Option<String>,
    pub task_queue: Option<String>,
    pub agent_run_id: Option<String>,
}

pub const SPOOL_TASK_TRAILER: &str = "Spool-Task";
pub const SPOOL_QUEUE_TRAILER: &str = "Spool-Queue";
pub const SPOOL_AGENT_RUN_TRAILER: &str = "Spool-Agent-Run";

pub fn final_commit_message(
    task_identifier: &str,
    task_title: &str,
    task_queue: &str,
    agent_run_id: Option<&str>,
) -> String {
    let mut message = format!(
        "{task_identifier}: {task_title}\n\n{}: {task_identifier}\n{}: {task_queue}",
        SPOOL_TASK_TRAILER, SPOOL_QUEUE_TRAILER
    );
    if let Some(agent_run_id) = agent_run_id {
        message.push_str(&format!("\n{SPOOL_AGENT_RUN_TRAILER:}: {agent_run_id}"));
    }
    message
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
        let complete = "feat: deliver task\n\nSpool-Task: TASKER-92\nSpool-Queue: TASKER\nSpool-Agent-Run: run-123\n";
        assert_eq!(
            parse_spool_commit_trailers(complete),
            SpoolCommitTrailers {
                task_identifier: Some("TASKER-92".to_string()),
                task_queue: Some("TASKER".to_string()),
                agent_run_id: Some("run-123".to_string()),
            }
        );

        let partial = "fix: partial\n\nSpool-Task: TASKER-93\nReviewed-by: Operator\n";
        assert_eq!(
            parse_spool_commit_trailers(partial),
            SpoolCommitTrailers {
                task_identifier: Some("TASKER-93".to_string()),
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
            "TASKER-92",
            "Add structured Spool metadata trailers to Final Commit messages",
            "TASKER",
            Some("5d019294-398e-4f89-ad70-9b434b10dadb"),
        );

        assert!(message.starts_with("TASKER-92: Add structured Spool metadata trailers"));
        assert!(message.contains("\n\nSpool-Task: TASKER-92\n"));
        assert!(message.contains("Spool-Queue: TASKER\n"));
        assert!(message.contains("Spool-Agent-Run: 5d019294-398e-4f89-ad70-9b434b10dadb"));
        assert!(!message.contains("Workpad"));
        assert!(!message.contains("Run Transcript"));
        assert!(!message.contains("prompt"));
        assert_eq!(
            parse_spool_commit_trailers(&message).task_identifier,
            Some("TASKER-92".to_string())
        );
    }
}
