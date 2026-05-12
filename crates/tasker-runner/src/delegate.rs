use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result};

const DEFAULT_DELEGATING_ROLE_PROMPT: &str = "You are a Tasker Delegating Agent running in a human-present Delegation Session. You are not a Worker Agent executing an Agent Run, and you are not a Review Agent recording a Review Decision. Turn out-of-band human intent into one structured Tasker Task draft for another agent to execute later. Use Tasker domain language exactly: Root Task, Task Brief, Acceptance Criteria, Validation Items, Task Conflict Hints, Blocking Tasks, Task Queue, Task State, Delegation Interview, and Agent-Gated Integration. Run a one-question-at-a-time Delegation Interview, asking only for information needed to express a small executable Task. Read repository context docs such as CONTEXT.md, ROADMAP.md, and relevant ADRs when needed to use local-first Tasker terminology correctly, but do not edit repository files during delegation by default. If documentation or implementation changes are discovered, capture them as Acceptance Criteria, Validation Items, Task Conflict Hints, Workpad Note seed context, or candidate follow-up Tasks instead of making hidden source changes. Produce structured Task draft output only with supported Tasker fields: queue_key, title, brief, priority, initial_state, review_required, tags, conflict_hints, blocking_task_identifiers, acceptance_criteria, and validation_items. Do not collect unsupported planning fields such as due dates, estimates, milestones, assignees, custom workflows, GitHub metadata, pull requests, or external tracker data.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationPromptContext<'a> {
    pub queue_key: Option<&'a str>,
    pub refine_task_identifier: Option<&'a str>,
    pub managed_source_repository: &'a Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationSessionRequest {
    pub queue_key: Option<String>,
    pub refine_task_identifier: Option<String>,
    pub existing_task_context: Option<String>,
    pub managed_source_repository: PathBuf,
    pub api_url: String,
    pub api_token: String,
    pub actor: String,
    pub pi_bin: String,
    pub pi_extension: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationSessionOutcome {
    pub queue_key: Option<String>,
    pub refine_task_identifier: Option<String>,
    pub completed: bool,
    pub exit_code: Option<i32>,
}

pub async fn run_delegation_session(
    request: DelegationSessionRequest,
) -> Result<DelegationSessionOutcome> {
    let mut prompt = build_delegation_prompt(DelegationPromptContext {
        queue_key: request.queue_key.as_deref(),
        refine_task_identifier: request.refine_task_identifier.as_deref(),
        managed_source_repository: &request.managed_source_repository,
    })?;
    if let Some(existing_task_context) = &request.existing_task_context {
        prompt.push_str("\nExisting Backlog Task context for refinement:\n");
        prompt.push_str(existing_task_context);
        prompt.push('\n');
    }

    let mut command = Command::new(&request.pi_bin);
    command.arg("--mode").arg("rpc");
    if let Some(extension) = &request.pi_extension {
        command.arg("--extension").arg(extension);
    }
    let mut child = command
        .current_dir(&request.managed_source_repository)
        .env("TASKER_API_URL", &request.api_url)
        .env("TASKER_API_TOKEN", &request.api_token)
        .env("TASKER_ACTOR_KIND", "delegating_agent")
        .env("TASKER_ACTOR_ID", &request.actor)
        .env("TASKER_ACTOR_DISPLAY_NAME", &request.actor)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "failed to start Pi-backed Delegation Session process {}",
                request.pi_bin
            )
        })?;

    if let Some(stdin) = child.stdin.as_mut() {
        let rpc_start = format!(
            "{}\n",
            serde_json::json!({ "type": "prompt", "message": prompt })
        );
        stdin
            .write_all(rpc_start.as_bytes())
            .context("failed to write Delegating Agent Role Prompt to Pi RPC stdin")?;
    }
    drop(child.stdin.take());

    let stdout = Arc::new(Mutex::new(String::new()));
    let stderr = Arc::new(Mutex::new(String::new()));
    let stdout_thread = child
        .stdout
        .take()
        .map(|pipe| spawn_reader(pipe, Arc::clone(&stdout)));
    let stderr_thread = child
        .stderr
        .take()
        .map(|pipe| spawn_reader(pipe, Arc::clone(&stderr)));

    let (completed, exit_code) = loop {
        if scan_agent_end(&locked_string(&stdout)) {
            let _ = child.kill();
            let exit_code = child
                .wait()
                .ok()
                .and_then(|status| status.code())
                .or(Some(0));
            break (true, exit_code);
        }
        if let Some(status) = child
            .try_wait()
            .context("failed to poll Pi-backed Delegation Session process")?
        {
            let exit_code = status.code();
            let completed = status.success() && scan_agent_end(&locked_string(&stdout));
            break (completed, exit_code);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    if let Some(handle) = stdout_thread {
        let _ = handle.join();
    }
    if let Some(handle) = stderr_thread {
        let _ = handle.join();
    }

    if !completed {
        let stderr_text = locked_string(&stderr);
        anyhow::bail!(
            "Pi-backed Delegation Session exited without agent_end{}",
            if stderr_text.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", stderr_text.trim())
            }
        );
    }

    Ok(DelegationSessionOutcome {
        queue_key: request.queue_key,
        refine_task_identifier: request.refine_task_identifier,
        completed,
        exit_code,
    })
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
            "Refinement target: {identifier}\nMode: refine an existing Backlog Task only. Do not revise active work in Ready, In Progress, Human Review, Rework, Integrating, Done, or Canceled. Use the Tasker Pi Extension tool `tasker_refine_backlog_task` for deterministic refinement."
        ),
        (Some(queue_key), None) => format!(
            "Task Queue Key: {queue_key}\nMode: create one new Root Task. The Task defaults to Backlog unless Ready is explicitly justified with structured Acceptance Criteria and Validation Items. Use the Tasker Pi Extension tool `tasker_create_delegated_root_task` for deterministic creation."
        ),
        (None, None) => "Task Queue Key: not selected yet\nMode: create one new Root Task after selecting the intended Task Queue.".to_string(),
    };

    Ok(format!(
        "{base}\n\nDelegation Session type: Interactive Agent Session\nQuestion UI is allowed because a human is intentionally present. Do not apply Unattended Worker Session question-failure handling here; that behavior remains only for Worker Loop launches.\n\n{session_target}\n\nDelegating Agent instructions:\n- Run a one-question-at-a-time Delegation Interview and ask at most one substantive question per turn.\n- Stop asking when the Task can be represented as clear structured Tasker data.\n- Create or refine only Tasker data through deterministic Tasker tooling, preferably the Tasker Pi Extension.\n- Keep structured Acceptance Criteria and Validation Items as authoritative fields; do not bury gates only in the Task Brief.\n- Use Backlog when requirements are incomplete; use Ready only when autonomous Worker Agent execution has enough structured requirements.\n- Prefer Agent-Gated Integration by leaving review_required false unless the human, Task, or Task Queue explicitly requires Human Review.\n- Do not claim to be a Worker Agent, Review Agent, Operator, or Subagent Review Loop reviewer.\n\nStructured Task draft fields:\n- queue_key\n- title\n- brief (Task Brief Markdown narrative; may include a short Workpad Note seed)\n- priority: urgent, high, normal, or low\n- initial_state: backlog or ready\n- review_required\n- tags\n- conflict_hints (advisory Task Conflict Hints / likely paths or docs)\n- blocking_task_identifiers (same Task Queue only)\n- acceptance_criteria\n- validation_items\n"
    ))
}

fn scan_agent_end(output: &str) -> bool {
    output.lines().any(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|value| {
                value
                    .get("type")
                    .and_then(|kind| kind.as_str())
                    .map(str::to_owned)
            })
            .as_deref()
            == Some("agent_end")
    })
}

fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
    output: Arc<Mutex<String>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let chunk = String::from_utf8_lossy(&buffer[..count]);
                    if let Ok(mut locked) = output.lock() {
                        locked.push_str(&chunk);
                    }
                }
                Err(_) => break,
            }
        }
    })
}

fn locked_string(output: &Arc<Mutex<String>>) -> String {
    output.lock().map(|text| text.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn write_executable(path: &Path, body: &str) {
        fs::write(path, body).expect("write script");
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod");
    }

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
        assert!(prompt.contains("tasker_create_delegated_root_task"));
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
        assert!(prompt.contains("tasker_refine_backlog_task"));
        assert!(prompt.contains("Backlog Task only"));
        assert!(prompt.contains("Do not revise active work"));
        assert!(prompt
            .contains("Ready, In Progress, Human Review, Rework, Integrating, Done, or Canceled"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn delegation_session_launches_pi_with_delegating_actor_and_allows_question_ui_events() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pi_bin = temp.path().join("fake-pi");
        let capture = temp.path().join("capture.txt");
        write_executable(
            &pi_bin,
            &format!(
                r#"#!/bin/sh
cat > "{capture}"
printf '%s\n' '{{"type":"extension_ui_request","method":"input"}}'
printf '%s\n' '{{"type":"agent_end"}}'
printf '%s\n' "$TASKER_ACTOR_KIND:$TASKER_ACTOR_ID:$TASKER_API_URL" >> "{capture}"
"#,
                capture = capture.display()
            ),
        );

        let outcome = run_delegation_session(DelegationSessionRequest {
            queue_key: Some("TASK".to_string()),
            refine_task_identifier: None,
            existing_task_context: None,
            managed_source_repository: temp.path().to_path_buf(),
            api_url: "http://tasker.test".to_string(),
            api_token: "token".to_string(),
            actor: "delegator".to_string(),
            pi_bin: pi_bin.display().to_string(),
            pi_extension: None,
        })
        .await
        .expect("delegation session");

        assert!(outcome.completed);
        let captured = fs::read_to_string(capture).expect("capture");
        assert!(captured.contains("Tasker Delegating Agent"));
        assert!(captured.contains("tasker_create_delegated_root_task"));
        assert!(captured.contains("delegating_agent:delegator:http://tasker.test"));
    }
}
