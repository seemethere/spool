use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use sqlx::SqlitePool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkOnceRequest {
    pub queue: String,
    pub launcher: String,
    pub actor: String,
    pub fake_outcome: String,
    pub lease_seconds: i64,
    pub retry_hold_seconds: Option<i64>,
    pub max_run_seconds: Option<u64>,
    pub data_dir: PathBuf,
    pub api_url: String,
    pub api_token: String,
    pub pi_bin: String,
    pub pi_extension: Option<PathBuf>,
    pub worker_prompt: Option<PathBuf>,
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
    if request.launcher != "fake" && request.launcher != "pi" {
        bail!(
            "unsupported Agent Launcher {}; expected fake or pi",
            request.launcher
        );
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
            launcher_kind: request.launcher.clone(),
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

    let data_dir =
        absolute_path(&request.data_dir).context("failed to resolve Tasker data directory")?;
    let prepared_worktree =
        match prepare_local_worktree(pool, &claimed.task, &actor, &data_dir).await {
            Ok(prepared) => prepared,
            Err(error) => {
                let failure_reason = format!("Local Worktree setup failed after claim: {error:#}");
                let finished = tasker_db::finish_run(
                    pool,
                    &claimed.run.id,
                    &tasker_db::FinishRunInput {
                        outcome: "failed".to_string(),
                        failure_reason: Some(failure_reason),
                        retry_hold_seconds: request.retry_hold_seconds,
                    },
                    &actor,
                )
                .await?;
                return Ok(WorkOnceOutcome::Finished {
                    task_identifier: claimed.task.task.identifier,
                    run_id: finished.id,
                    outcome: finished.outcome.unwrap_or_else(|| "unknown".to_string()),
                });
            }
        };
    tasker_db::heartbeat_run(pool, &claimed.run.id, request.lease_seconds, &actor).await?;
    let transcript_dir = data_dir.join("runs").join(&claimed.run.id);
    fs::create_dir_all(&transcript_dir).with_context(|| {
        format!(
            "failed to create Run Transcript directory {}",
            transcript_dir.display()
        )
    })?;

    let launcher_result = if request.launcher == "fake" {
        run_fake_launcher(pool, &request, &claimed, &actor, &transcript_dir).await?
    } else {
        run_pi_launcher(
            pool,
            &request,
            &claimed,
            &actor,
            &prepared_worktree,
            &transcript_dir,
        )
        .await?
    };

    tasker_db::upsert_launcher_session_data(
        pool,
        &claimed.run.id,
        &launcher_result.session_data,
        &actor,
    )
    .await?;
    let finished = tasker_db::finish_run(
        pool,
        &claimed.run.id,
        &tasker_db::FinishRunInput {
            outcome: launcher_result.outcome,
            failure_reason: launcher_result.failure_reason,
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

struct LauncherResult {
    outcome: String,
    failure_reason: Option<String>,
    session_data: tasker_db::UpsertLauncherSessionData,
}

async fn run_fake_launcher(
    pool: &SqlitePool,
    request: &WorkOnceRequest,
    claimed: &tasker_db::ClaimedRun,
    actor: &tasker_db::Actor,
    transcript_dir: &Path,
) -> Result<LauncherResult> {
    let fake_note = fake_workpad_note(
        &claimed.task.task.identifier,
        &claimed.run.id,
        &request.fake_outcome,
    );
    tasker_db::update_workpad_note(pool, &claimed.task.task.identifier, &fake_note, actor).await?;
    let transcript_path = transcript_dir.join("fake.jsonl");
    fs::write(
        &transcript_path,
        format!(
            "{}\n",
            serde_json::json!({
                "launcher": "fake",
                "task_identifier": claimed.task.task.identifier,
                "agent_run_id": claimed.run.id,
                "outcome": request.fake_outcome,
            })
        ),
    )
    .with_context(|| {
        format!(
            "failed to write Run Transcript {}",
            transcript_path.display()
        )
    })?;
    Ok(LauncherResult {
        outcome: request.fake_outcome.clone(),
        failure_reason: None,
        session_data: tasker_db::UpsertLauncherSessionData {
            launcher_kind: "fake".to_string(),
            session_id: Some(claimed.run.id.clone()),
            model: None,
            provider: None,
            started_at: Some(claimed.run.created_at.clone()),
            finished_at: None,
            final_status: Some(request.fake_outcome.clone()),
            transcript_path: Some(transcript_path.display().to_string()),
            raw_json: Some(serde_json::json!({"fake_outcome": request.fake_outcome}).to_string()),
        },
    })
}

async fn run_pi_launcher(
    pool: &SqlitePool,
    request: &WorkOnceRequest,
    claimed: &tasker_db::ClaimedRun,
    actor: &tasker_db::Actor,
    prepared_worktree: &PreparedLocalWorktree,
    transcript_dir: &Path,
) -> Result<LauncherResult> {
    let transcript_path = transcript_dir.join("pi.jsonl");
    let prompt = build_worker_prompt(
        &claimed.task,
        &claimed.run,
        &prepared_worktree.path,
        &prepared_worktree.shared_cargo_target_dir,
        request.worker_prompt.as_deref(),
    )?;
    fs::create_dir_all(&prepared_worktree.shared_cargo_target_dir).with_context(|| {
        format!(
            "failed to create shared Cargo target directory {}",
            prepared_worktree.shared_cargo_target_dir.display()
        )
    })?;
    let mut command = Command::new(&request.pi_bin);
    command.arg("--mode").arg("rpc");
    if let Some(extension) = &request.pi_extension {
        command.arg("--extension").arg(extension);
    }
    let mut child = match command
        .current_dir(&prepared_worktree.path)
        .env("TASKER_API_URL", &request.api_url)
        .env("TASKER_API_TOKEN", &request.api_token)
        .env("TASKER_ACTOR_KIND", "worker_agent")
        .env("TASKER_ACTOR_ID", &actor.id)
        .env("TASKER_ACTOR_DISPLAY_NAME", &actor.display_name)
        .env("TASKER_AGENT_RUN_ID", &claimed.run.id)
        .env(
            "CARGO_TARGET_DIR",
            &prepared_worktree.shared_cargo_target_dir,
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return pi_failure_result(
                claimed,
                &transcript_path,
                format!(
                    "failed to start Pi Launcher process {}: {error}",
                    request.pi_bin
                ),
                None,
                None,
            )
        }
    };
    let mut stdin_guard = child.stdin.take();
    if let Some(stdin) = stdin_guard.as_mut() {
        let rpc_start = format!(
            "{}\n",
            serde_json::json!({ "type": "prompt", "message": prompt })
        );
        if let Err(error) = stdin.write_all(rpc_start.as_bytes()) {
            let _ = child.kill();
            let _ = child.wait();
            return pi_failure_result(
                claimed,
                &transcript_path,
                format!("failed to write Worker Role Prompt to Pi RPC stdin: {error}"),
                None,
                None,
            );
        }
    }

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
    let mut last_heartbeat = Instant::now();
    let heartbeat_interval = Duration::from_secs((request.lease_seconds.max(2) / 2) as u64);
    let mut blocking_ui_request: Option<String> = None;
    let mut agent_ended = false;
    let mut exit_code = None;
    let started_at = Instant::now();
    let mut timed_out = false;

    loop {
        let current_stdout = locked_string(&stdout);
        let event_scan = scan_pi_rpc_stdout(&current_stdout);
        if let Some(reason) = event_scan.blocking_ui_request {
            blocking_ui_request = Some(reason);
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        if event_scan.agent_ended {
            agent_ended = true;
            exit_code = Some(0);
            drop(stdin_guard.take());
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        if let Some(status) = child
            .try_wait()
            .context("failed to poll Pi Launcher process")?
        {
            exit_code = status.code();
            break;
        }
        if let Some(max_run_seconds) = request.max_run_seconds {
            if started_at.elapsed() >= Duration::from_secs(max_run_seconds) {
                timed_out = true;
                let _ = child.kill();
                exit_code = child.wait().ok().and_then(|status| status.code());
                break;
            }
        }
        if last_heartbeat.elapsed() >= heartbeat_interval {
            tasker_db::heartbeat_run(pool, &claimed.run.id, request.lease_seconds, actor).await?;
            last_heartbeat = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    if blocking_ui_request.is_none() {
        if let Some(handle) = stdout_thread {
            let _ = handle.join();
        }
        if let Some(handle) = stderr_thread {
            let _ = handle.join();
        }
    }
    let stdout_text = locked_string(&stdout);
    let stderr_text = locked_string(&stderr);
    let final_scan = scan_pi_rpc_stdout(&stdout_text);
    let blocking_ui_request = blocking_ui_request.or(final_scan.blocking_ui_request);
    agent_ended = agent_ended || final_scan.agent_ended;
    let ui_blocked = blocking_ui_request.is_some();
    let (outcome, failure_reason) = if let Some(reason) = blocking_ui_request {
        ("failed".to_string(), Some(reason))
    } else if timed_out {
        (
            "failed".to_string(),
            Some(format!(
                "Pi Launcher exceeded max run duration of {} seconds before agent_end",
                request.max_run_seconds.unwrap_or_default()
            )),
        )
    } else if agent_ended {
        ("completed".to_string(), None)
    } else if exit_code == Some(0) {
        (
            "failed".to_string(),
            Some("Pi Launcher exited without agent_end event".to_string()),
        )
    } else {
        (
            "failed".to_string(),
            Some(format!(
                "Pi Launcher exited with status {}",
                exit_code.map_or_else(|| "signal".to_string(), |c| c.to_string())
            )),
        )
    };
    write_pi_transcript(
        claimed,
        &transcript_path,
        exit_code,
        &stdout_text,
        &stderr_text,
        ui_blocked,
        timed_out,
    )?;
    Ok(pi_result(
        claimed,
        &transcript_path,
        outcome,
        failure_reason,
        PiResultMetadata {
            exit_code,
            question_detected: ui_blocked,
            timed_out,
            max_run_seconds: request.max_run_seconds,
        },
    ))
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

fn pi_failure_result(
    claimed: &tasker_db::ClaimedRun,
    transcript_path: &Path,
    failure_reason: String,
    stdout: Option<&str>,
    stderr: Option<&str>,
) -> Result<LauncherResult> {
    let stdout = stdout.unwrap_or_default();
    let stderr = stderr.unwrap_or_default();
    write_pi_transcript(claimed, transcript_path, None, stdout, stderr, false, false)?;
    Ok(pi_result(
        claimed,
        transcript_path,
        "failed".to_string(),
        Some(failure_reason),
        PiResultMetadata::default(),
    ))
}

#[derive(Debug, Clone, Copy, Default)]
struct PiResultMetadata {
    exit_code: Option<i32>,
    question_detected: bool,
    timed_out: bool,
    max_run_seconds: Option<u64>,
}

fn pi_result(
    claimed: &tasker_db::ClaimedRun,
    transcript_path: &Path,
    outcome: String,
    failure_reason: Option<String>,
    metadata: PiResultMetadata,
) -> LauncherResult {
    LauncherResult {
        outcome: outcome.clone(),
        failure_reason,
        session_data: tasker_db::UpsertLauncherSessionData {
            launcher_kind: "pi".to_string(),
            session_id: Some(claimed.run.id.clone()),
            model: None,
            provider: None,
            started_at: Some(claimed.run.created_at.clone()),
            finished_at: None,
            final_status: Some(outcome),
            transcript_path: Some(transcript_path.display().to_string()),
            raw_json: Some(
                serde_json::json!({
                    "exit_code": metadata.exit_code,
                    "unattended_question_detected": metadata.question_detected,
                    "timed_out": metadata.timed_out,
                    "max_run_seconds": metadata.max_run_seconds,
                })
                .to_string(),
            ),
        },
    }
}

fn write_pi_transcript(
    claimed: &tasker_db::ClaimedRun,
    transcript_path: &Path,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
    question_detected: bool,
    timed_out: bool,
) -> Result<()> {
    let transcript = serde_json::json!({
        "launcher": "pi",
        "task_identifier": claimed.task.task.identifier,
        "agent_run_id": claimed.run.id,
        "status": exit_code,
        "stdout": stdout,
        "stderr": stderr,
        "unattended_question_detected": question_detected,
        "timed_out": timed_out,
    });
    fs::write(transcript_path, format!("{}\n", transcript)).with_context(|| {
        format!(
            "failed to write Run Transcript {}",
            transcript_path.display()
        )
    })
}

fn build_worker_prompt(
    task: &tasker_db::TaskDetail,
    run: &tasker_db::AgentRun,
    worktree_path: &Path,
    shared_cargo_target_dir: &Path,
    prompt_path: Option<&Path>,
) -> Result<String> {
    let base = if let Some(path) = prompt_path {
        fs::read_to_string(path)
            .with_context(|| format!("failed to read Worker Role Prompt {}", path.display()))?
    } else {
        "You are a Tasker Worker Agent running unattended. Do not ask questions or open interactive UI. Use the Tasker Pi Extension tools to read and update Tasker state, Workpad Notes, requirements, child tasks, and transitions.".to_string()
    };
    Ok(format!(
        "{base}\n\nTask Identifier: {}\nTask Title: {}\nTask State: {}\nAgent Run ID: {}\nLocal Worktree: {}\nShared Cargo Target Directory: {}\nCargo commands inherit CARGO_TARGET_DIR so Rust build artifacts are shared across Worker Agent Local Worktrees for this Managed Source Repository. This Tasker-managed directory is safe to delete when reclaiming space.\nUse Tasker Pi Extension tools for Tasker mutations. When finished, update criteria/validation/workpad and request the appropriate Task State Transition.\n",
        task.task.identifier,
        task.task.title,
        task.task.state,
        run.id,
        worktree_path.display(),
        shared_cargo_target_dir.display()
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PiRpcEventScan {
    agent_ended: bool,
    blocking_ui_request: Option<String>,
}

fn scan_pi_rpc_stdout(output: &str) -> PiRpcEventScan {
    let mut scan = PiRpcEventScan {
        agent_ended: false,
        blocking_ui_request: None,
    };
    for line in output.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let type_name = value
            .get("type")
            .and_then(|kind| kind.as_str())
            .unwrap_or_default();
        if type_name == "agent_end" {
            scan.agent_ended = true;
        } else if type_name == "extension_ui_request" {
            let method_name = value
                .get("method")
                .and_then(|kind| kind.as_str())
                .unwrap_or_default();
            if matches!(method_name, "confirm" | "input" | "select" | "editor") {
                scan.blocking_ui_request = Some(format!(
                    "blocking extension UI request {method_name} in unattended Worker Session"
                ));
                break;
            }
        }
    }
    scan
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedLocalWorktree {
    path: PathBuf,
    shared_cargo_target_dir: PathBuf,
}

async fn prepare_local_worktree(
    pool: &SqlitePool,
    task: &tasker_db::TaskDetail,
    actor: &tasker_db::Actor,
    data_dir: &Path,
) -> Result<PreparedLocalWorktree> {
    let queue = tasker_db::get_task_queue(pool, &task.task.task_queue_key)
        .await?
        .with_context(|| format!("Task Queue {} not found", task.task.task_queue_key))?;
    let branch = queue
        .branch_template
        .replace("{task_identifier}", &task.task.identifier);
    let worktree_path = PathBuf::from(&queue.worktree_root).join(&task.task.identifier);

    setup_local_worktree(
        Path::new(&queue.managed_source_repository),
        &queue.main_branch,
        &branch,
        &worktree_path,
    )?;

    upsert_task_link_with_lock_retry(
        pool,
        &task.task.identifier,
        tasker_db::UpsertTaskLink {
            kind: "local_worktree".to_string(),
            target: worktree_path.display().to_string(),
            label: Some("Local Worktree".to_string()),
            is_primary: true,
        },
        actor,
    )
    .await?;
    upsert_task_link_with_lock_retry(
        pool,
        &task.task.identifier,
        tasker_db::UpsertTaskLink {
            kind: "task_branch".to_string(),
            target: branch,
            label: Some("Task Branch".to_string()),
            is_primary: false,
        },
        actor,
    )
    .await?;
    Ok(PreparedLocalWorktree {
        path: worktree_path,
        shared_cargo_target_dir: shared_cargo_target_dir(
            data_dir,
            Path::new(&queue.managed_source_repository),
        )?,
    })
}

async fn upsert_task_link_with_lock_retry(
    pool: &SqlitePool,
    identifier: &str,
    input: tasker_db::UpsertTaskLink,
    actor: &tasker_db::Actor,
) -> Result<tasker_db::TaskDetail> {
    const MAX_ATTEMPTS: usize = 5;
    let mut delay = Duration::from_millis(50);
    for attempt in 1..=MAX_ATTEMPTS {
        match tasker_db::upsert_task_link(pool, identifier, &input, actor).await {
            Ok(detail) => return Ok(detail),
            Err(error) if attempt < MAX_ATTEMPTS && is_transient_sqlite_lock(&error) => {
                tokio::time::sleep(delay).await;
                delay = delay.saturating_mul(2);
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("Task Link upsert retry loop always returns")
}

fn is_transient_sqlite_lock(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let message = cause.to_string().to_ascii_lowercase();
        message.contains("database is locked")
            || message.contains("database table is locked")
            || message.contains("sqlite busy")
            || message.contains("code: 5")
            || message.contains("code: 6")
    })
}

fn shared_cargo_target_dir(data_dir: &Path, managed_source_repository: &Path) -> Result<PathBuf> {
    let canonical_repo = managed_source_repository.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize Managed Source Repository {}",
            managed_source_repository.display()
        )
    })?;
    let repo_name = canonical_repo
        .file_name()
        .and_then(|name| name.to_str())
        .map(sanitize_path_component)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "repository".to_string());
    let hash = fnv1a64(canonical_repo.to_string_lossy().as_bytes());
    Ok(data_dir
        .join("cargo-target")
        .join(format!("{repo_name}-{hash:016x}")))
}

fn sanitize_path_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn setup_local_worktree(
    managed_source_repository: &Path,
    main_branch: &str,
    task_branch: &str,
    worktree_path: &Path,
) -> Result<()> {
    ensure_git_repository(managed_source_repository)?;
    ensure_clean_repository(managed_source_repository)?;
    validate_task_branch(managed_source_repository, task_branch)?;
    if let Some(parent) = worktree_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    if !git_branch_exists(managed_source_repository, task_branch)? {
        run_git(
            managed_source_repository,
            &["branch", task_branch, main_branch],
            "create Task Branch",
        )?;
    }
    if worktree_path.exists() {
        ensure_existing_worktree(managed_source_repository, worktree_path, task_branch)?;
    } else {
        let worktree = worktree_path.display().to_string();
        run_git(
            managed_source_repository,
            &["worktree", "add", &worktree, task_branch],
            "create Local Worktree",
        )?;
    }
    Ok(())
}

fn ensure_git_repository(path: &Path) -> Result<()> {
    run_git(
        path,
        &["rev-parse", "--show-toplevel"],
        "validate Managed Source Repository",
    )?;
    Ok(())
}

fn ensure_clean_repository(path: &Path) -> Result<()> {
    let output = git_output(
        path,
        &["status", "--porcelain"],
        "check Managed Source Repository cleanliness",
    )?;
    if !output.trim().is_empty() {
        bail!("Managed Source Repository has unexpected uncommitted changes");
    }
    Ok(())
}

fn validate_task_branch(repo: &Path, branch: &str) -> Result<()> {
    if branch.trim().is_empty() || branch.starts_with('-') {
        bail!("Task Branch must be a non-option Git branch name");
    }
    run_git(
        repo,
        &["check-ref-format", "--branch", branch],
        "validate Task Branch",
    )?;
    Ok(())
}

fn ensure_existing_worktree(
    managed_source_repository: &Path,
    worktree_path: &Path,
    task_branch: &str,
) -> Result<()> {
    run_git(
        worktree_path,
        &["rev-parse", "--is-inside-work-tree"],
        "validate existing Local Worktree",
    )?;
    let branch = git_output(
        worktree_path,
        &["branch", "--show-current"],
        "read existing Local Worktree branch",
    )?;
    if branch.trim() != task_branch {
        bail!(
            "existing Local Worktree is on branch {}, expected {}",
            branch.trim(),
            task_branch
        );
    }
    let source_common_dir = git_common_dir(managed_source_repository)?;
    let worktree_common_dir = git_common_dir(worktree_path)?;
    if source_common_dir != worktree_common_dir {
        bail!(
            "existing Local Worktree is not attached to the configured Managed Source Repository"
        );
    }
    Ok(())
}

fn git_common_dir(repo: &Path) -> Result<PathBuf> {
    let common_dir = git_output(
        repo,
        &["rev-parse", "--git-common-dir"],
        "read Git common dir",
    )?;
    let path = PathBuf::from(common_dir.trim());
    let absolute = if path.is_absolute() {
        path
    } else {
        repo.join(path)
    };
    absolute
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", absolute.display()))
}

fn git_branch_exists(repo: &Path, branch: &str) -> Result<bool> {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["show-ref", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .status()
        .context("failed to check Task Branch")?;
    Ok(status.success())
}

fn run_git(repo: &Path, args: &[&str], action: &str) -> Result<()> {
    git_output(repo, args, action).map(|_| ())
}

fn git_output(repo: &Path, args: &[&str], action: &str) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git to {action}"))?;
    if !output.status.success() {
        bail!(
            "failed to {action}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .context("failed to resolve current directory")?
            .join(path))
    }
}

fn fake_workpad_note(task_identifier: &str, run_id: &str, outcome: &str) -> String {
    format!(
        "Fake Agent Launcher processed Task {task_identifier} in Agent Run {run_id}.\nOutcome: {outcome}\n"
    )
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

    #[tokio::test]
    async fn fake_worker_prepares_local_worktree_and_records_task_links() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let worktrees = temp.path().join("worktrees");
        init_git_repo(&repo);
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        tasker_db::create_task_queue(
            &pool,
            &tasker_db::CreateTaskQueue {
                key: "TASK".to_string(),
                name: "Tasker".to_string(),
                managed_source_repository: repo.display().to_string(),
                main_branch: "main".to_string(),
                worktree_root: worktrees.display().to_string(),
                branch_template: "tasker/{task_identifier}".to_string(),
                done_worktree_retention: false,
                queue_concurrency_limit: None,
            },
            &tasker_db::Actor::operator("tester"),
        )
        .await
        .expect("create queue");
        tasker_db::create_task(
            &pool,
            &tasker_db::CreateTask {
                queue_key: "TASK".to_string(),
                title: "Test worktree".to_string(),
                brief: "Prepare worktree".to_string(),
                priority: "normal".to_string(),
                state: "ready".to_string(),
                review_required: false,
                acceptance_criteria: vec!["Worktree exists".to_string()],
                validation_items: vec!["Task Links recorded".to_string()],
                tags: vec![],
                conflict_hints: vec![],
            },
            &tasker_db::Actor::operator("tester"),
        )
        .await
        .expect("create task");

        let outcome = run_worker_once(
            &pool,
            WorkOnceRequest {
                queue: "TASK".to_string(),
                launcher: "fake".to_string(),
                actor: "worker".to_string(),
                fake_outcome: "completed".to_string(),
                lease_seconds: 90,
                retry_hold_seconds: None,
                max_run_seconds: None,
                data_dir: temp.path().join("data"),
                api_url: "http://127.0.0.1:4317".to_string(),
                api_token: "token".to_string(),
                pi_bin: "pi".to_string(),
                pi_extension: None,
                worker_prompt: None,
            },
        )
        .await
        .expect("run worker");

        assert!(matches!(outcome, WorkOnceOutcome::Finished { .. }));
        assert!(worktrees.join("TASK-1").is_dir());
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("load task")
            .expect("task exists");
        assert!(detail
            .task_links
            .iter()
            .any(|link| link.kind == "local_worktree" && link.is_primary));
        assert!(detail
            .task_links
            .iter()
            .any(|link| link.kind == "task_branch" && link.target == "tasker/TASK-1"));
        let runs_dir = temp.path().join("data/runs");
        assert!(runs_dir.is_dir());
        let run_id = match outcome {
            WorkOnceOutcome::Finished { run_id, .. } => run_id,
            WorkOnceOutcome::NoEligibleTask { .. } => panic!("expected finished run"),
        };
        let run_detail = tasker_db::get_agent_run_detail(&pool, &run_id)
            .await
            .expect("load run detail")
            .expect("run detail");
        let transcript_path = run_detail
            .launcher_session_data
            .expect("Launcher Session Data")
            .transcript_path
            .expect("Run Transcript path");
        assert!(Path::new(&transcript_path).is_absolute());
        assert!(Path::new(&transcript_path).starts_with(&runs_dir));
    }

    #[tokio::test]
    async fn worker_finishes_run_failed_when_local_worktree_setup_fails_after_claim() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let worktrees = temp.path().join("worktrees");
        init_git_repo(&repo);
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        seed_ready_task(&pool, &repo, &worktrees).await;
        fs::write(repo.join("dirty.txt"), "dirty").expect("dirty repo");

        let outcome = run_worker_once(
            &pool,
            WorkOnceRequest {
                queue: "TASK".to_string(),
                launcher: "fake".to_string(),
                actor: "worker".to_string(),
                fake_outcome: "completed".to_string(),
                lease_seconds: 90,
                retry_hold_seconds: Some(7),
                max_run_seconds: None,
                data_dir: temp.path().join("data"),
                api_url: "http://127.0.0.1:4317".to_string(),
                api_token: "token".to_string(),
                pi_bin: "pi".to_string(),
                pi_extension: None,
                worker_prompt: None,
            },
        )
        .await
        .expect("run worker");

        let WorkOnceOutcome::Finished {
            run_id, outcome, ..
        } = outcome
        else {
            panic!("finished")
        };
        assert_eq!(outcome, "failed");
        let run = tasker_db::get_agent_run(&pool, &run_id)
            .await
            .expect("load run")
            .expect("run");
        assert!(run.finished_at.is_some());
        let failure_reason = run.failure_reason.expect("failure reason");
        assert!(failure_reason.contains("Local Worktree setup failed after claim"));
        assert!(failure_reason.contains("unexpected uncommitted changes"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pi_worker_records_transcript_and_completes_successful_process() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let worktrees = temp.path().join("worktrees");
        init_git_repo(&repo);
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        seed_ready_task(&pool, &repo, &worktrees).await;
        let pi_bin = temp.path().join("fake-pi");
        let cargo_target_log = temp.path().join("cargo-target-dir.txt");
        write_executable(
            &pi_bin,
            &format!(
                "#!/bin/sh\ntest \"$1 $2 $3 $4\" = \"--mode rpc --extension extensions/tasker-pi/src/index.ts\" || exit 7\ntest -n \"$TASKER_AGENT_RUN_ID\" || exit 8\ntest -n \"$CARGO_TARGET_DIR\" || exit 9\nprintf '%s' \"$CARGO_TARGET_DIR\" > {}\nread line\necho '{{\"type\":\"agent_end\"}}'\n",
                cargo_target_log.display()
            ),
        );

        let outcome = run_worker_once(
            &pool,
            WorkOnceRequest {
                queue: "TASK".to_string(),
                launcher: "pi".to_string(),
                actor: "worker".to_string(),
                fake_outcome: "completed".to_string(),
                lease_seconds: 90,
                retry_hold_seconds: None,
                max_run_seconds: None,
                data_dir: temp.path().join("data"),
                api_url: "http://127.0.0.1:4317".to_string(),
                api_token: "token".to_string(),
                pi_bin: pi_bin.display().to_string(),
                pi_extension: Some(PathBuf::from("extensions/tasker-pi/src/index.ts")),
                worker_prompt: None,
            },
        )
        .await
        .expect("run worker");

        let WorkOnceOutcome::Finished {
            run_id, outcome, ..
        } = outcome
        else {
            panic!("finished")
        };
        assert_eq!(outcome, "completed");
        let detail = tasker_db::get_agent_run_detail(&pool, &run_id)
            .await
            .expect("load run")
            .expect("run detail");
        let session = detail.launcher_session_data.expect("session");
        assert_eq!(session.launcher_kind, "pi");
        assert!(Path::new(&session.transcript_path.expect("transcript")).is_file());
        let expected_cargo_target_dir =
            shared_cargo_target_dir(&temp.path().join("data"), &repo).expect("shared target dir");
        assert_eq!(
            fs::read_to_string(cargo_target_log).expect("cargo target log"),
            expected_cargo_target_dir.display().to_string()
        );
        assert!(expected_cargo_target_dir.is_dir());
        assert!(!worktrees.join("TASK-1/target").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pi_worker_fails_zero_exit_without_agent_end_event() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let worktrees = temp.path().join("worktrees");
        init_git_repo(&repo);
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        seed_ready_task(&pool, &repo, &worktrees).await;
        let pi_bin = temp.path().join("fake-pi-no-agent-end");
        write_executable(
            &pi_bin,
            "#!/bin/sh\nread line\necho '{\"type\":\"turn_end\"}'\n",
        );

        let outcome = run_worker_once(
            &pool,
            WorkOnceRequest {
                queue: "TASK".to_string(),
                launcher: "pi".to_string(),
                actor: "worker".to_string(),
                fake_outcome: "completed".to_string(),
                lease_seconds: 90,
                retry_hold_seconds: None,
                max_run_seconds: None,
                data_dir: temp.path().join("data"),
                api_url: "http://127.0.0.1:4317".to_string(),
                api_token: "token".to_string(),
                pi_bin: pi_bin.display().to_string(),
                pi_extension: None,
                worker_prompt: None,
            },
        )
        .await
        .expect("run worker");

        let WorkOnceOutcome::Finished {
            run_id, outcome, ..
        } = outcome
        else {
            panic!("finished")
        };
        assert_eq!(outcome, "failed");
        let run = tasker_db::get_agent_run(&pool, &run_id)
            .await
            .expect("load run")
            .expect("run");
        assert_eq!(
            run.failure_reason.as_deref(),
            Some("Pi Launcher exited without agent_end event")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pi_worker_fails_when_max_run_duration_elapses() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let worktrees = temp.path().join("worktrees");
        init_git_repo(&repo);
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        seed_ready_task(&pool, &repo, &worktrees).await;
        let pi_bin = temp.path().join("fake-pi-timeout");
        write_executable(
            &pi_bin,
            "#!/bin/sh\nread line\necho '{\"type\":\"turn_start\"}'\nsleep 5\necho '{\"type\":\"agent_end\"}'\n",
        );

        let outcome = run_worker_once(
            &pool,
            WorkOnceRequest {
                queue: "TASK".to_string(),
                launcher: "pi".to_string(),
                actor: "worker".to_string(),
                fake_outcome: "completed".to_string(),
                lease_seconds: 90,
                retry_hold_seconds: Some(7),
                max_run_seconds: Some(1),
                data_dir: temp.path().join("data"),
                api_url: "http://127.0.0.1:4317".to_string(),
                api_token: "token".to_string(),
                pi_bin: pi_bin.display().to_string(),
                pi_extension: None,
                worker_prompt: None,
            },
        )
        .await
        .expect("run worker");

        let WorkOnceOutcome::Finished {
            run_id, outcome, ..
        } = outcome
        else {
            panic!("finished")
        };
        assert_eq!(outcome, "failed");
        let detail = tasker_db::get_agent_run_detail(&pool, &run_id)
            .await
            .expect("load run")
            .expect("run detail");
        assert_eq!(
            detail.run.failure_reason.as_deref(),
            Some("Pi Launcher exceeded max run duration of 1 seconds before agent_end")
        );
        let session = detail.launcher_session_data.expect("session");
        assert_eq!(session.final_status.as_deref(), Some("failed"));
        let raw: serde_json::Value =
            serde_json::from_str(&session.raw_json.expect("raw json")).expect("raw session json");
        assert_eq!(
            raw.get("timed_out").and_then(|value| value.as_bool()),
            Some(true)
        );
        assert_eq!(
            raw.get("max_run_seconds").and_then(|value| value.as_u64()),
            Some(1)
        );
        let transcript_path = session.transcript_path.expect("transcript");
        let transcript_text = fs::read_to_string(&transcript_path).expect("transcript text");
        let transcript: serde_json::Value =
            serde_json::from_str(transcript_text.trim()).expect("transcript json");
        assert_eq!(
            transcript
                .get("timed_out")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pi_worker_fails_blocking_extension_ui_request() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let worktrees = temp.path().join("worktrees");
        init_git_repo(&repo);
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        seed_ready_task(&pool, &repo, &worktrees).await;
        let pi_bin = temp.path().join("fake-pi-question");
        write_executable(
            &pi_bin,
            "#!/bin/sh\nread line\necho '{\"type\":\"extension_ui_request\",\"id\":\"ui-1\",\"method\":\"input\"}'\nsleep 5\n",
        );

        let outcome = run_worker_once(
            &pool,
            WorkOnceRequest {
                queue: "TASK".to_string(),
                launcher: "pi".to_string(),
                actor: "worker".to_string(),
                fake_outcome: "completed".to_string(),
                lease_seconds: 90,
                retry_hold_seconds: None,
                max_run_seconds: None,
                data_dir: temp.path().join("data"),
                api_url: "http://127.0.0.1:4317".to_string(),
                api_token: "token".to_string(),
                pi_bin: pi_bin.display().to_string(),
                pi_extension: None,
                worker_prompt: None,
            },
        )
        .await
        .expect("run worker");

        let WorkOnceOutcome::Finished {
            run_id, outcome, ..
        } = outcome
        else {
            panic!("finished")
        };
        assert_eq!(outcome, "failed");
        let run = tasker_db::get_agent_run(&pool, &run_id)
            .await
            .expect("load run")
            .expect("run");
        assert_eq!(
            run.failure_reason.as_deref(),
            Some("blocking extension UI request input in unattended Worker Session")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pi_worker_ignores_fire_and_forget_extension_ui_request() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let worktrees = temp.path().join("worktrees");
        init_git_repo(&repo);
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        seed_ready_task(&pool, &repo, &worktrees).await;
        let pi_bin = temp.path().join("fake-pi-notify");
        write_executable(
            &pi_bin,
            "#!/bin/sh\nread line\necho '{\"type\":\"extension_ui_request\",\"id\":\"ui-1\",\"method\":\"notify\",\"message\":\"ok\"}'\necho '{\"event\":\"question\"}'\necho '{\"type\":\"agent_end\"}'\n",
        );

        let outcome = run_worker_once(
            &pool,
            WorkOnceRequest {
                queue: "TASK".to_string(),
                launcher: "pi".to_string(),
                actor: "worker".to_string(),
                fake_outcome: "completed".to_string(),
                lease_seconds: 90,
                retry_hold_seconds: None,
                max_run_seconds: None,
                data_dir: temp.path().join("data"),
                api_url: "http://127.0.0.1:4317".to_string(),
                api_token: "secret-token-that-must-not-be-in-raw-json".to_string(),
                pi_bin: pi_bin.display().to_string(),
                pi_extension: None,
                worker_prompt: None,
            },
        )
        .await
        .expect("run worker");

        let WorkOnceOutcome::Finished {
            run_id, outcome, ..
        } = outcome
        else {
            panic!("finished")
        };
        assert_eq!(outcome, "completed");
        let detail = tasker_db::get_agent_run_detail(&pool, &run_id)
            .await
            .expect("load run")
            .expect("run detail");
        let raw_json = detail
            .launcher_session_data
            .expect("session")
            .raw_json
            .expect("raw json");
        assert!(!raw_json.contains("secret-token-that-must-not-be-in-raw-json"));
    }

    #[test]
    fn pi_rpc_stdout_scan_uses_structured_jsonl_events() {
        let scan = scan_pi_rpc_stdout(
            "not json question\n{\"type\":\"extension_ui_request\",\"method\":\"notify\"}\n{\"event\":\"question\"}\n{\"type\":\"agent_end\"}\n",
        );
        assert!(scan.agent_ended);
        assert_eq!(scan.blocking_ui_request, None);

        let scan = scan_pi_rpc_stdout(
            "{\"type\":\"extension_ui_request\",\"method\":\"confirm\"}\n{\"type\":\"agent_end\"}\n",
        );
        assert_eq!(
            scan.blocking_ui_request.as_deref(),
            Some("blocking extension UI request confirm in unattended Worker Session")
        );
    }

    async fn seed_ready_task(pool: &SqlitePool, repo: &Path, worktrees: &Path) {
        tasker_db::create_task_queue(
            pool,
            &tasker_db::CreateTaskQueue {
                key: "TASK".to_string(),
                name: "Tasker".to_string(),
                managed_source_repository: repo.display().to_string(),
                main_branch: "main".to_string(),
                worktree_root: worktrees.display().to_string(),
                branch_template: "tasker/{task_identifier}".to_string(),
                done_worktree_retention: false,
                queue_concurrency_limit: None,
            },
            &tasker_db::Actor::operator("tester"),
        )
        .await
        .expect("create queue");
        tasker_db::create_task(
            pool,
            &tasker_db::CreateTask {
                queue_key: "TASK".to_string(),
                title: "Test work".to_string(),
                brief: "Do work".to_string(),
                priority: "normal".to_string(),
                state: "ready".to_string(),
                review_required: false,
                acceptance_criteria: vec!["Works".to_string()],
                validation_items: vec!["Validated".to_string()],
                tags: vec![],
                conflict_hints: vec![],
            },
            &tasker_db::Actor::operator("tester"),
        )
        .await
        .expect("create task");
    }

    #[test]
    fn local_worktree_setup_rejects_dirty_managed_source_repository() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        init_git_repo(&repo);
        fs::write(repo.join("dirty.txt"), "dirty").expect("dirty file");

        let error = setup_local_worktree(
            &repo,
            "main",
            "tasker/TASK-1",
            &temp.path().join("worktrees/TASK-1"),
        )
        .expect_err("dirty repo fails");

        assert!(error.to_string().contains("unexpected uncommitted changes"));
    }

    #[test]
    fn local_worktree_setup_rejects_existing_plain_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let worktree = temp.path().join("worktrees/TASK-1");
        init_git_repo(&repo);
        fs::create_dir_all(&worktree).expect("plain dir");

        let error = setup_local_worktree(&repo, "main", "tasker/TASK-1", &worktree)
            .expect_err("plain dir fails");

        assert!(error
            .to_string()
            .contains("validate existing Local Worktree"));
    }

    #[test]
    fn local_worktree_setup_rejects_unrelated_existing_worktree_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let unrelated = temp.path().join("worktrees/TASK-1");
        init_git_repo(&repo);
        init_git_repo(&unrelated);
        git(&unrelated, &["checkout", "-b", "tasker/TASK-1"]);

        let error = setup_local_worktree(&repo, "main", "tasker/TASK-1", &unrelated)
            .expect_err("unrelated repo fails");

        assert!(error
            .to_string()
            .contains("not attached to the configured Managed Source Repository"));
    }

    #[test]
    fn local_worktree_setup_rejects_option_like_branch_names() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        init_git_repo(&repo);

        let error = setup_local_worktree(
            &repo,
            "main",
            "-bad-branch",
            &temp.path().join("worktrees/TASK-1"),
        )
        .expect_err("option-like branch fails");

        assert!(error.to_string().contains("non-option Git branch"));
    }

    #[test]
    fn shared_cargo_target_dir_is_outside_worktrees_and_stable_for_repo() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        init_git_repo(&repo);
        let data_dir = temp.path().join("data");

        let first = shared_cargo_target_dir(&data_dir, &repo).expect("first target dir");
        let second = shared_cargo_target_dir(&data_dir, &repo).expect("second target dir");

        assert_eq!(first, second);
        assert!(first.starts_with(data_dir.join("cargo-target")));
        assert!(!first.starts_with(temp.path().join("worktrees/TASK-1")));
        assert!(first
            .file_name()
            .and_then(|name| name.to_str())
            .expect("file name")
            .starts_with("repo-"));
    }

    #[test]
    fn sqlite_lock_retry_detection_recognizes_lock_errors() {
        let error = anyhow::anyhow!("failed to upsert Task Link: database is locked");
        assert!(is_transient_sqlite_lock(&error));

        let error = anyhow::anyhow!("failed to upsert Task Link: malformed input");
        assert!(!is_transient_sqlite_lock(&error));
    }

    #[test]
    fn fake_workpad_note_is_deterministic() {
        assert_eq!(
            fake_workpad_note("TASK-1", "run-1", "completed"),
            "Fake Agent Launcher processed Task TASK-1 in Agent Run run-1.\nOutcome: completed\n"
        );
    }

    fn init_git_repo(repo: &Path) {
        fs::create_dir_all(repo).expect("repo dir");
        git(repo, &["init", "-b", "main"]);
        git(repo, &["config", "user.email", "tasker@example.test"]);
        git(repo, &["config", "user.name", "Tasker Test"]);
        fs::write(repo.join("README.md"), "test repo\n").expect("readme");
        git(repo, &["add", "README.md"]);
        git(repo, &["commit", "-m", "initial"]);
    }

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
