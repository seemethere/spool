use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoOperationLockFile {
    pub tasker_repo_operation_lock: bool,
    pub queue: String,
    pub pid: u32,
    pub operation: String,
    pub task_identifier: Option<String>,
    pub manual: bool,
    pub started_at_unix_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveRepoOperationLock {
    pub path: PathBuf,
    pub lock: RepoOperationLockFile,
}

#[derive(Debug)]
pub struct RepoOperationLockGuard {
    path: PathBuf,
    contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquireRepoOperationLock {
    pub data_dir: PathBuf,
    pub queue: String,
    pub operation: String,
    pub task_identifier: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowRepoOperationLock {
    pub data_dir: PathBuf,
    pub queue: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseRepoOperationLock {
    pub data_dir: PathBuf,
    pub queue: String,
}

impl Drop for RepoOperationLockGuard {
    fn drop(&mut self) {
        let Ok(existing) = fs::read_to_string(&self.path) else {
            return;
        };
        if existing == self.contents {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub fn lock_path(data_dir: &Path, queue: &str) -> PathBuf {
    data_dir
        .join("repo-operation-locks")
        .join(format!("{}.lock", queue_slug(queue)))
}

pub fn acquire_repo_operation_lock(
    request: AcquireRepoOperationLock,
) -> Result<ActiveRepoOperationLock> {
    acquire_manual(
        &request.data_dir,
        &request.queue,
        &request.operation,
        request.task_identifier.as_deref(),
    )
}

pub fn show_repo_operation_lock(
    request: ShowRepoOperationLock,
) -> Result<Option<ActiveRepoOperationLock>> {
    active_lock(&request.data_dir, &request.queue)
}

pub fn release_repo_operation_lock(
    request: ReleaseRepoOperationLock,
) -> Result<Option<ActiveRepoOperationLock>> {
    release_manual(&request.data_dir, &request.queue)
}

pub fn acquire_guard(
    data_dir: &Path,
    queue: &str,
    operation: &str,
    task_identifier: Option<&str>,
) -> Result<RepoOperationLockGuard> {
    let path = lock_path(data_dir, queue);
    create_lock(&path, queue, operation, task_identifier, false)
        .map(|contents| RepoOperationLockGuard { path, contents })
}

pub fn acquire_manual(
    data_dir: &Path,
    queue: &str,
    operation: &str,
    task_identifier: Option<&str>,
) -> Result<ActiveRepoOperationLock> {
    let path = lock_path(data_dir, queue);
    let contents = create_lock(&path, queue, operation, task_identifier, true)?;
    let lock =
        serde_json::from_str(&contents).context("failed to read created repo operation lock")?;
    Ok(ActiveRepoOperationLock { path, lock })
}

pub fn release_manual(data_dir: &Path, queue: &str) -> Result<Option<ActiveRepoOperationLock>> {
    let Some(active) = active_lock(data_dir, queue)? else {
        return Ok(None);
    };
    fs::remove_file(&active.path).with_context(|| {
        format!(
            "failed to remove Managed Source Repository operation lock {}",
            active.path.display()
        )
    })?;
    Ok(Some(active))
}

pub fn active_lock(data_dir: &Path, queue: &str) -> Result<Option<ActiveRepoOperationLock>> {
    let path = lock_path(data_dir, queue);
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).with_context(|| {
        format!(
            "failed to read Managed Source Repository operation lock {}",
            path.display()
        )
    })?;
    let lock = serde_json::from_str::<RepoOperationLockFile>(&text).with_context(|| {
        format!(
            "failed to parse Managed Source Repository operation lock {}; explicit operator repair required",
            path.display()
        )
    })?;
    if lock.queue == queue && !lock.manual && !is_process_alive(lock.pid) {
        fs::remove_file(&path).with_context(|| {
            format!(
                "failed to remove stale Managed Source Repository operation lock {}",
                path.display()
            )
        })?;
        eprintln!(
            "removed stale Managed Source Repository operation lock for Task Queue {queue} at {} from exited pid {} operation={}",
            path.display(),
            lock.pid,
            lock.operation
        );
        return Ok(None);
    }
    Ok(Some(ActiveRepoOperationLock { path, lock }))
}

pub fn blocked_message(active: &ActiveRepoOperationLock) -> String {
    format!(
        "Managed Source Repository operation lock is held for Task Queue {} by pid {} operation={}{} at {}. Workers must not claim Tasks while the Managed Source Repository is being mutated. Wait for the operation to finish or use `tasker merge lock release --queue {}` only after explicit operator verification.",
        active.lock.queue,
        active.lock.pid,
        active.lock.operation,
        active
            .lock
            .task_identifier
            .as_deref()
            .map(|task| format!(" task={task}"))
            .unwrap_or_default(),
        active.path.display(),
        active.lock.queue
    )
}

fn create_lock(
    path: &Path,
    queue: &str,
    operation: &str,
    task_identifier: Option<&str>,
    manual: bool,
) -> Result<String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    loop {
        let contents = lock_contents(queue, operation, task_identifier, manual)?;
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut file) => {
                file.write_all(contents.as_bytes()).with_context(|| {
                    format!(
                        "failed to write Managed Source Repository operation lock {}",
                        path.display()
                    )
                })?;
                return Ok(contents);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let data_dir = path
                    .parent()
                    .and_then(Path::parent)
                    .context("repo operation lock path is missing data dir parent")?;
                if let Some(active) = active_lock(data_dir, queue)? {
                    anyhow::bail!(blocked_message(&active));
                }
                continue;
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to create Managed Source Repository operation lock {}",
                        path.display()
                    )
                });
            }
        }
    }
}

fn lock_contents(
    queue: &str,
    operation: &str,
    task_identifier: Option<&str>,
    manual: bool,
) -> Result<String> {
    let started_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    serde_json::to_string_pretty(&RepoOperationLockFile {
        tasker_repo_operation_lock: true,
        queue: queue.to_string(),
        pid: std::process::id(),
        operation: operation.to_string(),
        task_identifier: task_identifier.map(ToString::to_string),
        manual,
        started_at_unix_ms,
    })
    .context("failed to serialize repo operation lock")
}

fn queue_slug(queue: &str) -> String {
    let mut slug = String::new();
    for byte in queue.as_bytes() {
        let ch = *byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
            slug.push(ch);
        } else {
            slug.push_str(&format!("%{byte:02X}"));
        }
    }
    slug
}

fn is_process_alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_operation_lock_acquire_release_and_refusal() {
        let temp = tempfile::tempdir().expect("tempdir");
        let guard =
            acquire_guard(temp.path(), "TASK", "integration", Some("TASK-1")).expect("acquire");
        let active = active_lock(temp.path(), "TASK")
            .expect("active")
            .expect("lock");
        assert_eq!(active.lock.operation, "integration");
        assert_eq!(active.lock.task_identifier.as_deref(), Some("TASK-1"));
        let error = acquire_guard(temp.path(), "TASK", "worker_claim", None)
            .expect_err("same queue refused");
        assert!(error.to_string().contains("operation lock is held"));
        drop(guard);
        assert!(active_lock(temp.path(), "TASK").expect("active").is_none());
    }

    #[test]
    fn repo_operation_lock_keeps_manual_lock_until_explicit_release() {
        let temp = tempfile::tempdir().expect("tempdir");
        let active = acquire_manual(temp.path(), "TASK", "manual_integration", None)
            .expect("manual acquire");
        assert!(active.lock.manual);
        let released = release_manual(temp.path(), "TASK")
            .expect("release")
            .expect("released");
        assert_eq!(released.path, active.path);
        assert!(active_lock(temp.path(), "TASK").expect("active").is_none());
    }

    #[test]
    fn repo_operation_lock_recovers_stale_automatic_lock() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = lock_path(temp.path(), "TASK");
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        let stale = RepoOperationLockFile {
            tasker_repo_operation_lock: true,
            queue: "TASK".to_string(),
            pid: u32::MAX,
            operation: "integration".to_string(),
            task_identifier: Some("TASK-1".to_string()),
            manual: false,
            started_at_unix_ms: 1,
        };
        fs::write(&path, serde_json::to_string_pretty(&stale).expect("json")).expect("write");
        assert!(active_lock(temp.path(), "TASK").expect("active").is_none());
        let guard = acquire_guard(temp.path(), "TASK", "worker_claim", None).expect("acquire");
        drop(guard);
    }
}
