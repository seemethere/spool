use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result};

const DEFAULT_REVIEW_ROLE_PROMPT: &str = "You are a Spool Review Agent running in a human-present Review Session. You are not a Worker Agent and you are not a pre-dogfooding advisory subagent reviewer. Prepare or summarize the Review Packet, guide the human to one explicit approve or rework Review Decision, and record that Review Decision through the deterministic Spool review-decision path.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewSessionRequest {
    pub identifier: String,
    pub review_packet: String,
    pub managed_source_repository: PathBuf,
    pub api_url: String,
    pub api_token: String,
    pub actor: String,
    pub pi_bin: String,
    pub pi_extension: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewSessionOutcome {
    pub identifier: String,
    pub completed: bool,
    pub exit_code: Option<i32>,
}

pub async fn run_review_session(request: ReviewSessionRequest) -> Result<ReviewSessionOutcome> {
    let prompt = build_review_prompt(
        &request.identifier,
        &request.review_packet,
        &request.managed_source_repository,
    )?;
    let mut command = Command::new(&request.pi_bin);
    command.arg("--mode").arg("rpc");
    if let Some(extension) = &request.pi_extension {
        command.arg("--extension").arg(extension);
    }
    let mut child = command
        .current_dir(&request.managed_source_repository)
        .env("SPOOL_API_URL", &request.api_url)
        .env("SPOOL_API_TOKEN", &request.api_token)
        .env("SPOOL_ACTOR_KIND", "review_agent")
        .env("SPOOL_ACTOR_ID", &request.actor)
        .env("SPOOL_ACTOR_DISPLAY_NAME", &request.actor)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "failed to start Pi-backed Review Session process {}",
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
            .context("failed to write Review Agent Role Prompt to Pi RPC stdin")?;
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
            .context("failed to poll Pi-backed Review Session process")?
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
            "Pi-backed Review Session exited without agent_end{}",
            if stderr_text.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", stderr_text.trim())
            }
        );
    }

    Ok(ReviewSessionOutcome {
        identifier: request.identifier,
        completed,
        exit_code,
    })
}

fn build_review_prompt(
    identifier: &str,
    review_packet: &str,
    managed_source_repository: &Path,
) -> Result<String> {
    let override_path = managed_source_repository.join(".spool/prompts/review.md");
    let base = if override_path.is_file() {
        fs::read_to_string(&override_path).with_context(|| {
            format!(
                "failed to read Review Agent Role Prompt override {}",
                override_path.display()
            )
        })?
    } else {
        DEFAULT_REVIEW_ROLE_PROMPT.to_string()
    };
    Ok(format!(
        "{base}\n\nTask Identifier: {identifier}\nReview Session type: Interactive Agent Session\n\nQuestion UI is allowed in this Review Session because a human is intentionally present. Do not apply Unattended Worker Session question-failure handling here; that behavior remains only for Worker Loop launches.\n\nReview Agent instructions:\n- Present or summarize the Review Packet below for the human.\n- Ask the human for exactly one explicit Review Decision: approve or rework.\n- If the decision is rework, collect concise human feedback.\n- Record the Review Decision through the deterministic Spool review-decision path, preferably the Spool Pi Extension tool `tasker_record_review_decision`.\n- Use Review Agent actor attribution and do not claim to be a Worker Agent or Subagent Review Loop reviewer.\n\nReview Packet:\n{review_packet}\n"
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
    fn review_prompt_uses_builtin_prompt_and_review_packet() {
        let temp = tempfile::tempdir().expect("tempdir");

        let prompt = build_review_prompt("TASK-1", "Review Packet\nTask: TASK-1", temp.path())
            .expect("prompt");

        assert!(prompt.contains("Spool Review Agent"));
        assert!(prompt.contains("Interactive Agent Session"));
        assert!(prompt.contains("Question UI is allowed"));
        assert!(prompt.contains("approve or rework"));
        assert!(prompt.contains("tasker_record_review_decision"));
        assert!(prompt.contains("Review Packet\nTask: TASK-1"));
    }

    #[test]
    fn review_prompt_uses_repo_owned_override() {
        let temp = tempfile::tempdir().expect("tempdir");
        let prompts = temp.path().join(".spool/prompts");
        fs::create_dir_all(&prompts).expect("mkdir prompts");
        fs::write(prompts.join("review.md"), "Custom Review Agent prompt.").expect("write prompt");

        let prompt = build_review_prompt("TASK-1", "packet", temp.path()).expect("prompt");

        assert!(prompt.starts_with("Custom Review Agent prompt."));
        assert!(prompt.contains("Question UI is allowed"));
        assert!(prompt.contains("packet"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn review_session_launches_pi_with_review_actor_and_allows_question_ui_events() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pi_bin = temp.path().join("fake-pi");
        let capture = temp.path().join("capture.txt");
        write_executable(
            &pi_bin,
            &format!(
                r#"#!/bin/sh
cat > "{capture}"
printf '%s\n' '{{"type":"extension_ui_request","method":"select"}}'
printf '%s\n' '{{"type":"agent_end"}}'
printf '%s\n' "$SPOOL_ACTOR_KIND:$SPOOL_ACTOR_ID:$SPOOL_API_URL" >> "{capture}"
"#,
                capture = capture.display()
            ),
        );

        let outcome = run_review_session(ReviewSessionRequest {
            identifier: "TASK-1".to_string(),
            review_packet: "Review Packet".to_string(),
            managed_source_repository: temp.path().to_path_buf(),
            api_url: "http://spool.test".to_string(),
            api_token: "token".to_string(),
            actor: "reviewer".to_string(),
            pi_bin: pi_bin.display().to_string(),
            pi_extension: None,
        })
        .await
        .expect("review session");

        assert!(outcome.completed);
        let captured = fs::read_to_string(capture).expect("capture");
        assert!(captured.contains("Review Packet"));
        assert!(captured.contains("review_agent:reviewer:http://spool.test"));
    }
}
