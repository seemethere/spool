use super::*;

pub(crate) async fn serve(
    paths: &SpoolPaths,
    bind: Option<SocketAddr>,
    db_path_overridden: bool,
) -> Result<()> {
    let mut config = SpoolConfig::load_or_default(paths)?;
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

    let pool = spool_db::connect(&config.database.path).await?;
    spool_db::check_migration_compatibility(&pool).await?;

    spool_server::serve(bind_addr, env!("CARGO_PKG_VERSION"), pool).await
}
