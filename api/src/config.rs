//! Runtime configuration, ported from `api/config.py` (both the Flask-style
//! `Config` classes and the typed `ConfigData` returned by `get_config()`),
//! plus the env-selection logic in `api/start.py` / `api/wsgi.py` /
//! `api/docker_start.py` and the rate-limit DB path resolution in
//! `api/utils/rate_limiter.py`.
//!
//! `.env` loading lives in `main.rs` (`dotenvy::dotenv()`, never-override
//! semantics, searched from the current directory upward so the repo-root
//! `.env` is found). This module only reads the process environment — and,
//! for testability, any `&dyn Fn(&str) -> Option<String>` lookup.

use anyhow::bail;

/// Mirrors the Flask config mapping in `api/config.py` (`development`,
/// `production`, `testing`; anything else falls back to the `default`
/// entry, which is the development config).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEnv {
    Development,
    Production,
    Testing,
}

impl AppEnv {
    /// Env selection as in the serving entrypoints (`api/start.py`,
    /// `api/wsgi.py`, `api/docker_start.py`): `APP_ENV`, falling back to
    /// `RAILWAY_ENVIRONMENT`, defaulting to `production`.
    fn from_lookup(env: &dyn Fn(&str) -> Option<String>) -> Self {
        let name = non_empty(env("APP_ENV"))
            .or_else(|| non_empty(env("RAILWAY_ENVIRONMENT")))
            .unwrap_or_else(|| "production".to_string());
        match name.as_str() {
            "production" => AppEnv::Production,
            "testing" => AppEnv::Testing,
            "development" => AppEnv::Development,
            // config["default"] is DevelopmentConfig in api/config.py.
            _ => AppEnv::Development,
        }
    }
}

/// JWT access-token lifetime, seconds (`Config.JWT_ACCESS_TOKEN_EXPIRES`
/// in `api/config.py`).
pub const JWT_ACCESS_TOKEN_EXPIRES_SECS: u64 = 3600;

/// Which backend serves the main data store (v0.6.0). Selected by
/// `DATABASE_URL`: unset/empty means SQLite via `DATABASE_PATH`, exactly as
/// before; a `postgres://` or `postgresql://` URL opts into Postgres. The
/// rate limiter is unaffected — it keeps its own SQLite file
/// (`rate_limit_db_path`) on both backends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatabaseTarget {
    /// Embedded SQLite at [`Config::database_path`] (the default).
    Sqlite,
    /// Opt-in Postgres; carries the full `DATABASE_URL` connection string.
    Postgres(String),
}

/// Default CORS origins. Deliberately diverges from the Flask default
/// (`api/config.py::Config.CORS_ORIGINS`), which shipped a credentialed
/// grant to the third-party `https://nightlio.vercel.app` origin for any
/// deployment that never set `CORS_ORIGINS`. Defaults are localhost-only;
/// public deployments must set `CORS_ORIGINS` explicitly.
const DEFAULT_CORS_ORIGINS: &str = "http://localhost:5173,http://localhost:5000";

/// The well-known dev fallback signing key. Weak by definition; production
/// startup refuses it via [`Config::validate_production_secrets`].
const DEV_SECRET_KEY: &str = "dev-secret-key-change-in-production";

/// Hot-swappable language packs (v0.6.0, `api/src/i18n.rs`): the GitHub
/// repository whose `lang-<code>-v<semver>` releases carry the pack assets.
const DEFAULT_I18N_GITHUB_REPO: &str = "FayedPatel/nightlio";

/// GitHub API base URL for release discovery. Overridable via
/// `I18N_GITHUB_API_BASE` so tests can point wiremock at a fake GitHub —
/// the same test seam idea as `JAMENDO_API_BASE` in
/// `api/src/routes/extras.rs`.
const DEFAULT_I18N_GITHUB_API_BASE: &str = "https://api.github.com";

/// Default TTL (seconds) between language-pack release-discovery refreshes.
const DEFAULT_I18N_REFRESH_SECS: u64 = 3600;

/// Known placeholder secrets, ported verbatim from
/// `api/config.py::_KNOWN_WEAK_SECRETS`. Kept in sync with the compose /
/// env-example placeholders so a self-hoster who copies an example file
/// verbatim is still caught.
const KNOWN_WEAK_SECRETS: &[&str] = &[
    "dev-secret-key-change-in-production",
    "your-secret-key-change-this",
    "your-jwt-secret-change-this",
    "your-secret-key-change-this-to-something-random-and-secure",
    "your-jwt-secret-change-this-to-something-different-and-secure",
    "changeme",
    "change-me",
    "change_me",
    "secret",
    "password",
    "nightlio",
];

/// Below this many characters a value is rejected outright regardless of
/// content — 16 is a conservative floor for an HS256 signing key
/// (`api/config.py::_MIN_SECRET_LENGTH`).
const MIN_SECRET_LENGTH: usize = 16;

/// True if `value` is missing, a known placeholder, or too short to be a
/// real signing key. Verbatim port of `api/config.py::is_weak_secret`.
pub fn is_weak_secret(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return true;
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return true;
    }
    let normalized = trimmed.to_lowercase();
    if KNOWN_WEAK_SECRETS.contains(&normalized.as_str()) {
        return true;
    }
    if trimmed.chars().count() < MIN_SECRET_LENGTH {
        return true;
    }
    false
}

/// Parse common truthy strings: `1`, `true`, `yes`, `on` (case-insensitive).
/// Port of `api/utils/is_truthy.py`. `None` and empty strings are false.
pub fn is_truthy(value: Option<&str>) -> bool {
    match value {
        None => false,
        Some(v) => matches!(
            v.trim().to_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
    }
}

/// Return the value only if it is an absolute http(s) URL, else `None`.
/// Port of `api/config.py::_safe_http_url`: values from env flow into
/// `<a href>` on the login page; anything with another scheme
/// (`file:`, `javascript:`, ...) must never reach the browser.
fn safe_http_url(raw: Option<&str>) -> Option<String> {
    let value = raw.unwrap_or("").trim();
    if value.is_empty() {
        return None;
    }
    if let Some((scheme, rest)) = value.split_once("://") {
        let scheme = scheme.to_lowercase();
        // netloc: everything up to the first path/query/fragment delimiter.
        let netloc = rest.split(['/', '?', '#']).next().unwrap_or("");
        if (scheme == "http" || scheme == "https") && !netloc.is_empty() {
            return Some(value.to_string());
        }
    }
    tracing::warn!(url = %value, "Ignoring configured URL with unsupported scheme");
    None
}

/// `os.getenv(...) or None` semantics: unset and empty both become `None`.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.is_empty())
}

/// Typed runtime configuration. Union of `api/config.py`'s `Config`
/// classes and `ConfigData`, resolved once at startup. Only use these
/// values on the server; never expose secrets to the client.
#[derive(Debug, Clone)]
pub struct Config {
    /// Selected via `APP_ENV` (legacy fallback `RAILWAY_ENVIRONMENT`).
    pub app_env: AppEnv,
    /// `PORT`, default 5000; unparseable values fall back to 5000.
    pub port: u16,
    /// `DATABASE_PATH`. Defaults per env: development `data/nightlio.db`
    /// (repo-root relative), production `/tmp/nightlio.db`
    /// (`ProductionConfig` fallback), testing always `/tmp/nightlio_test.db`
    /// (the Python `TestingConfig` hardcodes it, ignoring the env var).
    pub database_path: String,
    /// Raw `DATABASE_URL` (unset and empty both become `None`, meaning the
    /// SQLite backend via `database_path`). Validated by
    /// [`Config::database_target`] — startup fails fast on any scheme other
    /// than `postgres://` / `postgresql://`.
    pub database_url: Option<String>,
    /// `RATE_LIMIT_DB_PATH`; when unset, `rate_limit.db` next to the raw
    /// `DATABASE_PATH` env value or the documented default data directory
    /// (mirrors `api/utils/rate_limiter.py::_rate_limit_db_path`).
    pub rate_limit_db_path: String,
    /// `SECRET_KEY`, dev fallback as in the Python `Config`.
    pub secret_key: String,
    /// `JWT_SECRET` with fallback chain `JWT_SECRET` → `JWT_SECRET_KEY`
    /// (legacy alias) → `SECRET_KEY` → dev fallback.
    pub jwt_secret: String,
    /// Fixed 3600 seconds ([`JWT_ACCESS_TOKEN_EXPIRES_SECS`]).
    pub jwt_access_token_expires_secs: u64,
    /// `CORS_ORIGINS`, comma-split, no trimming (matches Python `.split(",")`).
    pub cors_origins: Vec<String>,
    /// `TRUST_PROXY_HEADERS`: honor `X-Forwarded-*` behind a trusted proxy.
    pub trust_proxy_headers: bool,
    /// `ENABLE_MOOD_MUSIC` feature flag.
    pub enable_mood_music: bool,
    /// `JAMENDO_CLIENT_ID` (mood-music service, `api/services/mus_service.py`).
    pub jamendo_client_id: Option<String>,
    /// `I18N_GITHUB_REPO`: `owner/repo` whose `lang-<code>-v<semver>`
    /// releases carry the hot-swappable language packs (v0.6.0,
    /// `api/src/i18n.rs`). Default `FayedPatel/nightlio`.
    pub i18n_github_repo: String,
    /// `I18N_GITHUB_API_BASE`: GitHub API base URL for release discovery,
    /// default `https://api.github.com`. Tests point wiremock here.
    pub i18n_github_api_base: String,
    /// `I18N_REFRESH_SECS`: TTL between release-discovery refreshes,
    /// default 3600; unparseable values fall back to the default.
    pub i18n_refresh_secs: u64,
    /// `I18N_OFFLINE`: never contact GitHub; serve only whatever the disk
    /// cache already holds (air-gapped self-hosters).
    pub i18n_offline: bool,
    /// `I18N_LOCAL_DIR`: directory of `<code>.json` pack files served
    /// directly from disk. Takes precedence over GitHub entirely — no
    /// network, no TTL, files re-read per request.
    pub i18n_local_dir: Option<String>,
    /// `I18N_GITHUB_TOKEN`: optional token for the release-discovery calls
    /// (raises the unauthenticated 60/h GitHub API rate limit).
    pub i18n_github_token: Option<String>,
    /// OIDC SSO (any spec-compliant provider). Discovery document derived as
    /// `<OIDC_ISSUER_URL>/.well-known/openid-configuration`.
    pub oidc_issuer_url: Option<String>,
    pub oidc_client_id: Option<String>,
    pub oidc_client_secret: Option<String>,
    pub oidc_callback_url: Option<String>,
    /// Where the SPA lives, for the post-SSO redirect. `None` means same
    /// origin as the API.
    pub frontend_url: Option<String>,
    /// Signup/registration URL at the identity provider; scheme-validated
    /// (http/https only) before it can reach an `<a href>`.
    pub oidc_signup_url: Option<String>,
    /// Hard-disable local login entirely (SSO-only deployments).
    ///
    /// Owner-approved behavior change from the Flask default (which was
    /// always `false` when unset): when `DISABLE_LOCAL_LOGIN` is unset or
    /// empty, this defaults to whether OIDC is configured — an SSO-enabled
    /// deployment delegates user management to the identity provider, so
    /// local login defaults OFF. An explicit value always wins in both
    /// directions: `DISABLE_LOCAL_LOGIN=0` re-enables local login even
    /// with SSO, `DISABLE_LOCAL_LOGIN=1` disables it without SSO.
    pub disable_local_login: bool,
    /// `DEFAULT_SELF_HOST_ID`, default `selfhost_default_user`.
    pub default_self_host_id: String,
    /// `SELFHOST_USER_NAME`, default `Me` (as in `_load_config_from_env`).
    pub selfhost_user_name: String,
    /// `SELFHOST_USER_EMAIL`, optional.
    pub selfhost_user_email: Option<String>,
}

impl Config {
    /// Load from the process environment (call after `dotenvy::dotenv()`).
    pub fn from_env() -> Self {
        Self::from_lookup(&|key| std::env::var(key).ok())
    }

    /// Load from an arbitrary lookup. Tests pass a `HashMap`-backed closure
    /// so they never mutate the process environment (`std::env::set_var` is
    /// unsafe under parallel tests).
    pub fn from_lookup(env: &dyn Fn(&str) -> Option<String>) -> Self {
        let app_env = AppEnv::from_lookup(env);

        let port = env("PORT")
            .unwrap_or_default()
            .trim()
            .parse::<u16>()
            .unwrap_or(5000);

        let raw_database_path = non_empty(env("DATABASE_PATH"));
        let database_path = match app_env {
            // TestingConfig hardcodes this regardless of the env var.
            AppEnv::Testing => "/tmp/nightlio_test.db".to_string(),
            AppEnv::Production => raw_database_path
                .clone()
                .unwrap_or_else(|| "/tmp/nightlio.db".to_string()),
            AppEnv::Development => raw_database_path
                .clone()
                .unwrap_or_else(|| "data/nightlio.db".to_string()),
        };

        // Mirror api/utils/rate_limiter.py::_rate_limit_db_path exactly:
        // it derives from the raw DATABASE_PATH env value (or the default
        // data directory), not from the per-env resolved path above.
        let rate_limit_db_path = match non_empty(env("RATE_LIMIT_DB_PATH")) {
            Some(explicit) => explicit,
            None => {
                let base = raw_database_path
                    .clone()
                    .unwrap_or_else(|| "data/nightlio.db".to_string());
                let dir = match base.rsplit_once('/') {
                    Some((dir, _)) if !dir.is_empty() => dir,
                    Some(_) => "/",
                    None => ".",
                };
                format!("{}/rate_limit.db", dir.trim_end_matches('/'))
            }
        };

        let secret_key = non_empty(env("SECRET_KEY")).unwrap_or_else(|| DEV_SECRET_KEY.to_string());
        // Fallback chain from api/config.py::_load_config_from_env, with
        // JWT_SECRET_KEY kept as the legacy alias.
        let jwt_secret = non_empty(env("JWT_SECRET"))
            .or_else(|| non_empty(env("JWT_SECRET_KEY")))
            .or_else(|| non_empty(env("SECRET_KEY")))
            .unwrap_or_else(|| DEV_SECRET_KEY.to_string());

        let cors_origins = env("CORS_ORIGINS")
            .unwrap_or_else(|| DEFAULT_CORS_ORIGINS.to_string())
            .split(',')
            .map(str::to_string)
            .collect();

        let oidc_issuer_url = non_empty(env("OIDC_ISSUER_URL"));
        // See the `disable_local_login` field docs: unset/empty defaults to
        // "OIDC configured" (same predicate as `oidc_enabled()`); any
        // explicit non-empty value is parsed with `is_truthy` and wins.
        let disable_local_login = match non_empty(env("DISABLE_LOCAL_LOGIN")) {
            Some(explicit) => is_truthy(Some(&explicit)),
            None => oidc_issuer_url
                .as_deref()
                .is_some_and(|url| !url.trim().is_empty()),
        };

        Config {
            app_env,
            port,
            database_path,
            database_url: non_empty(env("DATABASE_URL")),
            rate_limit_db_path,
            secret_key,
            jwt_secret,
            jwt_access_token_expires_secs: JWT_ACCESS_TOKEN_EXPIRES_SECS,
            cors_origins,
            trust_proxy_headers: is_truthy(env("TRUST_PROXY_HEADERS").as_deref()),
            enable_mood_music: is_truthy(env("ENABLE_MOOD_MUSIC").as_deref()),
            jamendo_client_id: non_empty(env("JAMENDO_CLIENT_ID")),
            i18n_github_repo: non_empty(env("I18N_GITHUB_REPO"))
                .unwrap_or_else(|| DEFAULT_I18N_GITHUB_REPO.to_string()),
            i18n_github_api_base: non_empty(env("I18N_GITHUB_API_BASE"))
                .unwrap_or_else(|| DEFAULT_I18N_GITHUB_API_BASE.to_string()),
            i18n_refresh_secs: env("I18N_REFRESH_SECS")
                .unwrap_or_default()
                .trim()
                .parse::<u64>()
                .unwrap_or(DEFAULT_I18N_REFRESH_SECS),
            i18n_offline: is_truthy(env("I18N_OFFLINE").as_deref()),
            i18n_local_dir: non_empty(env("I18N_LOCAL_DIR")),
            i18n_github_token: non_empty(env("I18N_GITHUB_TOKEN")),
            oidc_issuer_url,
            oidc_client_id: non_empty(env("OIDC_CLIENT_ID")),
            oidc_client_secret: non_empty(env("OIDC_CLIENT_SECRET")),
            oidc_callback_url: non_empty(env("OIDC_CALLBACK_URL")),
            frontend_url: non_empty(env("FRONTEND_URL")),
            oidc_signup_url: safe_http_url(env("OIDC_SIGNUP_URL").as_deref()),
            disable_local_login,
            default_self_host_id: non_empty(env("DEFAULT_SELF_HOST_ID"))
                .unwrap_or_else(|| "selfhost_default_user".to_string()),
            selfhost_user_name: non_empty(env("SELFHOST_USER_NAME"))
                .unwrap_or_else(|| "Me".to_string()),
            selfhost_user_email: non_empty(env("SELFHOST_USER_EMAIL")),
        }
    }

    /// Resolve which database backend `DATABASE_URL` selects. Called once
    /// at startup: unset ⇒ SQLite (byte-for-byte today's behavior);
    /// `postgres://` / `postgresql://` ⇒ Postgres; anything else refuses to
    /// start rather than silently falling back to SQLite. Error messages
    /// deliberately never echo the full URL (it can embed credentials).
    pub fn database_target(&self) -> anyhow::Result<DatabaseTarget> {
        let Some(url) = self.database_url.as_deref() else {
            return Ok(DatabaseTarget::Sqlite);
        };
        match url.split_once("://") {
            Some(("postgres" | "postgresql", _)) => Ok(DatabaseTarget::Postgres(url.to_string())),
            Some((scheme, _)) => bail!(
                "Refusing to start: DATABASE_URL has unsupported scheme `{scheme}://` \
                 (only postgres:// and postgresql:// are supported). Unset \
                 DATABASE_URL to use the default SQLite backend via DATABASE_PATH."
            ),
            None => bail!(
                "Refusing to start: DATABASE_URL is set but is not a URL (expected \
                 postgres://... or postgresql://...). Unset DATABASE_URL to use the \
                 default SQLite backend via DATABASE_PATH."
            ),
        }
    }

    /// OIDC SSO is considered configured when an issuer URL is set
    /// (`ConfigData.oidc_enabled`).
    pub fn oidc_enabled(&self) -> bool {
        self.oidc_issuer_url
            .as_deref()
            .is_some_and(|url| !url.trim().is_empty())
    }

    /// Fail closed in production rather than silently signing every JWT with
    /// a missing/placeholder/short key. Verbatim port of the check in
    /// `create_app()` (`api/app.py`): scoped to production only — dev and
    /// testing keep working with no secret set at all — and both keys are
    /// checked independently; both must be strong, not just one.
    ///
    /// The error message must keep the substring `SECRET_KEY/JWT_SECRET`:
    /// `api/docker_start.py` greps for it to distinguish this refusal from
    /// other startup failures.
    pub fn validate_production_secrets(&self) -> anyhow::Result<()> {
        if self.app_env != AppEnv::Production {
            return Ok(());
        }
        if is_weak_secret(Some(&self.secret_key)) || is_weak_secret(Some(&self.jwt_secret)) {
            bail!(
                "Refusing to start: SECRET_KEY/JWT_SECRET is missing, a known \
                 placeholder, or shorter than 16 characters. Set SECRET_KEY \
                 and JWT_SECRET in your .env to distinct, random values, e.g.: \
                 openssl rand -hex 32"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::collections::HashMap;

    /// Build a lookup over a fixed map so tests never touch process env.
    fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    const STRONG: &str = "9f2c6e1a4b7d3f0158e6c2a9d7b4f103";

    #[test]
    fn defaults_with_empty_environment() {
        let cfg = Config::from_lookup(&lookup(&[]));
        // Serving entrypoints default to production.
        assert_eq!(cfg.app_env, AppEnv::Production);
        assert_eq!(cfg.port, 5000);
        assert_eq!(cfg.database_path, "/tmp/nightlio.db");
        assert_eq!(cfg.database_url, None);
        assert_eq!(cfg.database_target().unwrap(), DatabaseTarget::Sqlite);
        assert_eq!(cfg.rate_limit_db_path, "data/rate_limit.db");
        assert_eq!(cfg.secret_key, "dev-secret-key-change-in-production");
        assert_eq!(cfg.jwt_secret, "dev-secret-key-change-in-production");
        assert_eq!(cfg.jwt_access_token_expires_secs, 3600);
        assert_eq!(
            cfg.cors_origins,
            vec!["http://localhost:5173", "http://localhost:5000"]
        );
        assert!(!cfg.trust_proxy_headers);
        assert!(!cfg.enable_mood_music);
        assert!(!cfg.disable_local_login);
        assert!(!cfg.oidc_enabled());
        assert_eq!(cfg.jamendo_client_id, None);
        assert_eq!(cfg.i18n_github_repo, "FayedPatel/nightlio");
        assert_eq!(cfg.i18n_github_api_base, "https://api.github.com");
        assert_eq!(cfg.i18n_refresh_secs, 3600);
        assert!(!cfg.i18n_offline);
        assert_eq!(cfg.i18n_local_dir, None);
        assert_eq!(cfg.i18n_github_token, None);
        assert_eq!(cfg.oidc_signup_url, None);
        assert_eq!(cfg.frontend_url, None);
        assert_eq!(cfg.default_self_host_id, "selfhost_default_user");
        assert_eq!(cfg.selfhost_user_name, "Me");
        assert_eq!(cfg.selfhost_user_email, None);
    }

    #[rstest]
    #[case("development", AppEnv::Development)]
    #[case("production", AppEnv::Production)]
    #[case("testing", AppEnv::Testing)]
    #[case("something-else", AppEnv::Development)] // config["default"]
    fn app_env_selection(#[case] name: &str, #[case] expected: AppEnv) {
        let cfg = Config::from_lookup(&lookup(&[("APP_ENV", name)]));
        assert_eq!(cfg.app_env, expected);
    }

    #[test]
    fn railway_environment_is_the_legacy_fallback() {
        let cfg = Config::from_lookup(&lookup(&[("RAILWAY_ENVIRONMENT", "development")]));
        assert_eq!(cfg.app_env, AppEnv::Development);
        // APP_ENV wins when both are set.
        let cfg = Config::from_lookup(&lookup(&[
            ("APP_ENV", "testing"),
            ("RAILWAY_ENVIRONMENT", "production"),
        ]));
        assert_eq!(cfg.app_env, AppEnv::Testing);
    }

    #[rstest]
    #[case("8080", 8080)]
    #[case(" 8080 ", 8080)] // Python int() strips whitespace
    #[case("not-a-number", 5000)]
    #[case("", 5000)]
    fn port_parsing(#[case] raw: &str, #[case] expected: u16) {
        let cfg = Config::from_lookup(&lookup(&[("PORT", raw)]));
        assert_eq!(cfg.port, expected);
    }

    #[test]
    fn database_path_per_environment() {
        // Explicit env var wins in development and production.
        for env_name in ["development", "production"] {
            let cfg = Config::from_lookup(&lookup(&[
                ("APP_ENV", env_name),
                ("DATABASE_PATH", "/srv/custom.db"),
            ]));
            assert_eq!(cfg.database_path, "/srv/custom.db");
        }
        // Development default.
        let cfg = Config::from_lookup(&lookup(&[("APP_ENV", "development")]));
        assert_eq!(cfg.database_path, "data/nightlio.db");
        // ProductionConfig falls back to /tmp/nightlio.db.
        let cfg = Config::from_lookup(&lookup(&[("APP_ENV", "production")]));
        assert_eq!(cfg.database_path, "/tmp/nightlio.db");
        // TestingConfig hardcodes its path, ignoring the env var.
        let cfg = Config::from_lookup(&lookup(&[
            ("APP_ENV", "testing"),
            ("DATABASE_PATH", "/srv/custom.db"),
        ]));
        assert_eq!(cfg.database_path, "/tmp/nightlio_test.db");
    }

    #[test]
    fn database_target_defaults_to_sqlite_when_unset_or_empty() {
        // Unset -> SQLite, DATABASE_PATH semantics untouched.
        let cfg = Config::from_lookup(&lookup(&[("DATABASE_PATH", "/srv/custom.db")]));
        assert_eq!(cfg.database_url, None);
        assert_eq!(cfg.database_target().unwrap(), DatabaseTarget::Sqlite);
        assert_eq!(cfg.database_path, "/srv/custom.db");
        // Empty = unset (os.getenv-or-None convention).
        let cfg = Config::from_lookup(&lookup(&[("DATABASE_URL", "")]));
        assert_eq!(cfg.database_url, None);
        assert_eq!(cfg.database_target().unwrap(), DatabaseTarget::Sqlite);
    }

    #[rstest]
    #[case("postgres://night:secret@db.example.com:5432/nightlio")]
    #[case("postgresql://night:secret@db.example.com:5432/nightlio")]
    fn database_target_accepts_postgres_schemes(#[case] url: &str) {
        let cfg = Config::from_lookup(&lookup(&[("DATABASE_URL", url)]));
        assert_eq!(cfg.database_url.as_deref(), Some(url));
        assert_eq!(
            cfg.database_target().unwrap(),
            DatabaseTarget::Postgres(url.to_string())
        );
        // The rate limiter stays its own SQLite file on both backends.
        assert_eq!(cfg.rate_limit_db_path, "data/rate_limit.db");
    }

    #[rstest]
    #[case("mysql://night:secret@db.example.com/nightlio")]
    #[case("sqlite:///tmp/nightlio.db")]
    #[case("file:///tmp/nightlio.db")]
    fn database_target_refuses_other_schemes(#[case] url: &str) {
        let cfg = Config::from_lookup(&lookup(&[("DATABASE_URL", url)]));
        let err = cfg.database_target().unwrap_err().to_string();
        assert!(err.contains("Refusing to start"), "{err}");
        assert!(err.contains("DATABASE_URL"), "{err}");
        assert!(err.contains("postgres://"), "{err}");
        // Never echo the full URL — it can embed credentials.
        assert!(!err.contains("secret"), "{err}");
    }

    #[test]
    fn database_target_refuses_non_url_values() {
        let cfg = Config::from_lookup(&lookup(&[("DATABASE_URL", "not-a-url")]));
        let err = cfg.database_target().unwrap_err().to_string();
        assert!(err.contains("Refusing to start"), "{err}");
        assert!(err.contains("not a URL"), "{err}");
    }

    #[test]
    fn rate_limit_db_path_resolution() {
        // Explicit override wins.
        let cfg = Config::from_lookup(&lookup(&[("RATE_LIMIT_DB_PATH", "/tmp/rl.db")]));
        assert_eq!(cfg.rate_limit_db_path, "/tmp/rl.db");
        // Otherwise next to the raw DATABASE_PATH env value.
        let cfg = Config::from_lookup(&lookup(&[("DATABASE_PATH", "/srv/night/db.sqlite")]));
        assert_eq!(cfg.rate_limit_db_path, "/srv/night/rate_limit.db");
        // Bare filename → current directory (Python dirname() -> "").
        let cfg = Config::from_lookup(&lookup(&[("DATABASE_PATH", "db.sqlite")]));
        assert_eq!(cfg.rate_limit_db_path, "./rate_limit.db");
    }

    #[test]
    fn jwt_secret_fallback_chain() {
        // JWT_SECRET wins over everything.
        let cfg = Config::from_lookup(&lookup(&[
            ("JWT_SECRET", "aaaa"),
            ("JWT_SECRET_KEY", "bbbb"),
            ("SECRET_KEY", "cccc"),
        ]));
        assert_eq!(cfg.jwt_secret, "aaaa");
        // Legacy alias JWT_SECRET_KEY next.
        let cfg = Config::from_lookup(&lookup(&[
            ("JWT_SECRET_KEY", "bbbb"),
            ("SECRET_KEY", "cccc"),
        ]));
        assert_eq!(cfg.jwt_secret, "bbbb");
        // Then SECRET_KEY.
        let cfg = Config::from_lookup(&lookup(&[("SECRET_KEY", "cccc")]));
        assert_eq!(cfg.jwt_secret, "cccc");
        assert_eq!(cfg.secret_key, "cccc");
    }

    #[test]
    fn cors_origins_comma_split_without_trimming() {
        let cfg = Config::from_lookup(&lookup(&[(
            "CORS_ORIGINS",
            "http://a.example, http://b.example",
        )]));
        // Python's .split(",") does not trim; neither do we.
        assert_eq!(
            cfg.cors_origins,
            vec!["http://a.example", " http://b.example"]
        );
    }

    #[rstest]
    #[case(Some("1"), true)]
    #[case(Some("true"), true)]
    #[case(Some("Yes"), true)]
    #[case(Some("ON"), true)]
    #[case(Some("0"), false)]
    #[case(Some("false"), false)]
    #[case(Some(""), false)]
    #[case(None, false)]
    fn truthy_parsing(#[case] raw: Option<&str>, #[case] expected: bool) {
        assert_eq!(is_truthy(raw), expected);
    }

    /// Owner-approved behavior change: `DISABLE_LOCAL_LOGIN` defaults to
    /// "OIDC configured" when unset/empty; an explicit value always wins in
    /// both directions. Matrix over (OIDC on/off) x (flag unset/""/0/1).
    #[rstest]
    // No OIDC: unset -> local login stays on (unchanged from Flask).
    #[case(None, None, false)]
    #[case(None, Some(""), false)] // empty = unset (os.getenv-or-None)
    #[case(None, Some("0"), false)]
    #[case(None, Some("1"), true)] // SSO-only without SSO: explicit wins
    // OIDC configured: unset -> local login defaults OFF.
    #[case(Some("https://id.example.com"), None, true)]
    #[case(Some("https://id.example.com"), Some(""), true)]
    // Explicit 0 re-enables local login even with SSO.
    #[case(Some("https://id.example.com"), Some("0"), false)]
    #[case(Some("https://id.example.com"), Some("1"), true)]
    // Blank / whitespace-only issuer is not "configured" — the default
    // must use the same predicate as `oidc_enabled()`.
    #[case(Some(""), None, false)]
    #[case(Some("   "), None, false)]
    fn disable_local_login_default_tracks_oidc(
        #[case] issuer: Option<&str>,
        #[case] flag: Option<&str>,
        #[case] expected: bool,
    ) {
        let mut vars: Vec<(&str, &str)> = Vec::new();
        if let Some(issuer) = issuer {
            vars.push(("OIDC_ISSUER_URL", issuer));
        }
        if let Some(flag) = flag {
            vars.push(("DISABLE_LOCAL_LOGIN", flag));
        }
        let cfg = Config::from_lookup(&lookup(&vars));
        assert_eq!(cfg.disable_local_login, expected);
        // Sanity: the default tracks oidc_enabled() exactly when unset/empty.
        if flag.is_none() || flag == Some("") {
            assert_eq!(cfg.disable_local_login, cfg.oidc_enabled());
        }
    }

    #[test]
    fn oidc_enabled_derived_from_issuer() {
        let cfg = Config::from_lookup(&lookup(&[("OIDC_ISSUER_URL", "https://id.example.com")]));
        assert!(cfg.oidc_enabled());
        assert_eq!(
            cfg.oidc_issuer_url.as_deref(),
            Some("https://id.example.com")
        );
        let cfg = Config::from_lookup(&lookup(&[("OIDC_ISSUER_URL", "")]));
        assert!(!cfg.oidc_enabled());
    }

    #[rstest]
    #[case(
        Some("https://id.example.com/signup"),
        Some("https://id.example.com/signup")
    )]
    #[case(
        Some("http://id.example.com/signup"),
        Some("http://id.example.com/signup")
    )]
    #[case(Some("  https://id.example.com  "), Some("https://id.example.com"))]
    #[case(Some("javascript:alert(1)"), None)]
    #[case(Some("file:///etc/passwd"), None)]
    #[case(Some("not-a-url"), None)]
    #[case(Some("http://"), None)] // empty netloc
    #[case(Some(""), None)]
    #[case(None, None)]
    fn signup_url_scheme_validation(#[case] raw: Option<&str>, #[case] expected: Option<&str>) {
        assert_eq!(safe_http_url(raw).as_deref(), expected);
    }

    #[test]
    fn is_weak_secret_rejects_missing_and_blank() {
        assert!(is_weak_secret(None));
        assert!(is_weak_secret(Some("")));
        assert!(is_weak_secret(Some("   ")));
    }

    #[test]
    fn is_weak_secret_rejects_known_placeholders_case_insensitively() {
        for placeholder in KNOWN_WEAK_SECRETS {
            assert!(is_weak_secret(Some(placeholder)), "{placeholder}");
            assert!(
                is_weak_secret(Some(&placeholder.to_uppercase())),
                "{placeholder}"
            );
            assert!(
                is_weak_secret(Some(&format!("  {placeholder}  "))),
                "{placeholder}"
            );
        }
    }

    #[test]
    fn is_weak_secret_rejects_short_values() {
        assert!(is_weak_secret(Some(&"a".repeat(15))));
        assert!(!is_weak_secret(Some(&"a".repeat(16))));
    }

    #[test]
    fn is_weak_secret_accepts_strong_random_value() {
        assert!(!is_weak_secret(Some(STRONG)));
    }

    #[test]
    fn production_refuses_weak_secrets_with_greppable_message() {
        // No secrets at all -> dev fallbacks -> refuse.
        let cfg = Config::from_lookup(&lookup(&[("APP_ENV", "production")]));
        let err = cfg.validate_production_secrets().unwrap_err().to_string();
        // api/docker_start.py greps for this exact substring.
        assert!(err.contains("SECRET_KEY/JWT_SECRET"), "{err}");
        assert!(err.contains("SECRET_KEY"), "{err}");
        assert!(err.contains("JWT_SECRET"), "{err}");

        // Both keys must be strong, not just one.
        let cfg = Config::from_lookup(&lookup(&[
            ("APP_ENV", "production"),
            ("JWT_SECRET", STRONG),
            // SECRET_KEY unset -> dev fallback -> still weak.
        ]));
        assert!(cfg.validate_production_secrets().is_err());
        let cfg = Config::from_lookup(&lookup(&[
            ("APP_ENV", "production"),
            ("SECRET_KEY", STRONG),
            ("JWT_SECRET", "changeme"),
        ]));
        assert!(cfg.validate_production_secrets().is_err());
    }

    #[test]
    fn production_accepts_strong_secrets() {
        let cfg = Config::from_lookup(&lookup(&[
            ("APP_ENV", "production"),
            ("SECRET_KEY", STRONG),
            ("JWT_SECRET", "0011223344556677889900aabbccddee"),
        ]));
        assert!(cfg.validate_production_secrets().is_ok());
    }

    #[test]
    fn dev_and_testing_skip_the_secret_check() {
        for env_name in ["development", "testing"] {
            let cfg = Config::from_lookup(&lookup(&[("APP_ENV", env_name)]));
            assert!(cfg.validate_production_secrets().is_ok(), "{env_name}");
        }
    }

    #[test]
    fn i18n_env_seams_load() {
        let cfg = Config::from_lookup(&lookup(&[
            ("I18N_GITHUB_REPO", "acme/l10n"),
            ("I18N_GITHUB_API_BASE", "http://127.0.0.1:9099"),
            ("I18N_REFRESH_SECS", "60"),
            ("I18N_OFFLINE", "yes"),
            ("I18N_LOCAL_DIR", "/srv/i18n"),
            ("I18N_GITHUB_TOKEN", "ghp_example_token_value"),
        ]));
        assert_eq!(cfg.i18n_github_repo, "acme/l10n");
        assert_eq!(cfg.i18n_github_api_base, "http://127.0.0.1:9099");
        assert_eq!(cfg.i18n_refresh_secs, 60);
        assert!(cfg.i18n_offline);
        assert_eq!(cfg.i18n_local_dir.as_deref(), Some("/srv/i18n"));
        assert_eq!(
            cfg.i18n_github_token.as_deref(),
            Some("ghp_example_token_value")
        );
    }

    #[test]
    fn i18n_empty_values_fall_back_to_defaults() {
        // Empty = unset (os.getenv-or-None convention), like every other
        // optional variable in this module.
        let cfg = Config::from_lookup(&lookup(&[
            ("I18N_GITHUB_REPO", ""),
            ("I18N_GITHUB_API_BASE", ""),
            ("I18N_OFFLINE", ""),
            ("I18N_LOCAL_DIR", ""),
            ("I18N_GITHUB_TOKEN", ""),
        ]));
        assert_eq!(cfg.i18n_github_repo, "FayedPatel/nightlio");
        assert_eq!(cfg.i18n_github_api_base, "https://api.github.com");
        assert!(!cfg.i18n_offline);
        assert_eq!(cfg.i18n_local_dir, None);
        assert_eq!(cfg.i18n_github_token, None);
    }

    #[rstest]
    #[case("60", 60)]
    #[case(" 60 ", 60)] // int()-style whitespace tolerance, like PORT
    #[case("0", 0)]
    #[case("not-a-number", 3600)]
    #[case("-5", 3600)]
    #[case("", 3600)]
    fn i18n_refresh_secs_parsing(#[case] raw: &str, #[case] expected: u64) {
        let cfg = Config::from_lookup(&lookup(&[("I18N_REFRESH_SECS", raw)]));
        assert_eq!(cfg.i18n_refresh_secs, expected);
    }

    #[test]
    fn optional_feature_vars_load() {
        let cfg = Config::from_lookup(&lookup(&[
            ("ENABLE_MOOD_MUSIC", "true"),
            ("JAMENDO_CLIENT_ID", "jamendo-123"),
            ("TRUST_PROXY_HEADERS", "1"),
            ("DISABLE_LOCAL_LOGIN", "yes"),
            ("FRONTEND_URL", "https://app.example.com"),
            ("OIDC_CLIENT_ID", "cid"),
            ("OIDC_CLIENT_SECRET", "csecret"),
            ("OIDC_CALLBACK_URL", "https://api.example.com/callback"),
            ("DEFAULT_SELF_HOST_ID", "my_user"),
            ("SELFHOST_USER_NAME", "Night Owl"),
            ("SELFHOST_USER_EMAIL", "me@example.com"),
        ]));
        assert!(cfg.enable_mood_music);
        assert_eq!(cfg.jamendo_client_id.as_deref(), Some("jamendo-123"));
        assert!(cfg.trust_proxy_headers);
        assert!(cfg.disable_local_login);
        assert_eq!(cfg.frontend_url.as_deref(), Some("https://app.example.com"));
        assert_eq!(cfg.oidc_client_id.as_deref(), Some("cid"));
        assert_eq!(cfg.oidc_client_secret.as_deref(), Some("csecret"));
        assert_eq!(
            cfg.oidc_callback_url.as_deref(),
            Some("https://api.example.com/callback")
        );
        assert_eq!(cfg.default_self_host_id, "my_user");
        assert_eq!(cfg.selfhost_user_name, "Night Owl");
        assert_eq!(cfg.selfhost_user_email.as_deref(), Some("me@example.com"));
    }
}
