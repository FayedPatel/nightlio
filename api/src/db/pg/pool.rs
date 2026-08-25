//! deadpool-postgres pool construction with rustls TLS.
//!
//! The pool is lazy: building it never touches the network, so startup
//! failures for an unreachable server surface at the first checkout — the
//! migration runner does that immediately after this and turns it into a
//! clear refuse-to-start message.
//!
//! TLS: the connector is always installed and `sslmode` in `DATABASE_URL`
//! decides whether it is used (`disable`, `prefer` — the default — or
//! `require`). Server certificates verify against the bundled web-PKI roots
//! (webpki-roots); private CAs and client certificates are not supported in
//! this experimental release — see docs/POSTGRES.md.

use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod};
use tokio_postgres_rustls::MakeRustlsConnect;

/// Upper bound on pooled connections. Deliberately modest: the API is a
/// single process, and Postgres defaults to `max_connections = 100`.
const POOL_MAX_SIZE: usize = 10;

/// Build the shared deadpool-postgres pool from a `postgres://` /
/// `postgresql://` `DATABASE_URL` (already scheme-validated by
/// `Config::database_target`). Error messages never echo the URL — it can
/// embed credentials.
pub fn build_pool(database_url: &str) -> anyhow::Result<Pool> {
    let pg_config: tokio_postgres::Config = database_url.parse().map_err(|exc| {
        anyhow::anyhow!(
            "Refusing to start: DATABASE_URL is not a valid PostgreSQL connection string \
             ({exc}). Expected postgres://user:password@host:port/dbname[?sslmode=...]."
        )
    })?;

    let manager = Manager::from_config(
        pg_config,
        MakeRustlsConnect::new(client_tls_config()),
        ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        },
    );
    Pool::builder(manager)
        .max_size(POOL_MAX_SIZE)
        .build()
        .map_err(|exc| anyhow::anyhow!("failed to build the PostgreSQL connection pool: {exc}"))
}

/// rustls client config trusting the bundled web-PKI roots, no client auth.
fn client_tls_config() -> rustls::ClientConfig {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pool construction is lazy — it must succeed without a reachable
    /// server (connectivity is checked at first checkout, by the migration
    /// runner's actionable error path).
    #[test]
    fn build_pool_is_lazy_and_accepts_a_valid_url() {
        let pool =
            build_pool("postgres://nightlio:secret@localhost:1/nightlio?sslmode=disable").unwrap();
        assert_eq!(
            pool.status().size,
            0,
            "no connection may open at build time"
        );
    }

    #[test]
    fn build_pool_rejects_a_malformed_url_without_echoing_it() {
        let err = build_pool("postgres://user@:not-a-port/db")
            .unwrap_err()
            .to_string();
        assert!(err.starts_with("Refusing to start"), "{err}");
        assert!(
            !err.contains("not-a-port") || !err.contains("user@"),
            "{err}"
        );
    }
}
