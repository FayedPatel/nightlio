//! i18n route family — the unauthenticated language-pack endpoints
//! (NEW in v0.6.0; Rust-native, this family never existed in Flask). The
//! eight hand-authored fixtures in `contract/fixtures/i18n/` plus
//! `contract/openapi-parts/i18n.yaml` are the normative contract
//! (`contract/DECISIONS.md` entry dated 2026-08-22); the tests below
//! replay every one of them against the assembled router.
//!
//! Routing notes (fixture-verified):
//! - Both rules are **unauthenticated** — they must stay reachable
//!   pre-login (the frontend fetches packs before any session exists), and
//!   no 401/403 path exists on this family.
//! - Strict-slash like every other rule: the trailing-slash variants fall
//!   through to the app-level JSON 404 (`/api/i18n/es/` does NOT fall
//!   through to the `{code}` route). POST takes the unified 405 envelope
//!   with axum's synthesized `Allow`; OPTIONS answers the automatic 204.
//! - `{code}` is an opaque string segment (no `<int:>` converter
//!   semantics) extracted via [`FlaskPath`] so any extraction failure maps
//!   to the JSON 404. The static `languages` segment wins over `{code}`,
//!   making the code "languages" unreachable — harmless, no such language
//!   code exists.
//!
//! Caching contract (the browser HTTP cache is the caching layer — the
//! service worker is NetworkOnly for `/api/*`):
//! - `GET /api/i18n/{code}` 200 carries the strong validator
//!   `ETag: "<code>-<version>"` plus `Cache-Control: public,
//!   max-age=3600`; an `If-None-Match` hit (exact string compare, quotes
//!   included) yields 304 with an empty body, no Content-Type, and the
//!   same ETag + Cache-Control.
//! - `GET /api/i18n/languages` carries Cache-Control only — the list has
//!   no single stable version identity, so it gets NO ETag (owner
//!   decision, DECISIONS.md 2026-08-22).
//! - The 404 for an uncached code is the standard app-level envelope with
//!   no caching headers, indistinguishable from any unknown path.
//!
//! Empty states are soft: nothing cached (fresh instance, `I18N_OFFLINE`,
//! or GitHub unreachable with a cold cache) means an empty language list
//! and a 404 for every code — clients run on bundled English. Upstream
//! failures never surface here; [`crate::i18n::I18nStore`] serves its
//! stale cache forever.

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use super::{FlaskPath, automatic_options, resource_not_found};
use crate::state::AppState;

/// `Allow` value for the automatic OPTIONS on both GET-only rules.
const GET_ALLOW: &str = "HEAD, GET, OPTIONS";

/// The shared caching header: one hour, publicly cacheable (contract fact,
/// both fixtures' `headers_that_matter`).
const CACHE_CONTROL_VALUE: &str = "public, max-age=3600";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/i18n/languages",
            get(list_languages).merge(automatic_options(GET_ALLOW)),
        )
        .route(
            "/i18n/{code}",
            get(get_language_pack).merge(automatic_options(GET_ALLOW)),
        )
}

/// `GET /api/i18n/languages` — the cached packs projected to
/// `{code, name, native_name, version}`, sorted by code ascending, English
/// never listed (the frontend unions bundled English into its picker).
/// Always the object envelope, never a bare array; empty when nothing is
/// cached. Cache-Control only — no ETag on this endpoint.
async fn list_languages(State(state): State<AppState>) -> Response {
    let languages = state.i18n.list_languages().await;
    (
        StatusCode::OK,
        [(
            header::CACHE_CONTROL,
            HeaderValue::from_static(CACHE_CONTROL_VALUE),
        )],
        Json(json!({ "languages": languages })),
    )
        .into_response()
}

/// `GET /api/i18n/{code}` — the cached pack envelope, or the standard 404
/// for an uncached code. English is served here even though it is never
/// listed — that is how English itself stays hot-swappable. 200s carry the
/// strong `ETag: "<code>-<version>"` + Cache-Control; an `If-None-Match`
/// exact-string hit yields the empty-body 304 with the same two headers.
async fn get_language_pack(
    State(state): State<AppState>,
    FlaskPath(code): FlaskPath<String>,
    headers: HeaderMap,
) -> Response {
    let Some(pack) = state.i18n.get_pack(&code).await else {
        return resource_not_found();
    };
    let etag = format!("\"{}-{}\"", pack.language, pack.version);
    let Ok(etag_value) = HeaderValue::from_str(&etag) else {
        // Only reachable via an I18N_LOCAL_DIR pack whose version field
        // contains header-invalid characters: such a pack cannot satisfy
        // the caching contract, so it is treated as absent.
        tracing::warn!(
            code = %pack.language,
            "i18n: pack version is not a valid ETag; treating the pack as absent"
        );
        return resource_not_found();
    };
    let cache_headers = [
        (header::ETAG, etag_value),
        (
            header::CACHE_CONTROL,
            HeaderValue::from_static(CACHE_CONTROL_VALUE),
        ),
    ];
    // Exact string compare, quotes included (contract fact,
    // pack_get_304_etag_match.json) — no weak-comparison or `*` semantics.
    let revalidated = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == etag);
    if revalidated {
        // Empty body, no Content-Type; same ETag + Cache-Control as the
        // 200 would carry.
        (StatusCode::NOT_MODIFIED, cache_headers).into_response()
    } else {
        (StatusCode::OK, cache_headers, Json(pack)).into_response()
    }
}

// ---------------------------------------------------------------------------
// Tests — every contract/fixtures/i18n/ fixture replayed against the
// assembled router; wiremock fakes the GitHub API for the seeded states.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::body::Body;
    use axum::http::Request;
    use serde_json::Value;
    use tower::ServiceExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::config::Config;
    use crate::db::{self, SelfHostSeed};

    // -- harness ------------------------------------------------------------

    struct TestApp {
        app: Router,
        _dir: tempfile::TempDir,
    }

    /// Fresh bootstrapped app over a tempdir database. The i18n env seams
    /// come in through `extra`; every test pins one of the three
    /// deterministic modes (offline, wiremock-faked GitHub, local dir) so
    /// no test ever touches the real network.
    fn make_app(extra: &[(&str, &str)]) -> TestApp {
        make_app_in(tempfile::tempdir().expect("tempdir"), extra)
    }

    /// [`make_app`] over a caller-provided tempdir (the local-dir harness
    /// keeps its pack files in the same dir as the database).
    fn make_app_in(dir: tempfile::TempDir, extra: &[(&str, &str)]) -> TestApp {
        let mut vars: HashMap<String, String> = HashMap::new();
        vars.insert("APP_ENV".to_string(), "development".to_string());
        for (key, value) in extra {
            vars.insert((*key).to_string(), (*value).to_string());
        }
        let lookup = move |key: &str| vars.get(key).cloned();
        let mut cfg = Config::from_lookup(&lookup);
        cfg.database_path = dir
            .path()
            .join("nightlio.db")
            .to_string_lossy()
            .into_owned();
        db::bootstrap(&cfg.database_path, &SelfHostSeed::from(&cfg)).expect("bootstrap");
        let pool = db::open_pool(&cfg.database_path).expect("pool");
        let state = crate::state::AppState::new(cfg, db::DbHandle::Sqlite(pool));
        TestApp {
            app: crate::routes::build_router(state),
            _dir: dir,
        }
    }

    /// The cold-cache empty state: `I18N_OFFLINE=1` guarantees the store
    /// never fetches, matching the "fresh instance / offline" seeding of
    /// the empty-state fixtures.
    fn offline_app() -> TestApp {
        make_app(&[("I18N_OFFLINE", "1")])
    }

    /// The seeded state via wiremock: a fake GitHub API serving the
    /// `lang-es-v1.0.0` + `lang-fr-v1.2.0` releases whose es asset is
    /// byte-identical to the recorded `pack_get_200` body.
    async fn github_app() -> (TestApp, MockServer) {
        let server = MockServer::start().await;
        let base = server.uri();
        Mock::given(method("GET"))
            .and(path("/repos/FayedPatel/nightlio/releases"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {
                    "tag_name": "lang-es-v1.0.0",
                    "prerelease": true,
                    "assets": [{
                        "name": "es.json",
                        "browser_download_url": format!("{base}/download/es.json"),
                    }],
                },
                {
                    "tag_name": "lang-fr-v1.2.0",
                    "prerelease": true,
                    "assets": [{
                        "name": "fr.json",
                        "browser_download_url": format!("{base}/download/fr.json"),
                    }],
                },
            ])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/download/es.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(es_pack()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/download/fr.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fr_pack()))
            .mount(&server)
            .await;
        let app = make_app(&[("I18N_GITHUB_API_BASE", &base)]);
        (app, server)
    }

    /// The seeded state via `I18N_LOCAL_DIR` (the fixtures' alternate
    /// seeding). The dir also carries an en pack — proving at the route
    /// level that English is excluded from the list yet still served —
    /// and the store never contacts GitHub (local-dir precedence).
    fn local_dir_app() -> TestApp {
        let dir = tempfile::tempdir().expect("tempdir");
        let packs = dir.path().join("packs");
        std::fs::create_dir_all(&packs).expect("packs dir");
        for (name, pack) in [
            ("es.json", es_pack()),
            ("fr.json", fr_pack()),
            ("en.json", en_pack()),
        ] {
            std::fs::write(packs.join(name), serde_json::to_vec(&pack).unwrap()).expect("pack");
        }
        let packs = packs.to_string_lossy().into_owned();
        make_app_in(dir, &[("I18N_LOCAL_DIR", &packs)])
    }

    // -- fixtures -----------------------------------------------------------

    fn fixture(name: &str) -> Value {
        let path = format!(
            "{}/../contract/fixtures/i18n/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|exc| panic!("missing fixture {path}: {exc}"));
        serde_json::from_str(&raw).expect("fixture json")
    }

    /// The recorded es pack envelope — `pack_get_200`'s body IS the pack,
    /// so seeding from it keeps the replay byte-faithful.
    fn es_pack() -> Value {
        fixture("pack_get_200.json")["response"]["body"].clone()
    }

    /// A second pack matching the fr entry of `languages_get_200_with_packs`.
    fn fr_pack() -> Value {
        json!({
            "schema_version": 1,
            "language": "fr",
            "name": "French",
            "native_name": "Français",
            "version": "1.2.0",
            "strings": { "nav": { "home": "Accueil" } },
        })
    }

    /// An en pack: never listed, still served (contract fact).
    fn en_pack() -> Value {
        json!({
            "schema_version": 1,
            "language": "en",
            "name": "English",
            "native_name": "English",
            "version": "1.0.0",
            "strings": { "nav": { "home": "Home" } },
        })
    }

    // -- request/response helpers -------------------------------------------

    async fn send(app: &TestApp, request: Request<Body>) -> Response {
        app.app.clone().oneshot(request).await.expect("infallible")
    }

    async fn get_path(app: &TestApp, path: &str) -> Response {
        send(
            app,
            Request::builder().uri(path).body(Body::empty()).unwrap(),
        )
        .await
    }

    async fn body_bytes(response: Response) -> Vec<u8> {
        axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("body")
            .to_vec()
    }

    async fn body_json(response: Response) -> Value {
        serde_json::from_slice(&body_bytes(response).await).expect("json body")
    }

    fn header_value(response: &Response, name: &str) -> Option<String> {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// Sorted no-space method set — the 405 `Allow` is synthesized by axum
    /// and its method order is not graded (fixture note).
    fn normalize_allow(raw: &str) -> String {
        let mut methods: Vec<&str> = raw.split(',').map(str::trim).collect();
        methods.sort_unstable();
        methods.join(",")
    }

    /// Grade a response against a recorded fixture: status, Content-Type,
    /// every `headers_that_matter` entry, and the body (JSON equality, or
    /// byte-emptiness for the empty-body responses). Returns the graded
    /// `{status, content_type, headers, body}` value for insta pinning.
    async fn check_fixture(name: &str, response: Response) -> Value {
        let recorded = fixture(&format!("{name}.json"));
        let expected = &recorded["response"];
        let status = response.status().as_u16();
        assert_eq!(json!(status), expected["status"], "{name}: status");

        let content_type = header_value(&response, "content-type");
        match &expected["content_type"] {
            Value::Null => assert_eq!(content_type, None, "{name}: expected no Content-Type"),
            Value::String(ct) => {
                assert_eq!(
                    content_type.as_deref(),
                    Some(ct.as_str()),
                    "{name}: content type"
                );
            }
            other => panic!("{name}: bad content_type in fixture: {other:?}"),
        }

        let mut graded_headers = serde_json::Map::new();
        if let Some(headers) = expected["headers_that_matter"].as_object() {
            for (key, expected_value) in headers {
                let expected_value = expected_value.as_str().expect("header value is a string");
                let actual = header_value(&response, key)
                    .unwrap_or_else(|| panic!("{name}: missing header {key}"));
                let (actual, expected_value) = if status == 405 && key == "Allow" {
                    (normalize_allow(&actual), normalize_allow(expected_value))
                } else {
                    (actual, expected_value.to_string())
                };
                assert_eq!(actual, expected_value, "{name}: header {key}");
                graded_headers.insert(key.clone(), json!(actual));
            }
        }

        let bytes = body_bytes(response).await;
        let graded_body = match &expected["body"] {
            Value::String(empty) if empty.is_empty() => {
                assert!(
                    bytes.is_empty(),
                    "{name}: expected an empty body, got {} bytes",
                    bytes.len()
                );
                json!("")
            }
            expected_body => {
                let actual: Value = serde_json::from_slice(&bytes)
                    .unwrap_or_else(|exc| panic!("{name}: body is not JSON: {exc}"));
                assert_eq!(&actual, expected_body, "{name}: body");
                actual
            }
        };

        json!({
            "status": status,
            "content_type": expected["content_type"],
            "headers": graded_headers,
            "body": graded_body,
        })
    }

    /// Grade against the fixture AND pin the graded value as an insta
    /// snapshot named after it.
    async fn assert_fixture(name: &str, response: Response) {
        let graded = check_fixture(name, response).await;
        insta::assert_json_snapshot!(name, graded);
    }

    // -- the eight fixtures -------------------------------------------------

    #[tokio::test]
    async fn languages_get_200_empty() {
        let app = offline_app();
        let response = get_path(&app, "/api/i18n/languages").await;
        // Owner decision: the list endpoint never carries an ETag.
        assert_eq!(header_value(&response, "etag"), None);
        assert_fixture("languages_get_200_empty", response).await;
    }

    #[tokio::test]
    async fn languages_get_200_with_packs() {
        let (app, _server) = github_app().await;
        let response = get_path(&app, "/api/i18n/languages").await;
        assert_eq!(header_value(&response, "etag"), None);
        assert_fixture("languages_get_200_with_packs", response).await;
    }

    #[tokio::test]
    async fn pack_get_200() {
        let (app, _server) = github_app().await;
        let response = get_path(&app, "/api/i18n/es").await;
        assert_fixture("pack_get_200", response).await;
    }

    #[tokio::test]
    async fn pack_get_304_etag_match() {
        let (app, _server) = github_app().await;
        let response = send(
            &app,
            Request::builder()
                .uri("/api/i18n/es")
                .header(header::IF_NONE_MATCH, "\"es-1.0.0\"")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        // The caching contract, asserted explicitly: same ETag +
        // Cache-Control as the 200, no Content-Type, empty body.
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(
            header_value(&response, "etag").as_deref(),
            Some("\"es-1.0.0\"")
        );
        assert_eq!(
            header_value(&response, "cache-control").as_deref(),
            Some("public, max-age=3600")
        );
        assert_eq!(header_value(&response, "content-type"), None);
        assert_fixture("pack_get_304_etag_match", response).await;
    }

    #[tokio::test]
    async fn pack_get_404_unknown() {
        let app = offline_app();
        let response = get_path(&app, "/api/i18n/xx").await;
        // No caching headers on the 404 (fixture note).
        assert_eq!(header_value(&response, "etag"), None);
        assert_eq!(header_value(&response, "cache-control"), None);
        assert_fixture("pack_get_404_unknown", response).await;
    }

    #[tokio::test]
    async fn languages_get_trailing_slash_404() {
        let app = offline_app();
        let response = get_path(&app, "/api/i18n/languages/").await;
        assert_fixture("languages_get_trailing_slash_404", response).await;

        // The slash variant of the {code} rule 404s the same way — it does
        // NOT fall through to the code route (fixture note).
        let response = get_path(&app, "/api/i18n/es/").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            body_json(response).await,
            json!({ "error": "Resource not found" })
        );
    }

    #[tokio::test]
    async fn pack_post_405() {
        let app = offline_app();
        let response = send(
            &app,
            Request::builder()
                .method("POST")
                .uri("/api/i18n/es")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_fixture("pack_post_405", response).await;

        // POST /api/i18n/languages behaves identically (fixture note).
        let response = send(
            &app,
            Request::builder()
                .method("POST")
                .uri("/api/i18n/languages")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        let allow = header_value(&response, "allow").expect("Allow on 405");
        assert_eq!(normalize_allow(&allow), "GET,HEAD,OPTIONS");
        assert_eq!(
            body_json(response).await,
            json!({ "error": "Method not allowed" })
        );
    }

    #[tokio::test]
    async fn languages_options_204() {
        let app = offline_app();
        let response = send(
            &app,
            Request::builder()
                .method("OPTIONS")
                .uri("/api/i18n/languages")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_fixture("languages_options_204", response).await;

        // OPTIONS /api/i18n/es answers identically (fixture note).
        let response = send(
            &app,
            Request::builder()
                .method("OPTIONS")
                .uri("/api/i18n/es")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            header_value(&response, "allow").as_deref(),
            Some("HEAD, GET, OPTIONS")
        );
        assert!(body_bytes(response).await.is_empty());
    }

    // -- alternate seeding + conditional-request edge cases -----------------

    #[tokio::test]
    async fn local_dir_replays_the_seeded_fixtures() {
        // The fixtures' alternate seeding: I18N_LOCAL_DIR instead of the
        // GitHub discovery — same wire responses, graded against the same
        // recordings (compare-only; the wiremock replays own the pins).
        let app = local_dir_app();
        let response = get_path(&app, "/api/i18n/languages").await;
        check_fixture("languages_get_200_with_packs", response).await;

        let response = get_path(&app, "/api/i18n/es").await;
        check_fixture("pack_get_200", response).await;

        let response = send(
            &app,
            Request::builder()
                .uri("/api/i18n/es")
                .header(header::IF_NONE_MATCH, "\"es-1.0.0\"")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        check_fixture("pack_get_304_etag_match", response).await;

        // The dir also holds an en pack: never listed (asserted by the
        // with-packs equality above), still served with its own ETag —
        // that is how English itself stays hot-swappable.
        let response = get_path(&app, "/api/i18n/en").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            header_value(&response, "etag").as_deref(),
            Some("\"en-1.0.0\"")
        );
        assert_eq!(
            header_value(&response, "cache-control").as_deref(),
            Some("public, max-age=3600")
        );
        assert_eq!(body_json(response).await, en_pack());
    }

    #[tokio::test]
    async fn stale_or_malformed_if_none_match_yields_the_full_200() {
        // Exact string compare, quotes included: a stale version, a weak
        // validator, missing quotes, and `*` all miss and get the full
        // envelope (no RFC weak-comparison or star semantics).
        let app = local_dir_app();
        for value in ["\"es-0.9.0\"", "W/\"es-1.0.0\"", "es-1.0.0", "*"] {
            let response = send(
                &app,
                Request::builder()
                    .uri("/api/i18n/es")
                    .header(header::IF_NONE_MATCH, value)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK, "If-None-Match: {value}");
            assert_eq!(
                header_value(&response, "etag").as_deref(),
                Some("\"es-1.0.0\""),
                "If-None-Match: {value}"
            );
            assert_eq!(
                body_json(response).await,
                es_pack(),
                "If-None-Match: {value}"
            );
        }
    }
}
