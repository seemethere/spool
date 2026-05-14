use super::*;

pub(crate) async fn delegate(
    paths: &SpoolPaths,
    db_path_overridden: bool,
    options: DelegateOptions,
) -> Result<()> {
    let pool = open_pool(paths, db_path_overridden).await?;
    let mut config = SpoolConfig::load_or_default(paths)?;
    if db_path_overridden {
        config.database.path = paths.db_path.clone();
    }
    if options.queue.is_some() && options.refine.is_some() {
        anyhow::bail!("spool delegate accepts either --queue for creation or --refine for Backlog Task refinement, not both");
    }
    let initial_intent = resolve_initial_intent(options.initial_intent, options.intent_file)?;

    let api_token = spool_db::ensure_local_api_token(&pool).await?;
    let api_url = options
        .api_url
        .unwrap_or_else(|| format!("http://{}", config.service.bind_addr));

    let (queue_key, refine_task_identifier, managed_source_repository, existing_task_context) =
        if let Some(identifier) = options.refine {
            let bundle = spool_db::get_task_context_bundle(&pool, &identifier)
                .await?
                .with_context(|| format!("Task {identifier} not found"))?;
            if bundle.task.task.state != "backlog" {
                anyhow::bail!(
                    "spool delegate --refine requires Task {} to be in Backlog; current Task State is {}",
                    identifier,
                    bundle.task.task.state
                );
            }
            let context = render_refinement_context(&bundle)?;
            (
                Some(bundle.queue.key),
                Some(identifier),
                PathBuf::from(bundle.queue.managed_source_repository),
                Some(context),
            )
        } else {
            let queue = resolve_delegate_queue(&pool, options.queue.as_deref()).await?;
            (
                Some(queue.key),
                None,
                PathBuf::from(queue.managed_source_repository),
                None,
            )
        };

    let pi_extension =
        resolve_delegate_pi_extension(options.pi_extension, &managed_source_repository)?;

    let outcome = spool_runner::delegate::run_delegation_session(
        spool_runner::delegate::DelegationSessionRequest {
            queue_key,
            refine_task_identifier,
            existing_task_context,
            initial_intent,
            managed_source_repository,
            api_url,
            api_token,
            actor: options.actor,
            pi_bin: options.pi_bin,
            pi_extension,
        },
    )
    .await?;

    match outcome.refine_task_identifier {
        Some(identifier) => println!(
            "finished Delegation Session for refining Task {} with Pi-backed Delegating Agent",
            identifier
        ),
        None => println!(
            "finished Delegation Session for Task Queue {} with Pi-backed Delegating Agent",
            outcome.queue_key.as_deref().unwrap_or("unknown")
        ),
    }
    Ok(())
}

fn resolve_initial_intent(
    positional_intent: Option<String>,
    intent_file: Option<PathBuf>,
) -> Result<Option<String>> {
    match (positional_intent, intent_file) {
        (Some(_), Some(path)) => anyhow::bail!(
            "spool delegate accepts either positional INTENT or --intent-file {}, not both",
            path.display()
        ),
        (Some(intent), None) => {
            let trimmed = intent.trim();
            if trimmed.is_empty() {
                anyhow::bail!("spool delegate positional INTENT must not be empty");
            }
            Ok(Some(trimmed.to_string()))
        }
        (None, Some(path)) => {
            let intent = fs::read_to_string(&path)
                .with_context(|| format!("failed to read intent file {}", path.display()))?;
            let trimmed = intent.trim();
            if trimmed.is_empty() {
                anyhow::bail!("spool delegate --intent-file {} is empty", path.display());
            }
            Ok(Some(trimmed.to_string()))
        }
        (None, None) => Ok(None),
    }
}

fn resolve_delegate_pi_extension(
    explicit: Option<PathBuf>,
    managed_source_repository: &Path,
) -> Result<Option<PathBuf>> {
    if let Some(path) = explicit {
        return Ok(Some(path));
    }

    let local_extension = managed_source_repository.join("extensions/tasker-pi/src/index.ts");
    if local_extension.is_file() {
        Ok(Some(PathBuf::from("extensions/tasker-pi/src/index.ts")))
    } else {
        anyhow::bail!(
            "spool delegate requires a Spool Pi Extension. No --pi-extension was provided and the repo-local extension was not found at {}. Run from the Managed Source Repository, restore extensions/tasker-pi/src/index.ts, or pass --pi-extension explicitly.",
            local_extension.display()
        )
    }
}

async fn resolve_delegate_queue(
    pool: &sqlx::SqlitePool,
    queue_key: Option<&str>,
) -> Result<spool_db::TaskQueue> {
    if let Some(queue_key) = queue_key {
        return spool_db::get_task_queue(pool, queue_key)
            .await?
            .with_context(|| format!("Task Queue {queue_key} not found"));
    }

    let queues = spool_db::list_task_queues(pool).await?;
    match queues.as_slice() {
        [queue] => Ok(queue.clone()),
        [] => anyhow::bail!("spool delegate requires --queue because no Task Queues exist"),
        _ => anyhow::bail!("spool delegate requires --queue when more than one Task Queue exists"),
    }
}

fn render_refinement_context(bundle: &spool_db::TaskContextBundle) -> Result<String> {
    serde_json::to_string_pretty(&serde_json::json!({
        "task": bundle.task.task,
        "acceptance_criteria": bundle.task.acceptance_criteria,
        "validation_items": bundle.task.validation_items,
        "tags": bundle.task.tags,
        "conflict_hints": bundle.task.conflict_hints,
        "blocking_tasks": bundle.task.blocking_tasks,
        "workpad_note": bundle.task.workpad_note,
        "queue": {
            "key": bundle.queue.key,
            "name": bundle.queue.name,
        },
        "deterministic_refinement_tool": "tasker_refine_backlog_task",
        "interactive_session_note": "Question UI is allowed in this Delegation Session only; Unattended Worker Session question handling is unchanged.",
    }))
    .context("failed to render Backlog Task refinement context")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolve_delegate_queue_selects_single_queue_or_requires_explicit_choice() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("spool.db");
        let pool = spool_db::connect(&db_path).await.expect("connect");
        spool_db::run_migrations(&pool).await.expect("migrate");

        let none = resolve_delegate_queue(&pool, None)
            .await
            .expect_err("no queues requires explicit queue");
        assert!(none.to_string().contains("no Task Queues"));

        create_queue(&pool, "TASK", temp.path()).await;
        let selected = resolve_delegate_queue(&pool, None)
            .await
            .expect("single queue selected");
        assert_eq!(selected.key, "TASK");

        create_queue(&pool, "ALT", temp.path()).await;
        let multiple = resolve_delegate_queue(&pool, None)
            .await
            .expect_err("multiple queues require explicit queue");
        assert!(multiple.to_string().contains("more than one Task Queue"));

        let explicit = resolve_delegate_queue(&pool, Some("ALT"))
            .await
            .expect("explicit queue");
        assert_eq!(explicit.key, "ALT");
    }

    #[test]
    fn delegate_pi_extension_defaults_to_repo_local_path_or_reports_missing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let missing = resolve_delegate_pi_extension(None, temp.path())
            .expect_err("missing repo-local extension is actionable");
        assert!(missing
            .to_string()
            .contains("repo-local extension was not found"));

        let extension = temp.path().join("extensions/tasker-pi/src/index.ts");
        fs::create_dir_all(extension.parent().expect("extension parent")).expect("mkdir");
        fs::write(&extension, "// fake extension").expect("write extension");

        let resolved = resolve_delegate_pi_extension(None, temp.path()).expect("default extension");
        assert_eq!(
            resolved,
            Some(PathBuf::from("extensions/tasker-pi/src/index.ts"))
        );
    }

    async fn create_queue(pool: &sqlx::SqlitePool, key: &str, root: &Path) {
        spool_db::create_task_queue(
            pool,
            &spool_db::CreateTaskQueue {
                key: key.to_string(),
                name: key.to_string(),
                managed_source_repository: root.display().to_string(),
                main_branch: "main".to_string(),
                worktree_root: root.join("worktrees").display().to_string(),
                branch_template: "spool/{task_identifier}".to_string(),
                done_worktree_retention: false,
                queue_concurrency_limit: None,
            },
            &spool_db::Actor::operator("operator"),
        )
        .await
        .expect("queue");
    }
}
