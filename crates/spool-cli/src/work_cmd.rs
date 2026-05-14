use super::*;

pub(crate) async fn work(
    paths: &SpoolPaths,
    db_path_overridden: bool,
    options: WorkOptions,
) -> Result<()> {
    if !options.once {
        anyhow::bail!("spool work currently requires --once");
    }
    let pool = open_pool(paths, db_path_overridden).await?;
    let mut config = SpoolConfig::load_or_default(paths)?;
    if db_path_overridden {
        config.database.path = paths.db_path.clone();
    }
    let api_token = spool_db::ensure_local_api_token(&pool).await?;
    let api_url = options
        .api_url
        .unwrap_or_else(|| format!("http://{}", config.service.bind_addr));
    let outcome = spool_runner::worker::run_worker_once(
        &pool,
        spool_runner::worker::WorkOnceRequest {
            queue: options.queue,
            launcher: options.launcher,
            actor: options.actor,
            fake_outcome: options.fake_outcome,
            lease_seconds: options.lease_seconds,
            retry_hold_seconds: options.retry_hold_seconds,
            max_run_seconds: options.max_run_seconds,
            data_dir: paths.data_dir.clone(),
            api_url,
            api_token,
            pi_bin: options.pi_bin,
            pi_extension: options.pi_extension,
            worker_prompt: options.worker_prompt,
        },
    )
    .await?;

    match outcome {
        spool_runner::worker::WorkOnceOutcome::NoEligibleTask { queue } => {
            println!("no eligible Tasks found for Task Queue {queue}");
        }
        spool_runner::worker::WorkOnceOutcome::PreflightFailed { queue, message } => {
            println!("Task Queue {queue} failed Worker Loop preflight; no Task was claimed and no Agent Run was created");
            println!("{message}");
        }
        spool_runner::worker::WorkOnceOutcome::RepoOperationLocked { queue, message } => {
            println!("Task Queue {queue} is blocked by a Managed Source Repository operation lock; no Task was claimed and no Agent Run was created");
            println!("{message}");
        }
        spool_runner::worker::WorkOnceOutcome::Finished {
            task_identifier,
            run_id,
            outcome,
        } => {
            println!("claimed Task {task_identifier} with Agent Run {run_id}");
            println!("finished Agent Run {run_id} with outcome {outcome}");
        }
    }
    Ok(())
}
