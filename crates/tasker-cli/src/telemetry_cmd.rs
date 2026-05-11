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
            efficiency_budget,
            adaptive_budget_window,
            json,
        } => {
            let config = TaskerConfig::load_or_default(paths)?;
            let mode_text = efficiency_budget
                .as_deref()
                .unwrap_or(&config.telemetry.efficiency_budget);
            let mode = match mode_text {
                "fixed" => telemetry::EfficiencyBudgetMode::Fixed,
                "adaptive" => telemetry::EfficiencyBudgetMode::Adaptive,
                other => anyhow::bail!(
                    "invalid telemetry.efficiency_budget {other:?}; expected fixed or adaptive"
                ),
            };
            let summary = telemetry::summarize_agent_runs(
                &pool,
                &telemetry::TelemetryOptions {
                    queue,
                    slow_limit,
                    budget: telemetry::EfficiencyBudgetOptions {
                        mode,
                        window_size: adaptive_budget_window
                            .map(|value| value as usize)
                            .unwrap_or(config.telemetry.adaptive_efficiency_budget_window),
                        min_metric_coverage: config
                            .telemetry
                            .adaptive_efficiency_budget_min_coverage,
                    },
                },
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
        TelemetryCommand::Efficiency {
            queue,
            recent,
            top_limit,
            json,
        } => {
            let summary = telemetry::recent_efficiency_summary(
                &pool,
                &telemetry::RecentEfficiencyOptions {
                    queue,
                    recent,
                    top_limit,
                },
            )
            .await?;
            if json {
                serde_json::to_writer_pretty(std::io::stdout(), &summary)?;
                println!();
            } else {
                print!("{}", telemetry::render_recent_efficiency_summary(&summary));
            }
        }
    }
    Ok(())
}
