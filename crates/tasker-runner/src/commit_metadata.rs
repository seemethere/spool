#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskerCommitTrailers {
    pub task_identifier: Option<String>,
    pub task_queue: Option<String>,
    pub agent_run_id: Option<String>,
}

pub const TASKER_TASK_TRAILER: &str = "Tasker-Task";
pub const TASKER_QUEUE_TRAILER: &str = "Tasker-Queue";
pub const TASKER_AGENT_RUN_TRAILER: &str = "Tasker-Agent-Run";

pub fn final_commit_message(
    task_identifier: &str,
    task_title: &str,
    task_queue: &str,
    agent_run_id: Option<&str>,
) -> String {
    let mut message = format!(
        "{task_identifier}: {task_title}\n\n{}: {task_identifier}\n{}: {task_queue}",
        TASKER_TASK_TRAILER, TASKER_QUEUE_TRAILER
    );
    if let Some(agent_run_id) = agent_run_id {
        message.push_str(&format!("\n{TASKER_AGENT_RUN_TRAILER:}: {agent_run_id}"));
    }
    message
}

pub fn parse_tasker_commit_trailers(message: &str) -> TaskerCommitTrailers {
    let mut trailers = TaskerCommitTrailers::default();
    for line in message.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            TASKER_TASK_TRAILER => trailers.task_identifier = Some(value.to_string()),
            TASKER_QUEUE_TRAILER => trailers.task_queue = Some(value.to_string()),
            TASKER_AGENT_RUN_TRAILER => trailers.agent_run_id = Some(value.to_string()),
            _ => {}
        }
    }
    trailers
}
