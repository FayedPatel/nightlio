use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use nightlio_api::config::Config;
use nightlio_api::db;
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

    // `nightlio-api migrate-to-postgres [--sqlite <path>] [--postgres-url
    // <url>]`: one-shot SQLite → PostgreSQL data import (docs/POSTGRES.md).
    // Same no-clap pattern as --health-check; runs before the production
    // secret validation below — it is a data tool, not the server.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("migrate-to-postgres") {
        let options = match migrate_to_postgres::parse_args(&cfg, &args[1..]) {
            Ok(options) => options,
            Err(message) => {
                eprintln!("{message}");
                std::process::exit(2);
            }
        };
        let outcome = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(migrate_to_postgres::run(&options));
        match outcome {
            Ok(()) => std::process::exit(0),
            Err(exc) => {
                eprintln!("migrate-to-postgres failed: {exc:#}");
                std::process::exit(1);
            }
        }
    }

    // `nightlio-api seed-demo [--database-url <url>] [--sqlite <path>]
    // [--force]`: fill a database with presentable demo data (the demo
    // compose stack's one-shot seed service — docs/POSTGRES.md). Same
    // no-clap pattern; runs before the production secret validation below —
    // it is a data tool, not the server.
    if args.first().map(String::as_str) == Some("seed-demo") {
        let options = match nightlio_api::seed::parse_args(&cfg, &args[1..]) {
            Ok(options) => options,
            Err(message) => {
                eprintln!("{message}");
                std::process::exit(2);
            }
        };
        let outcome = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(nightlio_api::seed::run(&options.config, options.force));
        match outcome {
            Ok(outcome) => {
                print!("{}", outcome.report());
                std::process::exit(0);
            }
            Err(exc) => {
                eprintln!("seed-demo failed: {exc:#}");
                std::process::exit(1);
            }
        }
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
    // Backend selection + boot (v0.6.0): DATABASE_URL unset means SQLite
    // exactly as before (legacy bootstrap — the idempotent v0→v3 upgrade
    // path — then the migration runner, pool, and one WAL fold); a
    // postgres:// URL selects the Postgres backend (migrations bootstrap a
    // fresh database, then the self-host baseline seed matches SQLite's);
    // any other scheme fails fast. The whole sequence lives in
    // db::connect_from_config so `seed-demo` and the integration tests
    // boot the exact database the server serves.
    let db = db::connect_from_config(&cfg).await?;

    let port = cfg.port;
    tracing::info!(
        app_env = ?cfg.app_env,
        port,
        database_path = %cfg.database_path,
        oidc_enabled = cfg.oidc_enabled(),
        "nightlio-api starting"
    );

    let app = routes::build_router(AppState::new(cfg, db.clone()));

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
    if let Err(exc) = db::checkpoint_truncate(&db) {
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

/// `nightlio-api migrate-to-postgres`: one-shot import of an existing
/// SQLite database into an (empty) PostgreSQL database, for self-hosters
/// opting into the v0.6.0 Postgres backend. Guarantees (docs/POSTGRES.md):
///
/// - the SQLite source is opened **read-only** and never modified;
/// - PostgreSQL migrations run first, then the import **refuses to run
///   unless the target `users` table is empty**;
/// - all 10 tables are copied in foreign-key order **preserving row ids
///   verbatim** (including historical garbage like US-format dates — the
///   schema stores the same text SQLite did);
/// - every identity sequence is `setval`-advanced past the copied ids;
/// - the whole import is **one transaction** with per-table row-count
///   verification printed and enforced — a mismatch aborts and rolls back.
mod migrate_to_postgres {
    use anyhow::Context;
    use rusqlite::{Connection, OpenFlags};
    use tokio_postgres::types::ToSql;

    use nightlio_api::config::{Config, DatabaseTarget};
    use nightlio_api::db;

    const USAGE: &str = "usage: nightlio-api migrate-to-postgres [--sqlite <path>] [--postgres-url <url>]\n\
        defaults: --sqlite <DATABASE_PATH>, --postgres-url <DATABASE_URL>";

    /// How a SQLite column maps onto its PostgreSQL twin. Nullability is
    /// handled uniformly: every value is read as an `Option` and NULL is
    /// written back as NULL.
    #[derive(Clone, Copy)]
    enum ColType {
        /// `bigint` (ids, counters).
        BigInt,
        /// `integer` (only `achievements.nft_minted`, the raw 0/1 boolean).
        Int,
        /// `double precision` (only `mood_entries.mood`; SQLite stores
        /// INTEGER or REAL there and both read losslessly as f64).
        Double,
        /// `text` — names, dates, and timestamp text, copied verbatim.
        Text,
    }

    use ColType::{BigInt, Double, Int, Text};

    /// The 10 tables in foreign-key order, with their full column lists.
    /// Explicit ids are inserted verbatim (the PG schema's identity columns
    /// are GENERATED BY DEFAULT for exactly this tool).
    const TABLES: &[(&str, &[(&str, ColType)])] = &[
        (
            "users",
            &[
                ("id", BigInt),
                ("google_id", Text),
                ("email", Text),
                ("name", Text),
                ("avatar_url", Text),
                ("created_at", Text),
                ("last_login", Text),
                ("auth_provider", Text),
                ("external_id", Text),
                ("password_hash", Text),
                ("theme_preference", Text),
            ],
        ),
        (
            "groups",
            &[
                ("id", BigInt),
                ("user_id", BigInt),
                ("name", Text),
                ("created_at", Text),
            ],
        ),
        (
            "group_options",
            &[
                ("id", BigInt),
                ("group_id", BigInt),
                ("name", Text),
                ("created_at", Text),
            ],
        ),
        (
            "mood_entries",
            &[
                ("id", BigInt),
                ("user_id", BigInt),
                ("date", Text),
                ("mood", Double),
                ("content", Text),
                ("created_at", Text),
                ("updated_at", Text),
            ],
        ),
        (
            "entry_selections",
            &[
                ("id", BigInt),
                ("entry_id", BigInt),
                ("option_id", BigInt),
                ("created_at", Text),
            ],
        ),
        (
            "achievements",
            &[
                ("id", BigInt),
                ("user_id", BigInt),
                ("achievement_type", Text),
                ("earned_at", Text),
                ("nft_minted", Int),
                ("nft_token_id", BigInt),
                ("nft_tx_hash", Text),
            ],
        ),
        (
            "goals",
            &[
                ("id", BigInt),
                ("user_id", BigInt),
                ("title", Text),
                ("description", Text),
                ("frequency_per_week", BigInt),
                ("completed", BigInt),
                ("streak", BigInt),
                ("period_start", Text),
                ("last_completed_date", Text),
                ("created_at", Text),
                ("updated_at", Text),
            ],
        ),
        (
            "goal_completions",
            &[
                ("id", BigInt),
                ("user_id", BigInt),
                ("goal_id", BigInt),
                ("date", Text),
                ("created_at", Text),
            ],
        ),
        (
            "user_metrics",
            &[
                ("user_id", BigInt),
                ("stats_views", BigInt),
                ("updated_at", Text),
                ("last_view_date", Text),
            ],
        ),
        (
            "activity_log",
            &[
                ("id", BigInt),
                ("user_id", BigInt),
                ("event_type", Text),
                ("metadata", Text),
                ("created_at", Text),
            ],
        ),
    ];

    /// Tables whose `id` is an identity column with a sequence to advance
    /// (all of them except `user_metrics`, whose PK is `user_id`).
    const IDENTITY_TABLES: &[&str] = &[
        "users",
        "groups",
        "group_options",
        "mood_entries",
        "entry_selections",
        "achievements",
        "goals",
        "goal_completions",
        "activity_log",
    ];

    pub struct Options {
        pub sqlite_path: String,
        pub postgres_url: String,
    }

    /// No-clap argument parsing (the `--health-check` convention). Errors
    /// are user-facing strings carrying the usage line; the caller exits 2.
    pub fn parse_args(cfg: &Config, args: &[String]) -> Result<Options, String> {
        let mut sqlite_path = cfg.database_path.clone();
        let mut postgres_url: Option<String> = None;
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--sqlite" => {
                    sqlite_path = iter
                        .next()
                        .cloned()
                        .ok_or_else(|| format!("--sqlite needs a value\n{USAGE}"))?;
                }
                "--postgres-url" => {
                    postgres_url = Some(
                        iter.next()
                            .cloned()
                            .ok_or_else(|| format!("--postgres-url needs a value\n{USAGE}"))?,
                    );
                }
                other => return Err(format!("unknown argument `{other}`\n{USAGE}")),
            }
        }
        let postgres_url = match postgres_url {
            Some(url) => url,
            None => match cfg.database_target().map_err(|exc| exc.to_string())? {
                DatabaseTarget::Postgres(url) => url,
                DatabaseTarget::Sqlite => {
                    return Err(format!(
                        "no PostgreSQL target: set DATABASE_URL or pass --postgres-url\n{USAGE}"
                    ));
                }
            },
        };
        Ok(Options {
            sqlite_path,
            postgres_url,
        })
    }

    pub async fn run(options: &Options) -> anyhow::Result<()> {
        // 1. SQLite source, strictly read-only (the flag makes SQLite
        //    itself refuse any write; a missing file is an error instead of
        //    an implicit empty database).
        if !std::path::Path::new(&options.sqlite_path).is_file() {
            anyhow::bail!(
                "SQLite source not found at {} (set DATABASE_PATH or pass --sqlite)",
                options.sqlite_path
            );
        }
        let sqlite = Connection::open_with_flags(
            &options.sqlite_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("opening {} read-only", options.sqlite_path))?;
        let user_version: i64 = sqlite.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if user_version < 3 {
            anyhow::bail!(
                "the SQLite source is not fully bootstrapped (user_version {user_version} < 3): \
                 start the current release once against it (the automatic bootstrap upgrades it \
                 in place, see docs/UPGRADING.md), then re-run the import"
            );
        }

        // 2. Target schema via the real migration runner (0001_baseline on
        //    a fresh database; a ledger no-op plus integrity verification
        //    otherwise).
        let pool = db::pg::build_pool(&options.postgres_url)?;
        db::migrate::run_postgres(&pool).await?;

        // 3. Everything below happens in ONE transaction: the import lands
        //    completely or not at all.
        let mut client = pool.get().await.context("checking out a connection")?;
        let tx = client
            .transaction()
            .await
            .context("opening the import transaction")?;

        let existing: i64 = tx
            .query_one("SELECT COUNT(*) FROM users", &[])
            .await
            .context("counting target users")?
            .get(0);
        if existing > 0 {
            anyhow::bail!(
                "Refusing to import: the target PostgreSQL database already contains {existing} \
                 user(s). migrate-to-postgres only fills an empty database — point it at a fresh \
                 database (or drop and recreate this one) and re-run."
            );
        }

        // 4. Copy in FK order, preserving ids verbatim.
        for (table, columns) in TABLES {
            let copied = copy_table(&sqlite, &tx, table, columns).await?;
            println!("copied {copied:>6} row(s) into {table}");
        }

        // 5. Advance every identity sequence past the copied ids so future
        //    inserts never collide.
        for table in IDENTITY_TABLES {
            let max_id: Option<i64> = tx
                .query_one(&format!("SELECT max(id) FROM {table}"), &[])
                .await
                .with_context(|| format!("reading max(id) of {table}"))?
                .get(0);
            if let Some(max_id) = max_id {
                tx.query_one(
                    &format!("SELECT setval(pg_get_serial_sequence('{table}', 'id'), $1)"),
                    &[&max_id],
                )
                .await
                .with_context(|| format!("advancing the {table} id sequence"))?;
            }
        }

        // 6. Enforced per-table row-count verification, inside the still
        //    uncommitted transaction: a mismatch rolls everything back.
        println!("{:<18} {:>8} {:>8}", "table", "sqlite", "postgres");
        let mut mismatched = Vec::new();
        for (table, _) in TABLES {
            let source: i64 =
                sqlite.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })?;
            let target: i64 = tx
                .query_one(&format!("SELECT COUNT(*) FROM {table}"), &[])
                .await?
                .get(0);
            let marker = if source == target { "" } else { "  MISMATCH" };
            println!("{table:<18} {source:>8} {target:>8}{marker}");
            if source != target {
                mismatched.push(*table);
            }
        }
        if !mismatched.is_empty() {
            anyhow::bail!(
                "row-count verification failed for: {} — nothing was written (the transaction \
                 rolled back)",
                mismatched.join(", ")
            );
        }

        tx.commit().await.context("committing the import")?;
        println!(
            "Import complete. Set DATABASE_URL and start the API; keep the SQLite file at {} \
             as your rollback path (it was opened read-only and never modified).",
            options.sqlite_path
        );
        Ok(())
    }

    /// Copy one table row-by-row (`SELECT` on the read-only source,
    /// prepared `INSERT` inside the target transaction). Returns the number
    /// of rows written.
    async fn copy_table(
        sqlite: &Connection,
        tx: &tokio_postgres::Transaction<'_>,
        table: &str,
        columns: &[(&str, ColType)],
    ) -> anyhow::Result<i64> {
        let column_list = columns
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(", ");
        let placeholders = (1..=columns.len())
            .map(|idx| format!("${idx}"))
            .collect::<Vec<_>>()
            .join(", ");
        let order_by = columns[0].0; // the PK leads every column list
        let mut select = sqlite
            .prepare(&format!(
                "SELECT {column_list} FROM {table} ORDER BY {order_by}"
            ))
            .with_context(|| {
                format!("preparing the {table} SELECT (is the source fully migrated?)")
            })?;
        let insert = tx
            .prepare(&format!(
                "INSERT INTO {table} ({column_list}) VALUES ({placeholders})"
            ))
            .await
            .with_context(|| format!("preparing the {table} INSERT"))?;

        let mut rows = select.query([])?;
        let mut copied = 0i64;
        while let Some(row) = rows.next()? {
            let mut values: Vec<Box<dyn ToSql + Sync>> = Vec::with_capacity(columns.len());
            for (idx, (name, kind)) in columns.iter().enumerate() {
                let context = || format!("reading {table}.{name} (row {})", copied + 1);
                match kind {
                    ColType::BigInt => {
                        let value: Option<i64> = row.get(idx).with_context(context)?;
                        values.push(Box::new(value));
                    }
                    ColType::Int => {
                        let value: Option<i64> = row.get(idx).with_context(context)?;
                        let value: Option<i32> =
                            value.map(i32::try_from).transpose().with_context(context)?;
                        values.push(Box::new(value));
                    }
                    ColType::Double => {
                        let value: Option<f64> = row.get(idx).with_context(context)?;
                        values.push(Box::new(value));
                    }
                    ColType::Text => {
                        let value: Option<String> = row.get(idx).with_context(context)?;
                        values.push(Box::new(value));
                    }
                }
            }
            let params: Vec<&(dyn ToSql + Sync)> =
                values.iter().map(|value| value.as_ref()).collect();
            tx.execute(&insert, &params)
                .await
                .with_context(|| format!("inserting into {table} (row {})", copied + 1))?;
            copied += 1;
        }
        Ok(copied)
    }
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
