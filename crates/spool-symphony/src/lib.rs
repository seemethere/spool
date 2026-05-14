//! Thin Symphony Integration boundary for Spool.
//!
//! This crate contains Symphony-specific mapping types and adapter-facing
//! helpers. Spool core crates keep using Spool-native domain language and do
//! not depend on this crate.

use serde::{Deserialize, Serialize};

/// Symphony-facing snapshot of a Spool Task.
///
/// This is intentionally a projection over Spool data rather than an
/// authoritative domain model. Callers should mutate Spool through the Spool
/// API and use this crate only at the Symphony Integration boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SymphonyTaskSnapshot {
    pub identifier: String,
    pub title: String,
    pub brief: String,
    pub state: SymphonyTaskState,
    pub priority: String,
    pub review_required: bool,
    pub tags: Vec<String>,
    pub acceptance_criteria: Vec<SymphonyRequirementSnapshot>,
    pub validation_items: Vec<SymphonyRequirementSnapshot>,
    pub workpad_note: Option<String>,
    pub local_worktree: Option<String>,
    pub task_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SymphonyRequirementSnapshot {
    pub position: i64,
    pub description: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SymphonyTaskState {
    Backlog,
    Ready,
    InProgress,
    HumanReview,
    Rework,
    Integrating,
    Done,
    Canceled,
    Unknown(String),
}

impl From<&str> for SymphonyTaskState {
    fn from(value: &str) -> Self {
        match value {
            "backlog" => Self::Backlog,
            "ready" => Self::Ready,
            "in_progress" => Self::InProgress,
            "human_review" => Self::HumanReview,
            "rework" => Self::Rework,
            "integrating" => Self::Integrating,
            "done" => Self::Done,
            "canceled" => Self::Canceled,
            other => Self::Unknown(other.to_string()),
        }
    }
}

pub fn map_task_detail(detail: &spool_db::TaskDetail) -> SymphonyTaskSnapshot {
    SymphonyTaskSnapshot {
        identifier: detail.task.identifier.clone(),
        title: detail.task.title.clone(),
        brief: detail.task.brief.clone(),
        state: SymphonyTaskState::from(detail.task.state.as_str()),
        priority: detail.task.priority.clone(),
        review_required: detail.task.review_required,
        tags: detail.tags.clone(),
        acceptance_criteria: detail
            .acceptance_criteria
            .iter()
            .map(|criterion| SymphonyRequirementSnapshot {
                position: criterion.position,
                description: criterion.description.clone(),
                status: criterion.status.clone(),
            })
            .collect(),
        validation_items: detail
            .validation_items
            .iter()
            .map(|item| SymphonyRequirementSnapshot {
                position: item.position,
                description: item.description.clone(),
                status: item.status.clone(),
            })
            .collect(),
        workpad_note: detail.workpad_note.as_ref().map(|note| note.body.clone()),
        local_worktree: None,
        task_branch: None,
    }
}

pub fn map_context_bundle(bundle: &spool_db::TaskContextBundle) -> SymphonyTaskSnapshot {
    let mut snapshot = map_task_detail(&bundle.task);
    snapshot.local_worktree = bundle.local_workflow.local_worktree.clone();
    snapshot.task_branch = bundle.local_workflow.task_branch.clone();
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_spool_task_states() {
        assert_eq!(SymphonyTaskState::from("ready"), SymphonyTaskState::Ready);
        assert_eq!(
            SymphonyTaskState::from("human_review"),
            SymphonyTaskState::HumanReview
        );
        assert_eq!(
            SymphonyTaskState::from("unexpected"),
            SymphonyTaskState::Unknown("unexpected".to_string())
        );
    }
}
