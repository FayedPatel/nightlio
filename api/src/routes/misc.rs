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

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api", any(api_root_redirect))
        .route(
            "/api/",
            get(health_check).merge(automatic_options(GET_ALLOW)),
        )
        .route(
            "/api/time",
            get(get_current_time).merge(automatic_options(GET_ALLOW)),
        )
        .route(
            "/api/config",
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
    }))
}

/// Any-method `/api` → 308 to `/api/`, like Werkzeug's redirect during URL
/// matching (which fires before the method check, hence `any`). The
/// `Location` is absolute when a host is known — honoring ProxyFix-trusted
/// forwarded values when present — and falls back to a relative `/api/`.
async fn api_root_redirect(
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
    let location = match host {
        Some(host) => format!("{scheme}://{host}/api/"),
        None => "/api/".to_string(),
    };
    let location_value = match location.parse::<axum::http::HeaderValue>() {
        Ok(value) => value,
        Err(_) => axum::http::HeaderValue::from_static("/api/"),
    };
    // contract change: empty body — `Location` is the contract; the
    // Werkzeug HTML replica was dropped.
    (
        StatusCode::PERMANENT_REDIRECT,
        [(header::LOCATION, location_value)],
    )
        .into_response()
}
