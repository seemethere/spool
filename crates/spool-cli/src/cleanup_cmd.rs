use super::*;

pub(crate) async fn cleanup(
    paths: &SpoolPaths,
    db_path_overridden: bool,
    command: CleanupCommand,
) -> Result<()> {
    match command {
        CleanupCommand::LocalWorktrees {
            queue,
            dry_run: _,
            delete,
        } => {
            let pool = open_pool(paths, db_path_overridden).await?;
            let queue_record = spool_db::get_task_queue(&pool, &queue)
                .await?
                .with_context(|| format!("Task Queue {queue} not found"))?;
            let report = cleanup::cleanup_local_worktrees(&pool, &queue_record, delete).await?;
            print_local_worktree_cleanup_report(&report);
        }
        CleanupCommand::CargoTargets {
            queue,
            worktree_root,
            dry_run: _,
            delete,
        } => {
            let worktree_root = if let Some(root) = worktree_root {
                root
            } else if let Some(queue_key) = queue {
                let pool = open_pool(paths, db_path_overridden).await?;
                let queue = spool_db::get_task_queue(&pool, &queue_key)
                    .await?
                    .with_context(|| format!("Task Queue {queue_key} not found"))?;
                PathBuf::from(queue.worktree_root)
            } else {
                anyhow::bail!("cleanup cargo-targets requires --queue or --worktree-root");
            };
            let report = cleanup::cleanup_cargo_targets(&worktree_root, delete)?;
            println!("Local Worktree Cargo target cleanup");
            println!("Worktree Root: {}", worktree_root.display());
            println!("mode: {}", if delete { "delete" } else { "dry-run" });
            println!(
                "safe-to-delete artifact kind: rebuildable per-Local Worktree target/ directories"
            );
            println!("preserved Task data: Local Worktree source files, Task Branches, Task records, Agent Runs, and Audit Events");
            print_cleanup_report(&report);
        }
        CleanupCommand::Runs {
            runs_dir,
            older_than_days,
            keep_latest,
            dry_run: _,
            delete,
        } => {
            let runs_dir = runs_dir.unwrap_or_else(|| paths.data_dir.join("runs"));
            let report = cleanup::cleanup_run_artifacts(
                &runs_dir,
                cleanup::RunPruneOptions {
                    older_than_days,
                    keep_latest,
                },
                delete,
            )?;
            println!("Run Transcript and Launcher Session Data artifact cleanup");
            println!("Run artifact root: {}", runs_dir.display());
            println!("mode: {}", if delete { "delete" } else { "dry-run" });
            if let Some(days) = older_than_days {
                println!("selection: older than {days} day(s)");
            }
            if let Some(keep) = keep_latest {
                println!("selection: keep newest {keep} artifact(s)");
            }
            if older_than_days.is_none() && keep_latest.is_none() {
                println!("selection: summarize all artifacts");
            }
            println!("safe-to-delete artifact kind: saved Run Transcript files and launcher raw/session artifacts under runs/");
            println!("preserved authoritative data: Task records, Agent Run rows, Launcher Session Data database rows, and Audit Events");
            print_cleanup_report(&report);
        }
    }
    Ok(())
}

fn print_cleanup_report(report: &cleanup::CleanupReport) {
    println!(
        "{} entries, {} reclaimable",
        report.entries.len(),
        cleanup::human_bytes(report.total_bytes())
    );
    for entry in &report.entries {
        println!(
            "  {}	{}",
            cleanup::human_bytes(entry.bytes),
            entry.path.display()
        );
    }
}

fn print_local_worktree_cleanup_report(report: &cleanup::LocalWorktreeCleanupReport) {
    println!("Done/Canceled Local Worktree and Task Branch cleanup");
    println!("Task Queue: {}", report.queue_key);
    println!(
        "Managed Source Repository: {}",
        report.managed_source_repository.display()
    );
    println!("Worktree Root: {}", report.worktree_root.display());
    println!(
        "Done Worktree Retention: {}",
        report.done_worktree_retention
    );
    println!(
        "mode: {}",
        if report.deleted { "delete" } else { "dry-run" }
    );
    println!("preserved authoritative data: Task records, Audit Events, Agent Run rows, Run Transcripts, and Launcher Session Data");
    let safe = report
        .entries
        .iter()
        .filter(|entry| entry.safe_to_delete)
        .count();
    let attention = report.entries.len().saturating_sub(safe);
    println!(
        "{} safe cleanup candidate(s), {} need attention",
        safe, attention
    );
    for entry in &report.entries {
        println!(
            "  [{}] {} ({})",
            if entry.safe_to_delete {
                "safe"
            } else {
                "attention"
            },
            entry.identifier,
            entry.state
        );
        if let Some(worktree) = &entry.local_worktree {
            println!("    Local Worktree: {}", worktree.display());
        } else {
            println!("    Local Worktree: <missing link>");
        }
        if let Some(branch) = &entry.task_branch {
            println!("    Task Branch: {branch}");
        } else {
            println!("    Task Branch: <missing link>");
        }
        for reason in &entry.reasons {
            println!("    needs attention: {reason}");
        }
        for action in &entry.actions {
            println!("    action: {action}");
        }
    }
}
