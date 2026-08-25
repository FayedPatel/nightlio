//! Misc + config routes — port of the health/time handlers in
//! `api/routes/misc_routes.py` and `api/routes/config_routes.py`.
//! (`/export/pdf` from the same Flask blueprint lives with the [`extras`]
//! family.)
//!
//! `/api/` (the blueprint's `/` rule) is the ONLY rule in the whole app
//! registered WITH a trailing slash, so `GET /api` gets a 308 permanent
//! redirect to `/api/` — status + `Location` with an empty body (contract
//! change; Werkzeug's HTML interstitial was dropped). Every other
//! rule 404s its opposite-slash variant via the JSON fallback.
//!
//! Since 0.6.0 this router is built per mount prefix ([`router_at`]):
//! once for the canonical `/api` and once for the `/api/v1` alias, whose
//! slash pair redirects within its own prefix (`/api/v1` → `/api/v1/`).
//!
//! [`extras`]: crate::routes::extras

use axum::Extension;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::{Json, Router};
use serde_json::json;

use super::{ForwardedInfo, automatic_options};
use crate::state::AppState;

/// `Allow` value Werkzeug advertises for the GET-only rules here.
const GET_ALLOW: &str = "HEAD, GET, OPTIONS";

/// The misc family with its absolute paths built from `prefix` (`"/api"`
/// or `"/api/v1"`) — behavior at `/api` is byte-identical to the
/// pre-alias `router()`.
pub fn router_at(prefix: &'static str) -> Router<AppState> {
    Router::new()
        .route(
            prefix,
            any(
                move |forwarded: Option<Extension<ForwardedInfo>>, headers: HeaderMap| {
                    api_root_redirect(prefix, forwarded, headers)
                },
            ),
        )
        .route(
            &format!("{prefix}/"),
            get(health_check).merge(automatic_options(GET_ALLOW)),
        )
        .route(
            &format!("{prefix}/time"),
            get(get_current_time).merge(automatic_options(GET_ALLOW)),
        )
        .route(
            &format!("{prefix}/config"),
            get(get_public_config).merge(automatic_options(GET_ALLOW)),
        )
}

/// `time.time()` — float epoch seconds.
fn epoch_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64())
        .unwrap_or(0.0)
}

/// `GET /api/` — health check.
async fn health_check() -> Json<serde_json::Value> {
    Json(json!({
        "status": "healthy",
        "message": "Nightlio API is running",
        "timestamp": epoch_seconds(),
    }))
}

/// `GET /api/time`.
async fn get_current_time() -> Json<serde_json::Value> {
    Json(json!({ "time": epoch_seconds() }))
}

/// `GET /api/config` — port of `config_to_public_dict` (`api/config.py`):
/// the four public flags, secrets never included. `signup_url` is forced
/// null whenever OIDC is disabled, even if the env var is set.
///
/// `version` (added 0.6.0, contract/DECISIONS.md 2026-08-22) is the crate
/// version baked in at compile time — the deploy's source of truth, since
/// the git tag never reaches the published image (publish.yml retags
/// without rebuilding). Kept in lockstep with package.json and
/// contract/openapi.yaml by scripts/check-version-sync.sh in CI.
async fn get_public_config(State(state): State<AppState>) -> Json<serde_json::Value> {
    let config = &state.config;
    let oidc_enabled = config.oidc_enabled();
    let signup_url = if oidc_enabled {
        config.oidc_signup_url.clone()
    } else {
        None
    };
    Json(json!({
        "enable_oidc": oidc_enabled,
        "enable_mood_music": config.enable_mood_music,
        "enable_local_login": !config.disable_local_login,
        "signup_url": signup_url,
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// Any-method bare `prefix` → 308 to `{prefix}/` (`/api` → `/api/`,
/// `/api/v1` → `/api/v1/`), like Werkzeug's redirect during URL matching
/// (which fires before the method check, hence `any`). The `Location` is
/// absolute when a host is known — honoring ProxyFix-trusted forwarded
/// values when present — and falls back to the relative `{prefix}/`.
async fn api_root_redirect(
    prefix: &'static str,
    forwarded: Option<Extension<ForwardedInfo>>,
    headers: HeaderMap,
) -> Response {
    let forwarded = forwarded.map(|Extension(info)| info).unwrap_or_default();
    let scheme = forwarded.scheme.as_deref().unwrap_or("http");
    let host = forwarded.host.clone().or_else(|| {
        headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    });
    let relative = format!("{prefix}/");
    let location = match host {
        Some(host) => format!("{scheme}://{host}{prefix}/"),
        None => relative.clone(),
    };
    let location_value = location
        .parse::<axum::http::HeaderValue>()
        .or_else(|_| relative.parse())
        .unwrap_or_else(|_| axum::http::HeaderValue::from_static("/api/"));
    // contract change: empty body — `Location` is the contract; the
    // Werkzeug HTML replica was dropped.
    (
        StatusCode::PERMANENT_REDIRECT,
        [(header::LOCATION, location_value)],
    )
        .into_response()
}
