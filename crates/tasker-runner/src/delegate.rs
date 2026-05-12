use std::{fs, path::Path};

use anyhow::{Context, Result};

const DEFAULT_DELEGATING_ROLE_PROMPT: &str = "You are a Tasker Delegating Agent running in a human-present Delegation Session. You are not a Worker Agent executing an Agent Run, and you are not a Review Agent recording a Review Decision. Turn out-of-band human intent into one structured Tasker Task draft for another agent to execute later. Use Tasker domain language exactly: Root Task, Task Brief, Acceptance Criteria, Validation Items, Task Conflict Hints, Blocking Tasks, Task Queue, Task State, Delegation Interview, and Agent-Gated Integration. Run a one-question-at-a-time Delegation Interview, asking only for information needed to express a small executable Task. Read repository context docs such as CONTEXT.md, ROADMAP.md, and relevant ADRs when needed to use local-first Tasker terminology correctly, but do not edit repository files during delegation by default. If documentation or implementation changes are discovered, capture them as Acceptance Criteria, Validation Items, Task Conflict Hints, Workpad Note seed context, or candidate follow-up Tasks instead of making hidden source changes. Produce structured Task draft output only with supported Tasker fields: queue_key, title, brief, priority, initial_state, review_required, tags, conflict_hints, blocking_task_identifiers, acceptance_criteria, and validation_items. Do not collect unsupported planning fields such as due dates, estimates, milestones, assignees, custom workflows, GitHub metadata, pull requests, or external tracker data.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationPromptContext<'a> {
    pub queue_key: Option<&'a str>,
    pub refine_task_identifier: Option<&'a str>,
    pub managed_source_repository: &'a Path,
}

pub fn build_delegation_prompt(context: DelegationPromptContext<'_>) -> Result<String> {
    let override_path = context
        .managed_source_repository
        .join(".tasker/prompts/delegate.md");
    let base = if override_path.is_file() {
        fs::read_to_string(&override_path).with_context(|| {
            format!(
                "failed to read Delegating Agent Role Prompt override {}",
                override_path.display()
            )
        })?
    } else {
        DEFAULT_DELEGATING_ROLE_PROMPT.to_string()
    };

    let session_target = match (context.queue_key, context.refine_task_identifier) {
        (_, Some(identifier)) => format!(
            "Refinement target: {identifier}\nMode: refine an existing Backlog Task only. Do not revise active work in Ready, In Progress, Human Review, Rework, Integrating, Done, or Canceled."
        ),
        (Some(queue_key), None) => format!(
            "Task Queue Key: {queue_key}\nMode: create one new Root Task. The Task defaults to Backlog unless Ready is explicitly justified with structured Acceptance Criteria and Validation Items."
        ),
        (None, None) => "Task Queue Key: not selected yet\nMode: create one new Root Task after selecting the intended Task Queue.".to_string(),
    };

    Ok(format!(
        "{base}\n\nDelegation Session type: Interactive Agent Session\nQuestion UI is allowed because a human is intentionally present. Do not apply Unattended Worker Session question-failure handling here; that behavior remains only for Worker Loop launches.\n\n{session_target}\n\nDelegating Agent instructions:\n- Run a one-question-at-a-time Delegation Interview and ask at most one substantive question per turn.\n- Stop asking when the Task can be represented as clear structured Tasker data.\n- Create or refine only Tasker data through deterministic Tasker tooling, preferably the Tasker Pi Extension.\n- Keep structured Acceptance Criteria and Validation Items as authoritative fields; do not bury gates only in the Task Brief.\n- Use Backlog when requirements are incomplete; use Ready only when autonomous Worker Agent execution has enough structured requirements.\n- Prefer Agent-Gated Integration by leaving review_required false unless the human, Task, or Task Queue explicitly requires Human Review.\n- Do not claim to be a Worker Agent, Review Agent, Operator, or Subagent Review Loop reviewer.\n\nStructured Task draft fields:\n- queue_key\n- title\n- brief (Task Brief Markdown narrative; may include a short Workpad Note seed)\n- priority: urgent, high, normal, or low\n- initial_state: backlog or ready\n- review_required\n- tags\n- conflict_hints (advisory Task Conflict Hints / likely paths or docs)\n- blocking_task_identifiers (same Task Queue only)\n- acceptance_criteria\n- validation_items\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegation_prompt_uses_builtin_role_prompt_and_tasker_boundaries() {
        let temp = tempfile::tempdir().expect("tempdir");

        let prompt = build_delegation_prompt(DelegationPromptContext {
            queue_key: Some("TASKER"),
            refine_task_identifier: None,
            managed_source_repository: temp.path(),
        })
        .expect("prompt");

        assert!(prompt.contains("Tasker Delegating Agent"));
        assert!(prompt.contains("Interactive Agent Session"));
        assert!(prompt.contains("Question UI is allowed"));
        assert!(prompt.contains("one-question-at-a-time Delegation Interview"));
        assert!(prompt.contains("structured Task draft"));
        assert!(prompt.contains("Acceptance Criteria"));
        assert!(prompt.contains("Validation Items"));
        assert!(prompt.contains("Task Conflict Hints"));
        assert!(prompt.contains("Blocking Tasks"));
        assert!(prompt.contains("Task Queue Key: TASKER"));
        assert!(prompt.contains("Root Task"));
        assert!(prompt.contains("Agent-Gated Integration"));
        assert!(prompt.contains("not a Worker Agent"));
        assert!(prompt.contains("not a Review Agent"));
        assert!(prompt.contains("do not edit repository files during delegation by default"));
        assert!(prompt.contains("local-first Tasker terminology"));
        assert!(prompt.contains("Do not apply Unattended Worker Session question-failure handling"));
        assert!(prompt.contains("due dates"));
        assert!(prompt.contains("external tracker"));
    }

    #[test]
    fn delegation_prompt_uses_repo_owned_override() {
        let temp = tempfile::tempdir().expect("tempdir");
        let prompts = temp.path().join(".tasker/prompts");
        fs::create_dir_all(&prompts).expect("mkdir prompts");
        fs::write(
            prompts.join("delegate.md"),
            "Custom Delegating Agent prompt.",
        )
        .expect("write prompt");

        let prompt = build_delegation_prompt(DelegationPromptContext {
            queue_key: Some("TASKER"),
            refine_task_identifier: None,
            managed_source_repository: temp.path(),
        })
        .expect("prompt");

        assert!(prompt.starts_with("Custom Delegating Agent prompt."));
        assert!(prompt.contains("Question UI is allowed"));
        assert!(prompt.contains("Task Queue Key: TASKER"));
        assert!(prompt.contains("acceptance_criteria"));
        assert!(prompt.contains("validation_items"));
    }

    #[test]
    fn delegation_prompt_describes_backlog_refinement_scope() {
        let temp = tempfile::tempdir().expect("tempdir");

        let prompt = build_delegation_prompt(DelegationPromptContext {
            queue_key: None,
            refine_task_identifier: Some("TASKER-1"),
            managed_source_repository: temp.path(),
        })
        .expect("prompt");

        assert!(prompt.contains("Refinement target: TASKER-1"));
        assert!(prompt.contains("Backlog Task only"));
        assert!(prompt.contains("Do not revise active work"));
        assert!(prompt
            .contains("Ready, In Progress, Human Review, Rework, Integrating, Done, or Canceled"));
    }
}
