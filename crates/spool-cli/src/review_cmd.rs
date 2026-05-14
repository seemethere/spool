use super::*;

pub(crate) async fn review(
    paths: &SpoolPaths,
    db_path_overridden: bool,
    options: ReviewOptions,
) -> Result<()> {
    let pool = open_pool(paths, db_path_overridden).await?;
    let mut config = SpoolConfig::load_or_default(paths)?;
    if db_path_overridden {
        config.database.path = paths.db_path.clone();
    }
    let bundle = spool_db::get_task_context_bundle(&pool, &options.identifier)
        .await?
        .with_context(|| format!("Task {} not found", options.identifier))?;
    if bundle.task.task.state != "human_review" {
        anyhow::bail!(
            "spool review requires Task {} to be in Human Review; current Task State is {}",
            options.identifier,
            bundle.task.task.state
        );
    }
    let review_packet = review_packet::render_review_packet(&bundle)?;
    let api_token = spool_db::ensure_local_api_token(&pool).await?;
    let api_url = options
        .api_url
        .unwrap_or_else(|| format!("http://{}", config.service.bind_addr));

    let outcome =
        spool_runner::review::run_review_session(spool_runner::review::ReviewSessionRequest {
            identifier: options.identifier,
            review_packet,
            managed_source_repository: PathBuf::from(&bundle.queue.managed_source_repository),
            api_url,
            api_token,
            actor: options.actor,
            pi_bin: options.pi_bin,
            pi_extension: options.pi_extension,
        })
        .await?;

    println!(
        "finished Review Session for Task {} with Pi-backed Review Agent",
        outcome.identifier
    );
    Ok(())
}
