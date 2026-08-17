use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use nightlio_api::config::Config;
use nightlio_api::db::{self, SelfHostSeed};
use nightlio_api::routes;
use nightlio_api::state::AppState;

fn main() -> anyhow::Result<()> {
    // Load .env with never-override semantics: dotenvy::dotenv() searches the
    // current directory and its ancestors (so the repo-root .env is found when
    // running from api/ or the repo root) and preserves any variable that
    // is already set in the process environment — mirroring python-dotenv's
    // default behavior in api/config.py. A missing .env is not an error.
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cfg = Config::from_env();

    // `nightlio-api --health-check`: Docker healthcheck helper. GETs the
    // local health endpoint and exits 0 (healthy) or 1 (anything else).
    if std::env::args().skip(1).any(|arg| arg == "--health-check") {
        std::process::exit(if run_health_check(cfg.port) { 0 } else { 1 });
    }

    // Fail closed in production on missing/placeholder/short signing keys,
    // exactly like create_app() in api/app.py. The error message is grepped
    // by api/docker_start.py for the substring "SECRET_KEY/JWT_SECRET".
    cfg.validate_production_secrets()?;

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(serve(cfg))
}

async fn serve(cfg: Config) -> anyhow::Result<()> {
    // Schema bootstrap runs before the pool serves a single query, exactly
    // like MoodDatabase.__init__ → init_database() in create_app().
    db::bootstrap(&cfg.database_path, &SelfHostSeed::from(&cfg))?;
    let pool = db::open_pool(&cfg.database_path)?;
    // The pool just switched the main DB to WAL; fold and truncate the log
    // once before serving traffic so the -wal file starts at zero bytes.
    db::checkpoint_truncate(&pool)?;

    let port = cfg.port;
    tracing::info!(
        app_env = ?cfg.app_env,
        port,
        database_path = %cfg.database_path,
        oidc_enabled = cfg.oidc_enabled(),
        "nightlio-api starting"
    );

    let app = routes::build_router(AppState::new(cfg, pool.clone()));

    // Bind [::] (dual-stack on Linux: accepts IPv4-mapped connections too);
    // fall back to 0.0.0.0 on IPv6-less hosts.
    let v6 = SocketAddr::from((Ipv6Addr::UNSPECIFIED, port));
    let listener = match tokio::net::TcpListener::bind(v6).await {
        Ok(listener) => listener,
        Err(exc) => {
            tracing::warn!("binding {v6} failed ({exc}); falling back to 0.0.0.0:{port}");
            tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))).await?
        }
    };
    tracing::info!("listening on {}", listener.local_addr()?);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // Graceful shutdown: fold and truncate the WAL so the volume is left
    // with an empty -wal file (backup- and rollback-friendly). Best-effort —
    // a failure here must not turn a clean shutdown into a non-zero exit.
    if let Err(exc) = db::checkpoint_truncate(&pool) {
        tracing::warn!("wal_checkpoint(TRUNCATE) on shutdown failed: {exc}");
    }
    Ok(())
}

/// Resolve when the process is asked to stop: Ctrl-C (SIGINT) or, on Unix,
/// SIGTERM (what `docker stop` sends).
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sigterm) => {
                sigterm.recv().await;
            }
            Err(exc) => {
                tracing::warn!("SIGTERM handler unavailable: {exc}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutdown signal received; draining connections");
}

/// GET `http://localhost:<port>/api/` and report whether it answered 2xx.
fn run_health_check(port: u16) -> bool {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return false;
    };
    runtime.block_on(async {
        match reqwest::get(format!("http://localhost:{port}/api/")).await {
            Ok(response) => response.status().is_success(),
            Err(exc) => {
                eprintln!("health check failed: {exc}");
                false
            }
        }
    })
}
