//! Extras route family — preferences (`api/routes/preferences_routes.py`),
//! activity (`api/routes/activity_routes.py`), `POST /export/pdf` (from
//! `api/routes/misc_routes.py`), and `/music/vibe`
//! (`api/routes/mus_routes.py` + `api/services/mus_service.py`).
//!
//! Paths are relative to `/api` (this router is nested there).
//!
//! # PDF export — in-process renderer (`contract/DECISIONS.md` #15)
//!
//! Rendering is done in-process by the `markdown2pdf` crate (owner decision
//! superseding the earlier Python-sidecar default; output is visually
//! different from Flask's PyMuPDF but the wire contract — 400 missing
//! content, 413 over the 1 MiB UTF-8-byte cap, `application/pdf` +
//! `Content-Disposition: attachment; filename=entry_export.pdf` — is
//! unchanged). Flask's renderer-missing 501 branch is retired: the
//! renderer is compiled into the binary and cannot be absent.
//!
//! `POST /export/pdf` **requires auth** (contract change 2026-08-17): the
//! rule carried Flask's missing `@require_auth` as parity until then. The
//! success/400/413 bodies are untouched; a credential-less call now takes
//! the standard 401, and a cookie-authenticated call must satisfy the CSRF
//! predicate like every other mutation.
//!
//! # Conditional mounting — music
//!
//! The music blueprint is only registered in Flask when `ENABLE_MOOD_MUSIC`
//! is truthy. Here the `/music/vibe` rule is always registered but every
//! method (including OPTIONS) short-circuits to the standard JSON 404 when
//! the flag is off — byte-identical to the unregistered-blueprint 404.
//! Enabled mode proxies Jamendo with a random 0..=50 offset;
//! `JAMENDO_API_BASE` (test/verifier hook) overrides the upstream base URL.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use rusqlite::Connection;
use serde_json::{Value, json};

use super::{automatic_options, resource_not_found};
use crate::auth::extract::AuthUser;
use crate::db::{self, DatabaseError};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// `Allow` for the GET-only rules (Werkzeug auto-adds HEAD + OPTIONS;
/// ordering matches the recorded fixtures).
const GET_ALLOW: &str = "HEAD, GET, OPTIONS";

/// `Allow` for `/preferences` (fixture
/// `preferences_options_204_plain.json`).
const PREFERENCES_ALLOW: &str = "HEAD, GET, OPTIONS, PUT";

/// `Allow` for the POST-only `/export/pdf` rule (no HEAD without GET).
const EXPORT_PDF_ALLOW: &str = "OPTIONS, POST";

/// Theme ids must match `src/contexts/ThemeContext.jsx`
/// (`ALLOWED_THEMES` in `api/routes/preferences_routes.py`). Enforced ONLY
/// on PUT — reads echo whatever string is stored.
const ALLOWED_THEMES: [&str; 4] = ["default", "light", "dark", "synthwave"];

/// The PUT /preferences 400 message: `", ".join(sorted(ALLOWED_THEMES))`.
const THEME_ERROR: &str = "theme must be one of: dark, default, light, synthwave";

/// `MAX_PDF_CONTENT_SIZE` in `api/routes/misc_routes.py` — measured on
/// UTF-8 encoded bytes, never characters.
pub const MAX_PDF_CONTENT_SIZE: usize = 1024 * 1024;

/// Jamendo tracks endpoint (`api/services/mus_service.py`).
const JAMENDO_TRACKS_URL: &str = "https://api.jamendo.com/v3.0/tracks/";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/preferences",
            get(get_preferences)
                .put(update_preferences)
                .merge(automatic_options(PREFERENCES_ALLOW)),
        )
        .route(
            "/activity",
            get(get_activity).merge(automatic_options(GET_ALLOW)),
        )
        .route(
            "/export/pdf",
            post(export_pdf)
                .merge(automatic_options(EXPORT_PDF_ALLOW))
                // The 1 MiB *content* cap is enforced below with Flask's
                // exact 413 body; the transport limit only needs to be
                // comfortably above it (JSON escaping can double the wire
                // size of a max-size payload).
                .layer(DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
        // Every method in one handler so the ENABLE_MOOD_MUSIC=off case can
        // 404 OPTIONS/POST/... exactly like an unregistered Flask blueprint.
        .route("/music/vibe", any(music_vibe))
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Run a data-layer call on the blocking pool (`spawn_blocking`), checking a
/// connection out of the shared pool inside the task.
async fn run_db<T, F>(state: &AppState, func: F) -> Result<T, DatabaseError>
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> Result<T, DatabaseError> + Send + 'static,
{
    let pool = state.pool.clone();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        func(&conn)
    })
    .await
    .map_err(|exc| DatabaseError::Message(format!("Database error: blocking task failed: {exc}")))?
}

/// Flask `request.is_json`: mimetype `application/json` or an
/// `application/*+json` suffix type.
fn is_json_content_type(headers: &HeaderMap) -> bool {
    let Some(content_type) = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let mimetype = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    mimetype == "application/json"
        || (mimetype.starts_with("application/") && mimetype.ends_with("+json"))
}

/// `request.get_json(silent=True) or {}` — wrong/missing Content-Type or an
/// unparseable body silently degrade (to `Null` here; lookups on `Null`
/// return `None` just like `{}.get(...)`).
fn json_body_silent(headers: &HeaderMap, body: &[u8]) -> Value {
    if !is_json_content_type(headers) {
        return Value::Null;
    }
    serde_json::from_slice(body).unwrap_or(Value::Null)
}

/// First occurrence wins, like `request.args.get` over Werkzeug's MultiDict.
fn query_first(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if url_decode(name) == key {
            Some(url_decode(value))
        } else {
            None
        }
    })
}

/// Percent-decode a query component, `+` as space (Werkzeug query parsing).
fn url_decode(raw: &str) -> String {
    let plus_decoded = raw.replace('+', " ");
    urlencoding::decode(&plus_decoded)
        .map(|decoded| decoded.into_owned())
        .unwrap_or(plus_decoded)
}

/// Python `int(str)`: surrounding whitespace tolerated, optional sign, no
/// dot. Values past i64 return `None` (Python's unbounded int would bind
/// into SQLite and 500 — clean 400 here is the same accepted drift family
/// as `contract/DECISIONS.md` #1).
fn parse_python_int(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<i64>().ok()
}

/// The automatic-OPTIONS response (204, empty body, `Allow` — contract
/// change, previously Flask's 200), as a plain `Response` for
/// handlers that dispatch on method themselves (`/music/vibe`).
fn flask_automatic_options(allow: &'static str) -> Response {
    (
        StatusCode::NO_CONTENT,
        [
            (header::ALLOW, HeaderValue::from_static(allow)),
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
        ],
    )
        .into_response()
}

/// 405 with the standard JSON envelope (contract change). `Allow` is
/// set by hand because `/music/vibe` registers every method and dispatches
/// itself, so the router-level method-not-allowed fallback never fires here.
fn method_not_allowed(allow: &'static str) -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, HeaderValue::from_static(allow))],
        Json(json!({ "error": "Method not allowed" })),
    )
        .into_response()
}

/// App-level Flask 400 handler body (`{"error": "Bad request"}`).
fn bad_request() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": "Bad request" })),
    )
        .into_response()
}

/// App-level Flask 500 handler body.
fn internal_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "Internal server error" })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Preferences
// ---------------------------------------------------------------------------

/// `GET /api/preferences`. Fresh users have no stored theme → `null`; reads
/// echo any stored string (the enum is enforced only on PUT).
async fn get_preferences(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    let theme = run_db(&state, move |conn| {
        db::users::get_user_theme(conn, user.user_id)
    })
    .await
    .map_err(|exc| ApiError::Internal(exc.into()))?;
    Ok(Json(json!({ "theme": theme })))
}

/// `PUT /api/preferences`. Cookie-auth CSRF is enforced by the [`AuthUser`]
/// extractor before this body runs (Flask decorator order).
async fn update_preferences(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let data = json_body_silent(&headers, &body);
    // Missing key, non-string, and unknown themes all take the same 400.
    let Some(theme) = data
        .get("theme")
        .and_then(Value::as_str)
        .filter(|theme| ALLOWED_THEMES.contains(theme))
    else {
        return Err(ApiError::validation(THEME_ERROR));
    };
    let theme = theme.to_string();
    let stored = theme.clone();
    run_db(&state, move |conn| {
        db::users::set_user_theme(conn, user.user_id, &stored)
    })
    .await
    .map_err(|exc| ApiError::Internal(exc.into()))?;
    Ok(Json(json!({ "status": "success", "theme": theme })))
}

// ---------------------------------------------------------------------------
// Activity
// ---------------------------------------------------------------------------

/// `GET /api/activity` — keyset pagination on the row id (`before` =
/// previous page's `next_cursor`), `limit` clamped 1..=200 in the data
/// layer. Bad `before`/`limit` values are route-level 400s with the
/// recorded messages; any deeper failure is the Python route's catch-all
/// 500 `{"error": "Failed to load activity"}`.
async fn get_activity(
    State(state): State<AppState>,
    user: AuthUser,
    RawQuery(query): RawQuery,
) -> Response {
    let query = query.unwrap_or_default();

    let before = match query_first(&query, "before") {
        Some(raw) => match parse_python_int(&raw) {
            Some(value) => Some(value),
            None => return ApiError::validation("before must be an integer").into_response(),
        },
        None => None,
    };
    let limit = match query_first(&query, "limit") {
        Some(raw) => match parse_python_int(&raw) {
            Some(value) => value,
            None => return ApiError::validation("limit must be an integer").into_response(),
        },
        None => 50,
    };

    match run_db(&state, move |conn| {
        db::activity::get_activity_page(conn, user.user_id, before, limit)
    })
    .await
    {
        Ok(page) => Json(page).into_response(),
        Err(exc) => {
            tracing::error!(error = %exc, "Failed to load activity");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to load activity" })),
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// PDF export
// ---------------------------------------------------------------------------

/// `POST /api/export/pdf` — authenticated (contract change 2026-08-17;
/// Flask left this rule without `@require_auth`). Cookie-auth CSRF is
/// enforced by the [`AuthUser`] extractor before this body runs, exactly
/// like the sibling mutations.
///
/// Still deliberately un-rate-limited: with auth required, request-volume
/// abuse is an authenticated user's self-harm on a single-operator
/// self-hosted instance, and the 1 MiB content cap bounds the per-request
/// cost. See `SECURITY.md`.
async fn export_pdf(_user: AuthUser, headers: HeaderMap, body: Bytes) -> Response {
    export_pdf_impl(&headers, &body).await
}

/// Check order mirrors the Flask view: `get_json()` (non-silent → 400
/// `Bad request` on wrong Content-Type / unparseable JSON), then the
/// missing-content 400, then the UTF-8-byte 413. Rendering happens
/// in-process via the `markdown2pdf` crate (contract/DECISIONS.md #15 —
/// the Python sidecar was retired for it), so Flask's renderer-missing
/// 501 branch no longer exists: the renderer is compiled in.
async fn export_pdf_impl(headers: &HeaderMap, body: &[u8]) -> Response {
    if !is_json_content_type(headers) {
        return bad_request();
    }
    let Ok(data) = serde_json::from_slice::<Value>(body) else {
        return bad_request();
    };

    // `if not data or "content" not in data` — {} / null / missing key.
    let content = match data.as_object() {
        Some(object) if !object.is_empty() && object.contains_key("content") => {
            match object.get("content").and_then(Value::as_str) {
                Some(content) => content,
                // Python: `content.encode()` on a non-string →
                // AttributeError → app-level 500 handler.
                None => return internal_error(),
            }
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Content is required" })),
            )
                .into_response();
        }
    };

    // `len(content.encode("utf-8"))` — str::len is UTF-8 bytes.
    if content.len() > MAX_PDF_CONTENT_SIZE {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({
                "error": format!(
                    "Content is too large to export (max {MAX_PDF_CONTENT_SIZE} bytes)"
                )
            })),
        )
            .into_response();
    }

    // Render in-process. CPU-bound (parse + layout), so it runs on the
    // blocking pool; a render failure is the Flask renderer-crashed
    // equivalent: generic 500.
    let markdown = content.to_owned();
    let rendered = tokio::task::spawn_blocking(move || {
        markdown2pdf::parse_into_bytes(markdown, markdown2pdf::config::ConfigSource::Default, None)
    })
    .await;
    match rendered {
        Ok(Ok(pdf)) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/pdf"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=entry_export.pdf",
                ),
            ],
            pdf,
        )
            .into_response(),
        Ok(Err(exc)) => {
            tracing::error!(error = %exc, "PDF render failed");
            internal_error()
        }
        Err(exc) => {
            tracing::error!(error = %exc, "PDF render task panicked");
            internal_error()
        }
    }
}

// ---------------------------------------------------------------------------
// Music
// ---------------------------------------------------------------------------

/// Upstream base URL; `JAMENDO_API_BASE` lets tests/the verifier point at a
/// fake Jamendo without ever calling the real API.
fn jamendo_base_url() -> String {
    std::env::var("JAMENDO_API_BASE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| JAMENDO_TRACKS_URL.to_string())
}

/// `GET /api/music/vibe`. All methods route here so the feature-off case
/// can reproduce Flask's unregistered-blueprint 404 (see module docs); when
/// enabled, non-GET methods get the Flask automatic-OPTIONS / 405 regime.
async fn music_vibe(
    State(state): State<AppState>,
    method: Method,
    RawQuery(query): RawQuery,
) -> Response {
    if !state.config.enable_mood_music {
        return resource_not_found();
    }
    if method == Method::OPTIONS {
        return flask_automatic_options(GET_ALLOW);
    }
    if method != Method::GET && method != Method::HEAD {
        return method_not_allowed(GET_ALLOW);
    }

    let query = query.unwrap_or_default();
    let tag = query_first(&query, "tag").unwrap_or_else(|| "chill".to_string());
    let offset = rand::random_range(0..=50u32);
    let result = get_track_by_tag(
        &jamendo_base_url(),
        state.config.jamendo_client_id.as_deref(),
        &tag,
        offset,
    )
    .await;

    // Route passthrough: any result carrying an "error" key is a 400.
    let status = if result.get("error").is_some() {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::OK
    };
    (status, Json(result)).into_response()
}

/// Port of `MusicService.get_track_by_tag`: missing client id and any
/// upstream failure become `{"error": ...}` objects (the route maps those
/// to 400); success is the flat `{audio_url, track_name, artist}` object.
async fn get_track_by_tag(
    base_url: &str,
    client_id: Option<&str>,
    tag: &str,
    random_offset: u32,
) -> Value {
    let Some(client_id) = client_id else {
        return json!({ "error": "Jamendo Client ID not configured" });
    };
    match fetch_jamendo_track(base_url, client_id, tag, random_offset).await {
        Ok(result) => result,
        // `except Exception as e: {"error": str(e)}` — message not graded.
        Err(message) => json!({ "error": message }),
    }
}

async fn fetch_jamendo_track(
    base_url: &str,
    client_id: &str,
    tag: &str,
    random_offset: u32,
) -> Result<Value, String> {
    let client = reqwest::Client::new();
    let fetch = |offset: u32| {
        let client = client.clone();
        async move {
            client
                .get(base_url)
                .query(&[
                    ("client_id", client_id.to_string()),
                    ("limit", "1".to_string()),
                    ("offset", offset.to_string()),
                    ("fuzzytags", tag.to_string()),
                ])
                .send()
                .await
                .map_err(|exc| exc.to_string())?
                .json::<Value>()
                .await
                .map_err(|exc| exc.to_string())
        }
    };

    let mut data = fetch(random_offset).await?;
    let empty = |data: &Value| {
        data.get("results")
            .and_then(Value::as_array)
            .is_none_or(|results| results.is_empty())
    };
    // If the random-offset page is empty, try again from the beginning.
    if empty(&data) && random_offset > 0 {
        data = fetch(0).await?;
    }

    match data
        .get("results")
        .and_then(Value::as_array)
        .and_then(|results| results.first())
    {
        Some(track) => {
            // Missing keys are Python KeyErrors (str(e) == "'audio'" etc.),
            // surfaced as the 400 error passthrough.
            let audio = track.get("audio").cloned().ok_or("'audio'")?;
            let name = track.get("name").cloned().ok_or("'name'")?;
            let artist = track.get("artist_name").cloned().ok_or("'artist_name'")?;
            Ok(json!({
                "audio_url": audio,
                "track_name": name,
                "artist": artist,
            }))
        }
        None => Ok(json!({ "error": "No music found for this vibe" })),
    }
}

// ---------------------------------------------------------------------------
// Tests — insta snapshots graded against contract/fixtures/misc/*.json
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get as route_get;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::jwt;
    use crate::config::Config;
    use crate::db::SelfHostSeed;

    // -- harness ------------------------------------------------------------

    struct TestApp {
        app: Router,
        token: String,
        db_path: String,
        _dir: tempfile::TempDir,
    }

    /// Fresh bootstrapped app (user 1 = default self-host user) with the
    /// exact activity seed the fixtures were recorded against: 1 login +
    /// entry_created(1..=5) + the first_entry achievement unlock ⇒ activity
    /// rows 1..=7.
    fn make_app(extra: &[(&str, &str)]) -> TestApp {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir
            .path()
            .join("nightlio.db")
            .to_string_lossy()
            .into_owned();
        let mut vars: HashMap<String, String> = HashMap::new();
        vars.insert("APP_ENV".to_string(), "development".to_string());
        for (key, value) in extra {
            vars.insert((*key).to_string(), (*value).to_string());
        }
        let lookup = move |key: &str| vars.get(key).cloned();
        let mut cfg = Config::from_lookup(&lookup);
        cfg.database_path = db_path.clone();
        db::bootstrap(&cfg.database_path, &SelfHostSeed::from(&cfg)).expect("bootstrap");
        seed_activity(&db_path);
        let token = jwt::issue_token(&cfg.jwt_secret, 1).expect("token");
        let pool = db::open_pool(&cfg.database_path).expect("pool");
        let state = crate::state::AppState::new(cfg, pool);
        TestApp {
            app: crate::routes::build_router(state),
            token,
            db_path,
            _dir: dir,
        }
    }

    /// Fixture seed: ids 1..=7, matching the notes in
    /// `activity_get_200_last_page_no_cursor.json`.
    fn seed_activity(db_path: &str) {
        let conn = db::connect(db_path).expect("connect");
        let add = |event: &str, meta: Value| {
            db::activity::add_activity(&conn, 1, event, Some(&meta)).expect("seed")
        };
        assert_eq!(add("login", json!({"method": "selfhost"})), 1);
        assert_eq!(
            add(
                "entry_created",
                json!({"entry_id": 1, "date": "2026-08-10"})
            ),
            2
        );
        assert_eq!(
            add(
                "achievement_unlocked",
                json!({"achievement_type": "first_entry"})
            ),
            3
        );
        assert_eq!(
            add(
                "entry_created",
                json!({"entry_id": 2, "date": "2026-08-11"})
            ),
            4
        );
        assert_eq!(
            add(
                "entry_created",
                json!({"entry_id": 3, "date": "2026-08-12"})
            ),
            5
        );
        assert_eq!(
            add(
                "entry_created",
                json!({"entry_id": 4, "date": "2026-08-13"})
            ),
            6
        );
        assert_eq!(
            add(
                "entry_created",
                json!({"entry_id": 5, "date": "2026-08-14"})
            ),
            7
        );
    }

    fn fixture(name: &str) -> Value {
        let path = format!(
            "{}/../contract/fixtures/misc/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|exc| panic!("missing fixture {path}: {exc}"));
        serde_json::from_str(&raw).expect("fixture json")
    }

    async fn send(app: &Router, request: Request<Body>) -> Response {
        app.clone().oneshot(request).await.expect("infallible")
    }

    fn req(method: &str, path: &str) -> axum::http::request::Builder {
        Request::builder().method(method).uri(path)
    }

    async fn bearer(app: &TestApp, method: &str, path: &str, body: Option<Value>) -> Response {
        let mut builder =
            req(method, path).header(header::AUTHORIZATION, format!("Bearer {}", app.token));
        let body = match body {
            Some(value) => {
                builder = builder.header(header::CONTENT_TYPE, "application/json");
                Body::from(serde_json::to_vec(&value).unwrap())
            }
            None => Body::empty(),
        };
        send(&app.app, builder.body(body).unwrap()).await
    }

    async fn body_json(response: Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), 16 << 20)
            .await
            .expect("body");
        serde_json::from_slice(&bytes).expect("json body")
    }

    async fn body_bytes(response: Response) -> Vec<u8> {
        axum::body::to_bytes(response.into_body(), 16 << 20)
            .await
            .expect("body")
            .to_vec()
    }

    /// `{status, body}` value with activity timestamps normalized to
    /// `<TS>`, exactly like the recorded fixtures.
    async fn graded(response: Response) -> Value {
        let status = response.status().as_u16();
        let mut body = body_json(response).await;
        if let Some(activities) = body.get_mut("activities").and_then(Value::as_array_mut) {
            for row in activities {
                if let Some(object) = row.as_object_mut() {
                    assert!(
                        object.get("created_at").is_some_and(|ts| ts.is_string()),
                        "created_at must be populated"
                    );
                    object.insert("created_at".to_string(), json!("<TS>"));
                }
            }
        }
        json!({ "status": status, "body": body })
    }

    /// Assert response status + body against a recorded fixture AND pin an
    /// insta snapshot of the same graded value.
    async fn assert_fixture(name: &str, response: Response) {
        let value = graded(response).await;
        let recorded = fixture(&format!("{name}.json"));
        assert_eq!(
            value["status"], recorded["response"]["status"],
            "{name}: status"
        );
        assert_eq!(value["body"], recorded["response"]["body"], "{name}: body");
        insta::assert_json_snapshot!(name, value);
    }

    /// Spawn a throwaway HTTP server (fake sidecar / fake Jamendo).
    async fn spawn_server(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        format!("http://{addr}/")
    }

    // -- preferences --------------------------------------------------------

    #[tokio::test]
    async fn preferences_get_200_default() {
        let app = make_app(&[]);
        let response = bearer(&app, "GET", "/api/preferences", None).await;
        assert_fixture("preferences_get_200_default", response).await;
    }

    #[tokio::test]
    async fn preferences_put_200_then_get_200_after_put() {
        let app = make_app(&[]);
        let response = bearer(
            &app,
            "PUT",
            "/api/preferences",
            Some(json!({"theme": "synthwave"})),
        )
        .await;
        assert_fixture("preferences_put_200", response).await;

        let response = bearer(&app, "GET", "/api/preferences", None).await;
        assert_fixture("preferences_get_200_after_put", response).await;
    }

    #[tokio::test]
    async fn preferences_put_400_invalid_theme() {
        let app = make_app(&[]);
        let response = bearer(
            &app,
            "PUT",
            "/api/preferences",
            Some(json!({"theme": "hotdog-stand"})),
        )
        .await;
        assert_fixture("preferences_put_400_invalid_theme", response).await;

        // Same 400 for a missing theme key, a non-object body, and a
        // non-string theme (fixture note).
        for body in [json!({}), json!([1, 2]), json!({"theme": 7})] {
            let response = bearer(&app, "PUT", "/api/preferences", Some(body.clone())).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(
                body_json(response).await,
                json!({ "error": THEME_ERROR }),
                "{body}"
            );
        }
    }

    #[tokio::test]
    async fn preferences_401_without_credentials() {
        let app = make_app(&[]);
        let response = send(
            &app.app,
            req("GET", "/api/preferences").body(Body::empty()).unwrap(),
        )
        .await;
        assert_fixture("preferences_get_401", response).await;

        let response = send(
            &app.app,
            req("PUT", "/api/preferences")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"theme": "dark"}"#))
                .unwrap(),
        )
        .await;
        assert_fixture("preferences_put_401", response).await;
    }

    #[tokio::test]
    async fn preferences_put_403_cookie_missing_csrf_header() {
        let app = make_app(&[]);
        let response = send(
            &app.app,
            req("PUT", "/api/preferences")
                .header(header::COOKIE, format!("nightlio_token={}", app.token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"theme": "dark"}"#))
                .unwrap(),
        )
        .await;
        assert_fixture("preferences_put_403_cookie_missing_csrf_header", response).await;
    }

    #[tokio::test]
    async fn preferences_options_204_plain() {
        let app = make_app(&[]);
        let response = send(
            &app.app,
            req("OPTIONS", "/api/preferences")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        // contract change: all OPTIONS answer 204-empty with Allow.
        // No Content-Length assertion: hyper strips it from 204s at
        // serialization, but axum stamps it on the in-process response.
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let recorded = fixture("preferences_options_204_plain.json");
        assert_eq!(
            response
                .headers()
                .get(header::ALLOW)
                .and_then(|value| value.to_str().ok()),
            recorded["response"]["headers"]["Allow"].as_str()
        );
        assert_eq!(body_bytes(response).await, b"");
    }

    /// Enum is enforced ONLY on PUT: reads echo any stored string.
    #[tokio::test]
    async fn preferences_get_echoes_any_stored_string() {
        let app = make_app(&[]);
        {
            let conn = db::connect(&app.db_path).unwrap();
            conn.execute(
                "UPDATE users SET theme_preference = 'hotdog-stand' WHERE id = 1",
                [],
            )
            .unwrap();
        }
        let response = bearer(&app, "GET", "/api/preferences", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_json(response).await,
            json!({ "theme": "hotdog-stand" })
        );
    }

    // -- activity -----------------------------------------------------------

    #[tokio::test]
    async fn activity_get_200_first_page() {
        let app = make_app(&[]);
        let response = bearer(&app, "GET", "/api/activity?limit=3", None).await;
        assert_fixture("activity_get_200_first_page", response).await;
    }

    #[tokio::test]
    async fn activity_get_200_second_page() {
        let app = make_app(&[]);
        let response = bearer(&app, "GET", "/api/activity?before=5&limit=3", None).await;
        assert_fixture("activity_get_200_second_page", response).await;
    }

    #[tokio::test]
    async fn activity_get_200_last_page_no_cursor() {
        let app = make_app(&[]);
        let response = bearer(&app, "GET", "/api/activity?limit=200", None).await;
        assert_fixture("activity_get_200_last_page_no_cursor", response).await;
    }

    /// contract change (DECISIONS.md post-cutover item #10): a full
    /// *final* page emits `next_cursor: null`. before=4&limit=3 returns
    /// exactly [3, 2, 1] — page full, nothing older — so the cursor is null
    /// (Flask returned 1 here, forcing a wasted empty-page follow-up).
    #[tokio::test]
    async fn activity_get_200_full_final_page() {
        let app = make_app(&[]);
        let response = bearer(&app, "GET", "/api/activity?before=4&limit=3", None).await;
        assert_fixture("activity_get_200_full_final_page", response).await;
    }

    #[tokio::test]
    async fn activity_get_200_empty_page() {
        let app = make_app(&[]);
        let response = bearer(&app, "GET", "/api/activity?before=1", None).await;
        assert_fixture("activity_get_200_empty_page", response).await;
    }

    #[tokio::test]
    async fn activity_get_200_limit_clamped_high() {
        let app = make_app(&[]);
        let response = bearer(&app, "GET", "/api/activity?limit=9999", None).await;
        assert_fixture("activity_get_200_limit_clamped_high", response).await;
    }

    #[tokio::test]
    async fn activity_get_200_limit_clamped_low() {
        let app = make_app(&[]);
        let response = bearer(&app, "GET", "/api/activity?limit=0", None).await;
        assert_fixture("activity_get_200_limit_clamped_low", response).await;

        // Fixture note: negative limits behave the same.
        let response = bearer(&app, "GET", "/api/activity?limit=-5", None).await;
        let value = graded(response).await;
        assert_eq!(
            value["body"],
            fixture("activity_get_200_limit_clamped_low.json")["response"]["body"]
        );
    }

    #[tokio::test]
    async fn activity_get_400_bad_params() {
        let app = make_app(&[]);
        let response = bearer(&app, "GET", "/api/activity?before=abc", None).await;
        assert_fixture("activity_get_400_bad_before", response).await;

        let response = bearer(&app, "GET", "/api/activity?limit=abc", None).await;
        assert_fixture("activity_get_400_bad_limit", response).await;

        // Python int() accepts signs/whitespace but never a dot or empty.
        for bad in ["1.5", "", "%20"] {
            let response = bearer(&app, "GET", &format!("/api/activity?limit={bad}"), None).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{bad:?}");
        }
        let response = bearer(&app, "GET", "/api/activity?before=+7&limit=+3", None).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn activity_get_401_without_credentials() {
        let app = make_app(&[]);
        // Auth runs before param validation (decorator order): bad params
        // without credentials still 401.
        let response = send(
            &app.app,
            req("GET", "/api/activity?limit=abc")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_fixture("activity_get_401", response).await;
    }

    #[tokio::test]
    async fn activity_get_trailing_slash_404() {
        let app = make_app(&[]);
        let response = bearer(&app, "GET", "/api/activity/", None).await;
        assert_fixture("activity_get_trailing_slash_404", response).await;
    }

    #[tokio::test]
    async fn activity_default_limit_is_50() {
        let app = make_app(&[]);
        let response = bearer(&app, "GET", "/api/activity", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["activities"].as_array().unwrap().len(), 7);
        assert_eq!(body["next_cursor"], Value::Null);
    }

    // -- export/pdf ---------------------------------------------------------

    /// The body-shape tests below drive `export_pdf_impl` directly, i.e.
    /// downstream of the `AuthUser` extractor — they grade the 400/413/200
    /// bodies only. The auth requirement itself is graded by the
    /// router-level 401/403/200 tests further down.
    fn json_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers
    }

    #[tokio::test]
    async fn export_pdf_400_missing_content() {
        let response = export_pdf_impl(&json_headers(), b"{}").await;
        assert_fixture("export_pdf_post_400_missing_content", response).await;

        // Non-JSON Content-Type / unparseable body → app-level 400 handler
        // {"error": "Bad request"} (fixture note).
        let response = export_pdf_impl(&HeaderMap::new(), b"{}").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(response).await, json!({ "error": "Bad request" }));

        let response = export_pdf_impl(&json_headers(), b"{not json").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(response).await, json!({ "error": "Bad request" }));

        // Non-string content: Python AttributeError → generic 500.
        let response = export_pdf_impl(&json_headers(), br#"{"content": 7}"#).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn export_pdf_413_too_large() {
        // 'A' * 1048577 — one byte over the cap, measured on UTF-8 bytes.
        let content = "A".repeat(MAX_PDF_CONTENT_SIZE + 1);
        let body = serde_json::to_vec(&json!({ "content": content })).unwrap();
        let response = export_pdf_impl(&json_headers(), &body).await;
        assert_fixture("export_pdf_post_413_too_large", response).await;

        // Multi-byte smuggling check: 349_526 '€' (3 bytes each) exceeds the
        // cap despite being far fewer characters.
        let content = "€".repeat(MAX_PDF_CONTENT_SIZE / 3 + 1);
        let body = serde_json::to_vec(&json!({ "content": content })).unwrap();
        let response = export_pdf_impl(&json_headers(), &body).await;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    /// In-process rendering (contract/DECISIONS.md #15): real `markdown2pdf`
    /// output — a genuine PDF, correct headers, request body taken from the
    /// recorded fixture.
    #[tokio::test(flavor = "multi_thread")]
    async fn export_pdf_200_renders_in_process() {
        let recorded = fixture("export_pdf_post_200.json");
        let body = serde_json::to_vec(&recorded["request"]["body"]).unwrap();
        let response = export_pdf_impl(&json_headers(), &body).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/pdf")
        );
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_DISPOSITION)
                .and_then(|value| value.to_str().ok()),
            recorded["response"]["headers"]["Content-Disposition"].as_str()
        );
        let bytes = body_bytes(response).await;
        assert!(bytes.starts_with(b"%PDF"), "not a PDF: {:?}", &bytes[..8]);
    }

    /// Journal entries contain emoji and non-ASCII text; rendering must not
    /// error on them (visual fidelity is not graded, producing a PDF is).
    #[tokio::test(flavor = "multi_thread")]
    async fn export_pdf_200_renders_unicode_content() {
        let body = serde_json::to_vec(&json!({ "content": "# Mood 😀🌙\n\ncafé naïve — résumé" }))
            .unwrap();
        let response = export_pdf_impl(&json_headers(), &body).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body_bytes(response).await.starts_with(b"%PDF"));
    }

    /// contract change 2026-08-17: the rule requires auth. A credential-less
    /// POST takes the standard 401 (fixture `export_pdf_post_401`) before any body
    /// validation — the same ordering as every other protected rule.
    #[tokio::test]
    async fn export_pdf_post_401_without_credentials() {
        let app = make_app(&[]);
        let recorded = fixture("export_pdf_post_401.json");
        let response = send(
            &app.app,
            req("POST", "/api/export/pdf")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&recorded["request"]["body"]).unwrap(),
                ))
                .unwrap(),
        )
        .await;
        assert_fixture("export_pdf_post_401", response).await;

        // Auth runs before body validation: a body that would 400 with
        // credentials still 401s without them.
        let response = send(
            &app.app,
            req("POST", "/api/export/pdf")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            body_json(response).await,
            recorded["response"]["body"],
            "auth precedes the missing-content 400"
        );
    }

    /// Cookie-authenticated mutation without `X-Requested-With: nightlio`
    /// takes the shared CSRF 403 — the extractor enforces it before token
    /// verification, exactly like `PUT /api/preferences`.
    #[tokio::test]
    async fn export_pdf_403_cookie_missing_csrf_header() {
        let app = make_app(&[]);
        let response = send(
            &app.app,
            req("POST", "/api/export/pdf")
                .header(header::COOKIE, format!("nightlio_token={}", app.token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"content": "hello"}"#))
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            body_json(response).await,
            fixture("preferences_put_403_cookie_missing_csrf_header.json")["response"]["body"],
            "same CSRF 403 body as every other cookie-auth mutation"
        );
    }

    /// End-to-end through the router with credentials: the extractor is
    /// actually wired and a Bearer caller still gets the recorded PDF.
    #[tokio::test(flavor = "multi_thread")]
    async fn export_pdf_200_through_router_with_bearer() {
        let app = make_app(&[]);
        let recorded = fixture("export_pdf_post_200.json");
        let response = bearer(
            &app,
            "POST",
            "/api/export/pdf",
            Some(recorded["request"]["body"].clone()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_DISPOSITION)
                .and_then(|value| value.to_str().ok()),
            recorded["response"]["headers"]["Content-Disposition"].as_str()
        );
        assert!(body_bytes(response).await.starts_with(b"%PDF"));
    }

    /// Router-level regimes that sit in front of the auth extractor:
    /// OPTIONS/405/strict-slash all answer before any credential check.
    #[tokio::test]
    async fn export_pdf_route_registration() {
        let app = make_app(&[]);

        // OPTIONS: automatic 204-empty with Allow (contract change).
        let response = send(
            &app.app,
            req("OPTIONS", "/api/export/pdf")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            response
                .headers()
                .get(header::ALLOW)
                .and_then(|value| value.to_str().ok()),
            Some(EXPORT_PDF_ALLOW)
        );

        // GET → 405 with Allow and the JSON envelope (contract change).
        let response = send(
            &app.app,
            req("GET", "/api/export/pdf").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        let allow = response
            .headers()
            .get(header::ALLOW)
            .and_then(|value| value.to_str().ok())
            .expect("Allow on 405");
        assert!(allow.contains("POST"), "Allow was {allow:?}");
        assert_eq!(
            body_json(response).await,
            json!({ "error": "Method not allowed" })
        );

        // Trailing slash → strict_slashes JSON 404.
        let response = send(
            &app.app,
            req("POST", "/api/export/pdf/").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // -- music/vibe ---------------------------------------------------------

    #[tokio::test]
    async fn music_vibe_get_404_feature_disabled() {
        let app = make_app(&[]);
        let response = send(
            &app.app,
            req("GET", "/api/music/vibe").body(Body::empty()).unwrap(),
        )
        .await;
        assert_fixture("music_vibe_get_404_feature_disabled", response).await;

        // Unregistered-blueprint equivalence: EVERY method 404s, including
        // OPTIONS.
        for method in ["OPTIONS", "POST", "DELETE"] {
            let response = send(
                &app.app,
                req(method, "/api/music/vibe").body(Body::empty()).unwrap(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method}");
            assert_eq!(
                body_json(response).await,
                json!({ "error": "Resource not found" }),
                "{method}"
            );
        }
    }

    #[tokio::test]
    async fn music_vibe_enabled_regimes_without_client_id() {
        let app = make_app(&[("ENABLE_MOOD_MUSIC", "1")]);

        // No JAMENDO_CLIENT_ID → service-level error passthrough as 400,
        // no upstream call ever attempted.
        let response = send(
            &app.app,
            req("GET", "/api/music/vibe").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            body_json(response).await,
            json!({ "error": "Jamendo Client ID not configured" })
        );

        // OPTIONS: automatic-OPTIONS regime once the route exists —
        // 204-empty with Allow (contract change).
        let response = send(
            &app.app,
            req("OPTIONS", "/api/music/vibe")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            response
                .headers()
                .get(header::ALLOW)
                .and_then(|value| value.to_str().ok()),
            Some(GET_ALLOW)
        );
        assert_eq!(body_bytes(response).await, b"");

        // POST → 405 with Allow and the JSON envelope (contract change).
        let response = send(
            &app.app,
            req("POST", "/api/music/vibe").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(
            response
                .headers()
                .get(header::ALLOW)
                .and_then(|value| value.to_str().ok()),
            Some(GET_ALLOW)
        );
        assert_eq!(
            body_json(response).await,
            json!({ "error": "Method not allowed" })
        );
    }

    /// Fake Jamendo that echoes the received query into the track fields so
    /// the proxy parameters are observable.
    fn jamendo_echo() -> Router {
        Router::new().route(
            "/",
            route_get(
                |axum::extract::Query(params): axum::extract::Query<
                    HashMap<String, String>,
                >| async move {
                    assert_eq!(params.get("limit").map(String::as_str), Some("1"));
                    Json(json!({
                        "results": [{
                            "audio": "https://cdn.example/track.mp3",
                            "name": params.get("fuzzytags"),
                            "artist_name": params.get("client_id"),
                        }]
                    }))
                },
            ),
        )
    }

    #[tokio::test]
    async fn music_service_success_shape() {
        let base = spawn_server(jamendo_echo()).await;
        let result = get_track_by_tag(&base, Some("jamendo-123"), "chill", 17).await;
        assert_eq!(
            result,
            json!({
                "audio_url": "https://cdn.example/track.mp3",
                "track_name": "chill",
                "artist": "jamendo-123",
            })
        );
        insta::assert_json_snapshot!("music_vibe_enabled_success_shape", result);
    }

    #[tokio::test]
    async fn music_service_retries_from_offset_zero() {
        // Results exist only at offset 0: the random-offset miss must retry
        // from the beginning (mus_service.py behavior).
        let mock =
            Router::new().route(
                "/",
                route_get(
                    |axum::extract::Query(params): axum::extract::Query<
                        HashMap<String, String>,
                    >| async move {
                        if params.get("offset").map(String::as_str) == Some("0") {
                            Json(json!({
                                "results": [{
                                    "audio": "a", "name": "n", "artist_name": "x",
                                }]
                            }))
                        } else {
                            Json(json!({ "results": [] }))
                        }
                    },
                ),
            );
        let base = spawn_server(mock).await;
        let result = get_track_by_tag(&base, Some("cid"), "lofi", 42).await;
        assert_eq!(
            result,
            json!({ "audio_url": "a", "track_name": "n", "artist": "x" })
        );
    }

    #[tokio::test]
    async fn music_service_no_results_and_upstream_errors() {
        // Empty everywhere → the exact "No music found" error object.
        let empty =
            Router::new().route("/", route_get(|| async { Json(json!({ "results": [] })) }));
        let base = spawn_server(empty).await;
        let result = get_track_by_tag(&base, Some("cid"), "polka", 0).await;
        assert_eq!(result, json!({ "error": "No music found for this vibe" }));

        // Non-JSON upstream → error passthrough object (message not graded).
        let garbage = Router::new().route("/", route_get(|| async { "not json" }));
        let base = spawn_server(garbage).await;
        let result = get_track_by_tag(&base, Some("cid"), "polka", 0).await;
        assert!(result.get("error").is_some(), "got {result}");

        // Missing client id short-circuits without any network call.
        let result = get_track_by_tag("http://127.0.0.1:1/", None, "chill", 0).await;
        assert_eq!(
            result,
            json!({ "error": "Jamendo Client ID not configured" })
        );
    }

    // -- helper semantics ---------------------------------------------------

    #[test]
    fn python_int_parse_semantics() {
        assert_eq!(parse_python_int("5"), Some(5));
        assert_eq!(parse_python_int(" 5 "), Some(5));
        assert_eq!(parse_python_int("+5"), Some(5));
        assert_eq!(parse_python_int("-5"), Some(-5));
        for bad in ["", " ", "5.0", "abc", "0x10", "5 5"] {
            assert_eq!(parse_python_int(bad), None, "{bad:?}");
        }
        // > i64: Python's unbounded int would 500 at the SQLite bind; the
        // port 400s cleanly (accepted drift, DECISIONS.md #1 family).
        assert_eq!(parse_python_int("99999999999999999999"), None);
    }

    #[test]
    fn query_first_takes_first_occurrence_and_decodes() {
        assert_eq!(
            query_first("before=5&before=9", "before"),
            Some("5".to_string())
        );
        assert_eq!(
            query_first("tag=lo+fi%21", "tag"),
            Some("lo fi!".to_string())
        );
        assert_eq!(query_first("limit", "limit"), Some(String::new()));
        assert_eq!(query_first("a=1&b=2", "c"), None);
    }

    #[test]
    fn json_content_type_matches_flask_is_json() {
        let mut headers = HeaderMap::new();
        assert!(!is_json_content_type(&headers));
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        assert!(is_json_content_type(&headers));
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        assert!(is_json_content_type(&headers));
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/json"));
        assert!(!is_json_content_type(&headers));
    }
}
