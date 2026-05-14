use crate::{tasks::normalized_task_identifiers, *};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DelegationTaskDraft {
    pub queue_key: String,
    pub title: String,
    #[serde(alias = "task_brief")]
    pub brief: String,
    #[serde(default = "default_priority")]
    pub priority: String,
    #[serde(default = "default_initial_state", alias = "state")]
    pub initial_state: String,
    #[serde(default)]
    pub review_required: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub conflict_hints: Vec<String>,
    #[serde(default, alias = "blocking_tasks", alias = "blockers")]
    pub blocking_task_identifiers: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub validation_items: Vec<String>,
}

fn default_priority() -> String {
    "normal".to_string()
}

fn default_initial_state() -> String {
    "backlog".to_string()
}

pub fn validate_delegation_task_draft(draft: &DelegationTaskDraft) -> Result<CreateTask> {
    ensure_not_blank("Task Queue Key", &draft.queue_key)?;
    ensure_not_blank("title", &draft.title)?;
    ensure_not_blank("Task Brief", &draft.brief)?;

    let priority = normalize_draft_label(&draft.priority);
    validate_priority(&priority)?;

    let state = normalize_draft_label(&draft.initial_state);
    if state != "backlog" && state != "ready" {
        anyhow::bail!("Delegation Task drafts only support Backlog or Ready initial Task States");
    }
    if state == "ready"
        && (draft.acceptance_criteria.is_empty() || draft.validation_items.is_empty())
    {
        anyhow::bail!(
            "Ready Task drafts require at least one Acceptance Criterion and one Validation Item; add structured acceptance_criteria and validation_items or use initial_state: backlog"
        );
    }

    let input = CreateTask {
        queue_key: draft.queue_key.trim().to_string(),
        title: draft.title.trim().to_string(),
        brief: draft.brief.trim().to_string(),
        priority,
        state,
        review_required: draft.review_required,
        acceptance_criteria: draft
            .acceptance_criteria
            .iter()
            .map(|criterion| criterion.trim().to_string())
            .collect(),
        validation_items: draft
            .validation_items
            .iter()
            .map(|item| item.trim().to_string())
            .collect(),
        tags: normalized_tags(&draft.tags),
        conflict_hints: normalized_conflict_hints(&draft.conflict_hints),
        blocking_task_identifiers: normalized_task_identifiers(&draft.blocking_task_identifiers),
    };
    validate_create_task(&input)?;
    Ok(input)
}

pub async fn create_delegated_root_task(
    pool: &SqlitePool,
    draft: &DelegationTaskDraft,
    actor: &Actor,
) -> Result<TaskDetail> {
    validate_delegation_actor(actor)?;
    let input = validate_delegation_task_draft(draft)?;
    create_task(pool, &input, actor).await
}

fn validate_delegation_actor(actor: &Actor) -> Result<()> {
    validate_actor(actor)?;
    if actor.kind == "operator" || actor.kind == "delegating_agent" {
        Ok(())
    } else {
        anyhow::bail!(
            "Delegation Task draft creation requires an Operator or Delegating Agent actor"
        )
    }
}

fn normalize_draft_label(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace([' ', '-'], "_")
}
