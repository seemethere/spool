use super::*;

pub(crate) async fn telemetry(
    paths: &TaskerPaths,
    db_path_overridden: bool,
    command: TelemetryCommand,
) -> Result<()> {
    let pool = open_pool(paths, db_path_overridden).await?;
    match command {
        TelemetryCommand::BackfillMetrics { queue, write, json } => {
            let summary = telemetry::backfill_agent_run_metrics(
                &pool,
                &telemetry::BackfillOptions { queue, write },
            )
            .await?;
            if json {
                serde_json::to_writer_pretty(std::io::stdout(), &summary)?;
                println!();
            } else {
                print!("{}", telemetry::render_backfill_summary(&summary));
            }
        }
        TelemetryCommand::Summary {
            queue,
            slow_limit,
            json,
        } => {
            let summary = telemetry::summarize_agent_runs(
                &pool,
                &telemetry::TelemetryOptions { queue, slow_limit },
            )
            .await?;
            if json {
                serde_json::to_writer_pretty(std::io::stdout(), &summary)?;
                println!();
            } else {
                print!("{}", telemetry::render_summary(&summary));
            }
        }
        TelemetryCommand::Lifecycle { queue, limit } => {
            let summary = telemetry::lifecycle_summary(
                &pool,
                &telemetry::LifecycleTelemetryOptions { queue, limit },
            )
            .await?;
            print!("{}", telemetry::render_lifecycle_summary(&summary));
        }
        TelemetryCommand::Correlation {
            queue,
            landing_tasks,
            landing_commits,
            landing_timestamps,
            json,
        } => {
            let summary = telemetry::correlation_summary(
                &pool,
                &telemetry::CorrelationOptions {
                    queue,
                    landing_tasks,
                    landing_commits,
                    landing_timestamps,
                },
            )
            .await?;
            if json {
                serde_json::to_writer_pretty(std::io::stdout(), &summary)?;
                println!();
            } else {
                print!("{}", telemetry::render_correlation_summary(&summary));
            }
        }
        TelemetryCommand::Trend {
            queue,
            landing_tasks,
            landing_timestamps,
            before_runs,
            after_runs,
            json,
        } => {
            let summary = telemetry::trend_summary(
                &pool,
                &telemetry::TrendOptions {
                    queue,
                    landing_tasks,
                    landing_timestamps,
                    before_runs,
                    after_runs,
                },
            )
            .await?;
            if json {
                serde_json::to_writer_pretty(std::io::stdout(), &summary)?;
                println!();
            } else {
                print!("{}", telemetry::render_trend_summary(&summary));
            }
        }
    }
    Ok(())
}
