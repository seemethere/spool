use std::{env, fs, path::PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_BIND_ADDR: &str = "127.0.0.1:4317";
pub const CONFIG_ENV: &str = "TASKER_CONFIG";
pub const DATA_DIR_ENV: &str = "TASKER_DATA_DIR";
pub const DB_PATH_ENV: &str = "TASKER_DB_PATH";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PathOverrides {
    pub config_path: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
    pub db_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskerPaths {
    pub config_path: PathBuf,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
}

impl TaskerPaths {
    pub fn from_env(overrides: PathOverrides) -> Result<Self> {
        let resolved_overrides = PathOverrides {
            config_path: overrides
                .config_path
                .or_else(|| env::var_os(CONFIG_ENV).map(PathBuf::from)),
            data_dir: overrides
                .data_dir
                .or_else(|| env::var_os(DATA_DIR_ENV).map(PathBuf::from)),
            db_path: overrides
                .db_path
                .or_else(|| env::var_os(DB_PATH_ENV).map(PathBuf::from)),
        };

        let home =
            if resolved_overrides.config_path.is_none() || resolved_overrides.data_dir.is_none() {
                env::var_os("HOME")
                    .map(PathBuf::from)
                    .context("HOME is not set; pass explicit Tasker config and data paths")?
            } else {
                PathBuf::new()
            };

        Ok(Self::resolve(home, resolved_overrides))
    }

    pub fn resolve(home: impl Into<PathBuf>, overrides: PathOverrides) -> Self {
        let home = home.into();
        let config_path = overrides
            .config_path
            .unwrap_or_else(|| home.join(".config/tasker/config.toml"));
        let data_dir = overrides.data_dir.unwrap_or_else(|| {
            if is_repository_local_config(&config_path) {
                config_path
                    .parent()
                    .map(|parent| parent.join("data"))
                    .unwrap_or_else(|| home.join(".local/share/tasker"))
            } else {
                home.join(".local/share/tasker")
            }
        });
        let db_path = overrides
            .db_path
            .unwrap_or_else(|| data_dir.join("tasker.db"));

        Self {
            config_path,
            data_dir,
            db_path,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TaskerConfig {
    pub service: ServiceConfig,
    pub database: DatabaseConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ServiceConfig {
    pub bind_addr: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DatabaseConfig {
    pub path: PathBuf,
}

impl TaskerConfig {
    pub fn default_for_paths(paths: &TaskerPaths) -> Self {
        Self {
            service: ServiceConfig {
                bind_addr: DEFAULT_BIND_ADDR.to_string(),
            },
            database: DatabaseConfig {
                path: paths.db_path.clone(),
            },
        }
    }

    pub fn load_or_default(paths: &TaskerPaths) -> Result<Self> {
        if paths.config_path.exists() {
            Self::load(paths)
        } else {
            Ok(Self::default_for_paths(paths))
        }
    }

    pub fn load(paths: &TaskerPaths) -> Result<Self> {
        let text = fs::read_to_string(&paths.config_path)
            .with_context(|| format!("failed to read {}", paths.config_path.display()))?;
        toml::from_str(&text)
            .with_context(|| format!("failed to parse {}", paths.config_path.display()))
    }

    pub fn write_if_missing(&self, paths: &TaskerPaths) -> Result<bool> {
        if paths.config_path.exists() {
            return Ok(false);
        }

        if let Some(parent) = paths.config_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let text = toml::to_string_pretty(self).context("failed to serialize Tasker config")?;
        fs::write(&paths.config_path, text)
            .with_context(|| format!("failed to write {}", paths.config_path.display()))?;
        Ok(true)
    }
}

fn is_repository_local_config(config_path: &std::path::Path) -> bool {
    config_path.file_name().and_then(|name| name.to_str()) == Some("config.toml")
        && config_path
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            == Some(".tasker")
}

pub fn ensure_data_dir(paths: &TaskerPaths) -> Result<()> {
    fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("failed to create {}", paths.data_dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_default_xdg_paths() {
        let paths = TaskerPaths::resolve("/tmp/home", PathOverrides::default());

        assert_eq!(
            paths.config_path,
            PathBuf::from("/tmp/home/.config/tasker/config.toml")
        );
        assert_eq!(
            paths.data_dir,
            PathBuf::from("/tmp/home/.local/share/tasker")
        );
        assert_eq!(
            paths.db_path,
            PathBuf::from("/tmp/home/.local/share/tasker/tasker.db")
        );
    }

    #[test]
    fn explicit_repository_local_config_uses_sibling_data_dir() {
        let paths = TaskerPaths::resolve(
            "/tmp/home",
            PathOverrides {
                config_path: Some(PathBuf::from("/repo/.tasker/config.toml")),
                ..PathOverrides::default()
            },
        );

        assert_eq!(paths.data_dir, PathBuf::from("/repo/.tasker/data"));
        assert_eq!(paths.db_path, PathBuf::from("/repo/.tasker/data/tasker.db"));
    }

    #[test]
    fn explicit_non_project_config_keeps_default_xdg_data_dir() {
        let paths = TaskerPaths::resolve(
            "/tmp/home",
            PathOverrides {
                config_path: Some(PathBuf::from("/tmp/custom-config.toml")),
                ..PathOverrides::default()
            },
        );

        assert_eq!(
            paths.data_dir,
            PathBuf::from("/tmp/home/.local/share/tasker")
        );
    }

    #[test]
    fn explicit_data_dir_changes_default_db_path() {
        let paths = TaskerPaths::resolve(
            "/tmp/home",
            PathOverrides {
                data_dir: Some(PathBuf::from("/tmp/tasker-data")),
                ..PathOverrides::default()
            },
        );

        assert_eq!(paths.data_dir, PathBuf::from("/tmp/tasker-data"));
        assert_eq!(paths.db_path, PathBuf::from("/tmp/tasker-data/tasker.db"));
    }

    #[test]
    fn explicit_db_path_wins_over_data_dir_default() {
        let paths = TaskerPaths::resolve(
            "/tmp/home",
            PathOverrides {
                data_dir: Some(PathBuf::from("/tmp/tasker-data")),
                db_path: Some(PathBuf::from("/tmp/custom.db")),
                ..PathOverrides::default()
            },
        );

        assert_eq!(paths.db_path, PathBuf::from("/tmp/custom.db"));
    }

    #[test]
    fn writes_config_without_overwriting_existing_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = TaskerPaths::resolve(temp.path(), PathOverrides::default());
        let config = TaskerConfig::default_for_paths(&paths);

        assert!(config.write_if_missing(&paths).expect("first write"));
        let first = fs::read_to_string(&paths.config_path).expect("read first");
        assert!(!config.write_if_missing(&paths).expect("second write"));
        let second = fs::read_to_string(&paths.config_path).expect("read second");

        assert_eq!(first, second);
    }
}
