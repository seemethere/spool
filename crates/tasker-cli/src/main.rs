use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tasker_config::{ensure_data_dir, PathOverrides, TaskerConfig, TaskerPaths};

#[derive(Debug, Parser)]
#[command(name = "tasker")]
#[command(about = "Local-first task backend for agent-driven development")]
#[command(version)]
struct Cli {
    /// Override the Tasker config file path.
    #[arg(long, global = true, env = "TASKER_CONFIG")]
    config: Option<PathBuf>,

    /// Override the Tasker data directory.
    #[arg(long, global = true, env = "TASKER_DATA_DIR")]
    data_dir: Option<PathBuf>,

    /// Override the Tasker SQLite database path.
    #[arg(long, global = true, env = "TASKER_DB_PATH")]
    db_path: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Initialize Tasker local config, data directory, and database.
    Init,
    /// Start the Tasker Service.
    Serve {
        /// Override the service bind address.
        #[arg(long)]
        bind: Option<SocketAddr>,
    },
    /// Show the Tasker CLI version.
    Version,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    let paths = cli.paths()?;
    let db_path_overridden = cli.db_path.is_some();

    match cli.command {
        Some(Command::Init) => init(&paths, db_path_overridden).await,
        Some(Command::Serve { bind }) => serve(&paths, bind, db_path_overridden).await,
        Some(Command::Version) => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        None => {
            println!("Tasker CLI skeleton. Run `tasker --help` for usage.");
            Ok(())
        }
    }
}

impl Cli {
    fn paths(&self) -> Result<TaskerPaths> {
        TaskerPaths::from_env(PathOverrides {
            config_path: self.config.clone(),
            data_dir: self.data_dir.clone(),
            db_path: self.db_path.clone(),
        })
    }
}

async fn init(paths: &TaskerPaths, db_path_overridden: bool) -> Result<()> {
    ensure_data_dir(paths)?;

    let mut config = TaskerConfig::load_or_default(paths)?;
    if db_path_overridden {
        config.database.path = paths.db_path.clone();
    }
    let wrote_config = config.write_if_missing(paths)?;
    ensure_db_parent(&config.database.path)?;

    let pool = tasker_db::connect(&config.database.path).await?;
    tasker_db::run_migrations(&pool).await?;
    let token = tasker_db::ensure_local_api_token(&pool).await?;

    println!("Tasker initialized");
    println!("config: {}", paths.config_path.display());
    println!("data: {}", paths.data_dir.display());
    println!("database: {}", config.database.path.display());
    println!("local api token: {token}");
    if !wrote_config {
        println!("config already existed; left unchanged");
    }

    Ok(())
}

async fn serve(
    paths: &TaskerPaths,
    bind: Option<SocketAddr>,
    db_path_overridden: bool,
) -> Result<()> {
    let mut config = TaskerConfig::load_or_default(paths)?;
    if db_path_overridden {
        config.database.path = paths.db_path.clone();
    }
    let bind_addr = match bind {
        Some(bind) => bind,
        None => config
            .service
            .bind_addr
            .parse()
            .with_context(|| format!("invalid bind address {}", config.service.bind_addr))?,
    };

    let pool = tasker_db::connect(&config.database.path).await?;
    tasker_db::run_migrations(&pool).await?;

    tasker_server::serve(bind_addr, env!("CARGO_PKG_VERSION"), pool).await
}

fn ensure_db_parent(db_path: &Path) -> Result<()> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use clap::CommandFactory;

    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[tokio::test]
    async fn init_creates_local_state_and_is_idempotent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = TaskerPaths::resolve(temp.path(), PathOverrides::default());

        init(&paths, false).await.expect("first init");
        let config_text = fs::read_to_string(&paths.config_path).expect("config text");
        init(&paths, false).await.expect("second init");

        assert!(paths.data_dir.is_dir());
        assert!(paths.config_path.is_file());
        assert!(paths.db_path.is_file());
        assert_eq!(fs::read_to_string(&paths.config_path).unwrap(), config_text);
    }

    #[tokio::test]
    async fn init_creates_parent_directory_for_custom_db_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = TaskerPaths::resolve(
            temp.path(),
            PathOverrides {
                db_path: Some(temp.path().join("custom/sub/tasker.db")),
                ..PathOverrides::default()
            },
        );

        init(&paths, true).await.expect("init");

        assert!(paths.db_path.is_file());
    }

    #[tokio::test]
    async fn init_uses_existing_config_database_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = TaskerPaths::resolve(temp.path(), PathOverrides::default());
        let configured_db_path = temp.path().join("configured/tasker.db");
        let config = TaskerConfig {
            service: tasker_config::ServiceConfig {
                bind_addr: tasker_config::DEFAULT_BIND_ADDR.to_string(),
            },
            database: tasker_config::DatabaseConfig {
                path: configured_db_path.clone(),
            },
        };
        config.write_if_missing(&paths).expect("write config");

        init(&paths, false).await.expect("init");

        assert!(configured_db_path.is_file());
        assert!(!paths.db_path.exists());
    }
}
