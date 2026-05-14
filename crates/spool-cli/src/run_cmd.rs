use super::*;

pub(crate) async fn run(
    paths: &SpoolPaths,
    db_path_overridden: bool,
    command: RunCommand,
) -> Result<()> {
    let pool = open_pool(paths, db_path_overridden).await?;
    match command {
        RunCommand::Show { run_id, json } => {
            let detail = spool_db::get_agent_run_detail(&pool, &run_id)
                .await?
                .with_context(|| format!("Agent Run {run_id} not found"))?;
            if json {
                output::write_run_detail_json(std::io::stdout(), &detail)?;
            } else {
                output::print_run_detail(&detail)?;
            }
        }
        RunCommand::Fail {
            run_id,
            reason,
            failure_reason_code,
            retry_hold_seconds,
            actor,
        } => {
            let run = spool_db::operator_fail_run(
                &pool,
                &run_id,
                &spool_db::OperatorFailRunInput {
                    failure_reason: reason,
                    failure_reason_code,
                    retry_hold_seconds,
                },
                &spool_db::Actor::operator(actor),
            )
            .await?;
            let detail = spool_db::get_agent_run_detail(&pool, &run.id)
                .await?
                .with_context(|| format!("Agent Run {} not found after failure", run.id))?;
            println!("failed Agent Run {}", detail.run.id);
            println!(
                "retry hold created for Task {}",
                detail.task.task.identifier
            );
        }
    }
    Ok(())
}
