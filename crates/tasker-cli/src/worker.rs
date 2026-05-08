use anyhow::{bail, Result};
use sqlx::SqlitePool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkOnceRequest {
    pub queue: String,
    pub launcher: String,
    pub actor: String,
    pub fake_outcome: String,
    pub lease_seconds: i64,
    pub retry_hold_seconds: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkOnceOutcome {
    NoEligibleTask {
        queue: String,
    },
    Finished {
        task_identifier: String,
        run_id: String,
        outcome: String,
    },
}

pub async fn run_worker_once(
    pool: &SqlitePool,
    request: WorkOnceRequest,
) -> Result<WorkOnceOutcome> {
    if request.launcher != "fake" {
        bail!("only the fake Agent Launcher is available in this milestone");
    }

    let actor = tasker_db::Actor {
        kind: "worker_agent".to_string(),
        id: request.actor.clone(),
        display_name: request.actor.clone(),
    };
    let claim = tasker_db::claim_next(
        pool,
        &tasker_db::ClaimNextInput {
            queue_key: request.queue.clone(),
            worker_id: request.actor.clone(),
            launcher_kind: request.launcher,
            lease_seconds: request.lease_seconds,
        },
        &actor,
    )
    .await?;

    let Some(claimed) = claim else {
        return Ok(WorkOnceOutcome::NoEligibleTask {
            queue: request.queue,
        });
    };

    tasker_db::heartbeat_run(pool, &claimed.run.id, request.lease_seconds, &actor).await?;
    let fake_note = fake_workpad_note(
        &claimed.task.task.identifier,
        &claimed.run.id,
        &request.fake_outcome,
    );
    tasker_db::update_workpad_note(pool, &claimed.task.task.identifier, &fake_note, &actor).await?;
    let finished = tasker_db::finish_run(
        pool,
        &claimed.run.id,
        &tasker_db::FinishRunInput {
            outcome: request.fake_outcome,
            failure_reason: None,
            retry_hold_seconds: request.retry_hold_seconds,
        },
        &actor,
    )
    .await?;

    Ok(WorkOnceOutcome::Finished {
        task_identifier: claimed.task.task.identifier,
        run_id: finished.id,
        outcome: finished.outcome.unwrap_or_else(|| "unknown".to_string()),
    })
}

fn fake_workpad_note(task_identifier: &str, run_id: &str, outcome: &str) -> String {
    format!(
        "Fake Agent Launcher processed Task {task_identifier} in Agent Run {run_id}.\nOutcome: {outcome}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_workpad_note_is_deterministic() {
        assert_eq!(
            fake_workpad_note("TASK-1", "run-1", "completed"),
            "Fake Agent Launcher processed Task TASK-1 in Agent Run run-1.\nOutcome: completed\n"
        );
    }
}
