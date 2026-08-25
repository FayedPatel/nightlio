//! Hot-swappable language packs (v0.6.0 Feature 1) — the server-side store.
//!
//! [`I18nStore`] discovers language packs published as GitHub release assets
//! on the repository named by `I18N_GITHUB_REPO`, caches them in memory and
//! on disk, and serves them to the unauthenticated `/api/i18n` route family
//! (`api/src/routes/i18n.rs`). The eight hand-authored
//! fixtures in `contract/fixtures/i18n/` plus `contract/openapi-parts/`
//! `i18n.yaml` are the normative contract (DECISIONS.md entry dated
//! 2026-08-22).
//!
//! Design points, all from `docs/plans/v0.6.0.md`:
//!
//! - **Discovery.** `GET {I18N_GITHUB_API_BASE}/repos/{repo}/releases`
//!   `?per_page=100` with a stored-ETag conditional request — 304 responses
//!   do not count against the unauthenticated 60/h GitHub rate limit.
//!   Release tags follow `lang-<code>-v<semver>`; the tag grammar (not the
//!   prerelease flag) is what selects language releases, so a release
//!   missing its prerelease mark still works. The `<code>.json` asset is
//!   downloaded via its `browser_download_url` (unmetered). The highest
//!   semver per code wins, compared as a numeric triple.
//! - **Cadence.** On-demand + TTL (`I18N_REFRESH_SECS`, default 3600 s),
//!   guarded by a tokio `Mutex` so concurrent requests trigger exactly one
//!   fetch. No background task.
//! - **Cache.** Memory `RwLock<HashMap>` plus a disk mirror at
//!   `<dir(DATABASE_PATH)>/i18n/` (`<code>.json` files and a
//!   `manifest.json` holding the releases-list ETag), reloaded at
//!   construction. A refresh failure serves the stale cache forever with a
//!   warning; nothing cached means an empty language list / pack 404 and
//!   clients run on bundled English.
//! - **Validation.** Pack envelope
//!   `{schema_version: 1, language, name, native_name, version, strings}`
//!   with a 1 MiB cap. `strings` is a **nested** tree mirroring the
//!   `src/i18n/en.json` source format: each object key is one segment of
//!   the dot-path id and every leaf is a string (owner decision,
//!   DECISIONS.md 2026-08-22 amendment — nested end-to-end). A flat
//!   dot-key map is accepted on input as the degenerate nesting (a
//!   top-level `"nav.home"` key flattens to the same id, so rejecting it
//!   would buy nothing), but the canonical form is nested and the store
//!   serves packs exactly as validated, never converting. Non-string
//!   leaves (numbers, arrays, booleans, null) are rejected anywhere in
//!   the tree. A bad release is skipped with a warning and the previous
//!   good pack is kept.
//! - **Air-gapped mode.** `I18N_LOCAL_DIR` takes precedence over GitHub
//!   entirely: `<code>.json` files are validated and served straight from
//!   that directory on every request (so editing a file hot-swaps strings
//!   without a restart). `I18N_OFFLINE` keeps the GitHub path but never
//!   fetches, serving only what the disk cache already held.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use reqwest::StatusCode;
use reqwest::header;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::config::Config;

/// Hard cap on a pack asset or file: 1 MiB. Anything larger is rejected
/// before parsing (contract fact recorded in `contract/openapi-parts/`
/// `i18n.yaml`).
pub const MAX_PACK_BYTES: usize = 1_048_576;

/// Disk-cache metadata file (holds the stored releases-list ETag).
const MANIFEST_FILE: &str = "manifest.json";

/// GitHub requires a User-Agent on every API request.
const USER_AGENT_VALUE: &str = concat!("nightlio-api/", env!("CARGO_PKG_VERSION"));

/// The pack envelope — exactly these six keys, per the contract
/// (`LanguagePack` in `contract/openapi-parts/i18n.yaml`). Deserialization
/// rejects unknown keys and non-string leaves anywhere in the `strings`
/// tree, so an asset that fails this shape is a bad release and is
/// skipped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguagePack {
    /// Pinned integer 1; a future breaking envelope change bumps it (and
    /// this server version rejects it until it learns the new shape).
    pub schema_version: u32,
    /// Language code; equals the release-tag code and the `{code}` path
    /// segment.
    pub language: String,
    /// English display name (e.g. "Spanish").
    pub name: String,
    /// Self-name in the language itself (e.g. "Español").
    pub native_name: String,
    /// Pack semver from the `lang-<code>-v<semver>` release tag; also the
    /// second half of the route-layer `ETag: "<code>-<version>"`.
    pub version: String,
    /// Nested translation tree mirroring the `src/i18n/en.json` source
    /// format: each object key is one segment of the dot-path id and every
    /// leaf is a string, so `strings.nav.home` carries the id `nav.home`.
    /// Clients flatten the tree to dot-keys for lookup; a flat dot-key map
    /// is accepted on input as the degenerate nesting, but the canonical
    /// wire form is nested and packs are served exactly as validated.
    /// Partial packs are legal — the client falls back to bundled English
    /// per key.
    pub strings: BTreeMap<String, StringsNode>,
}

/// One node of the nested `strings` tree: a translated string (leaf) or a
/// deeper object whose keys are further dot-path segments. The untagged
/// serde representation accepts exactly those two JSON shapes — arrays,
/// numbers, booleans, and null anywhere in the tree fail deserialization,
/// so an asset carrying them is a bad release and is skipped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StringsNode {
    /// A translated value; `{date}`-style interpolation placeholders pass
    /// through verbatim as literal braces.
    Leaf(String),
    /// A deeper level of the dot-path hierarchy.
    Branch(BTreeMap<String, StringsNode>),
}

/// One entry of the `GET /api/i18n/languages` projection
/// (`LanguageInfo` in the contract): the pack envelope minus `strings` and
/// `schema_version`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LanguageInfo {
    pub code: String,
    pub name: String,
    pub native_name: String,
    pub version: String,
}

impl From<&LanguagePack> for LanguageInfo {
    fn from(pack: &LanguagePack) -> Self {
        LanguageInfo {
            code: pack.language.clone(),
            name: pack.name.clone(),
            native_name: pack.native_name.clone(),
            version: pack.version.clone(),
        }
    }
}

/// The subset of a GitHub release object the store reads. Unknown fields
/// are ignored (serde default).
#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

/// Disk-cache metadata (`manifest.json`).
#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    releases_etag: Option<String>,
}

/// State behind the refresh gate: held across the whole fetch so
/// concurrent requests queue instead of stampeding GitHub.
struct RefreshState {
    /// When the last refresh *attempt* finished (success or failure) — a
    /// failed attempt also waits out the TTL rather than hammering GitHub
    /// on every request.
    last_attempt: Option<Instant>,
    /// ETag of the last 200 releases-list response, replayed as
    /// `If-None-Match` (304 = list unchanged, reuse everything).
    releases_etag: Option<String>,
}

/// Server-side language-pack store. Constructed once at startup by
/// [`crate::state::AppState::new`] and shared as `Arc<I18nStore>`.
pub struct I18nStore {
    repo: String,
    api_base: String,
    token: Option<String>,
    ttl: Duration,
    offline: bool,
    local_dir: Option<PathBuf>,
    cache_dir: PathBuf,
    client: reqwest::Client,
    packs: RwLock<HashMap<String, LanguagePack>>,
    refresh: Mutex<RefreshState>,
}

impl I18nStore {
    /// Build the store from config and load whatever the disk cache holds.
    /// Never fails and never touches the network — the first refresh
    /// happens lazily on the first request.
    pub fn new(config: &Config) -> Self {
        let cache_dir = cache_dir_for(&config.database_path);
        let (packs, releases_etag) = load_disk_cache(&cache_dir);
        I18nStore {
            repo: config.i18n_github_repo.clone(),
            api_base: config.i18n_github_api_base.clone(),
            token: config.i18n_github_token.clone(),
            ttl: Duration::from_secs(config.i18n_refresh_secs),
            offline: config.i18n_offline,
            local_dir: config.i18n_local_dir.as_ref().map(PathBuf::from),
            cache_dir,
            client: http_client(),
            packs: RwLock::new(packs),
            refresh: Mutex::new(RefreshState {
                last_attempt: None,
                releases_etag,
            }),
        }
    }

    /// The `GET /api/i18n/languages` projection: cached packs as
    /// `{code, name, native_name, version}` entries, sorted by code
    /// ascending, with English excluded (the frontend unions bundled
    /// English into its picker — contract fact).
    pub async fn list_languages(&self) -> Vec<LanguageInfo> {
        if let Some(dir) = &self.local_dir {
            return list_from_dir(dir);
        }
        self.ensure_fresh().await;
        let packs = self.packs.read().expect("i18n packs lock");
        let mut languages: Vec<LanguageInfo> = packs
            .values()
            .filter(|pack| pack.language != "en")
            .map(LanguageInfo::from)
            .collect();
        languages.sort_by(|a, b| a.code.cmp(&b.code));
        languages
    }

    /// The pack for `code`, or `None` when nothing is cached for it (the
    /// route layer turns that into the standard 404 envelope). English is
    /// served here even though it is never listed — that is how English
    /// itself stays hot-swappable.
    pub async fn get_pack(&self, code: &str) -> Option<LanguagePack> {
        // Codes outside the release-tag grammar can never be cached; the
        // early return also keeps the local-dir path join clean.
        if !valid_code(code) {
            return None;
        }
        if let Some(dir) = &self.local_dir {
            return read_pack_file(&dir.join(format!("{code}.json")), code);
        }
        self.ensure_fresh().await;
        self.packs
            .read()
            .expect("i18n packs lock")
            .get(code)
            .cloned()
    }

    /// TTL-gated on-demand refresh. The tokio `Mutex` makes concurrent
    /// requests share one fetch: whoever acquires the lock first refreshes,
    /// the queued waiters observe the fresh `last_attempt` and return.
    async fn ensure_fresh(&self) {
        if self.offline {
            return;
        }
        let mut gate = self.refresh.lock().await;
        if let Some(last) = gate.last_attempt
            && last.elapsed() < self.ttl
        {
            return;
        }
        if let Err(err) = self.refresh_locked(&mut gate).await {
            tracing::warn!(
                error = %err,
                "i18n refresh failed; serving cached language packs"
            );
        }
        gate.last_attempt = Some(Instant::now());
    }

    /// One discovery pass against GitHub. Runs with the refresh gate held.
    async fn refresh_locked(&self, gate: &mut RefreshState) -> anyhow::Result<()> {
        let url = format!(
            "{}/repos/{}/releases?per_page=100",
            self.api_base.trim_end_matches('/'),
            self.repo
        );
        let mut request = self
            .client
            .get(&url)
            .header(header::USER_AGENT, USER_AGENT_VALUE)
            .header(header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = &self.token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        if let Some(etag) = &gate.releases_etag {
            request = request.header(header::IF_NONE_MATCH, etag);
        }
        let response = request.send().await.context("releases list request")?;
        if response.status() == StatusCode::NOT_MODIFIED {
            // Nothing changed upstream; the cached packs stay as they are
            // and the 304 did not count against the rate limit.
            return Ok(());
        }
        if !response.status().is_success() {
            bail!("releases list returned HTTP {}", response.status());
        }
        let etag = response
            .headers()
            .get(header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let releases: Vec<Release> = response.json().await.context("releases list body")?;

        // Highest semver per code, numeric triple compare. The tag grammar
        // is the filter — anything else in the release list is ignored.
        let mut wanted: HashMap<String, ([u64; 3], String, String)> = HashMap::new();
        for release in &releases {
            let Some((code, triple, version)) = parse_release_tag(&release.tag_name) else {
                continue;
            };
            let asset_name = format!("{code}.json");
            let Some(asset) = release.assets.iter().find(|asset| asset.name == asset_name) else {
                tracing::warn!(
                    tag = %release.tag_name,
                    "i18n release has no <code>.json asset; skipping"
                );
                continue;
            };
            match wanted.get(&code) {
                Some((best, _, _)) if *best >= triple => {}
                _ => {
                    wanted.insert(code, (triple, version, asset.browser_download_url.clone()));
                }
            }
        }

        let current: HashMap<String, LanguagePack> =
            self.packs.read().expect("i18n packs lock").clone();
        let mut fresh: HashMap<String, LanguagePack> = HashMap::new();
        for (code, (_, version, download_url)) in &wanted {
            if let Some(existing) = current.get(code)
                && existing.version == *version
            {
                // Already have this exact release — no re-download.
                fresh.insert(code.clone(), existing.clone());
                continue;
            }
            match self.download_pack(download_url, code, version).await {
                Ok(pack) => {
                    self.persist_pack(&pack);
                    fresh.insert(code.clone(), pack);
                }
                Err(err) => {
                    tracing::warn!(
                        code = %code,
                        version = %version,
                        error = %err,
                        "i18n: skipping bad language-pack release; keeping the previous pack if any"
                    );
                    if let Some(existing) = current.get(code) {
                        fresh.insert(code.clone(), existing.clone());
                    }
                }
            }
        }
        // A successful 200 list is authoritative: a code with no matching
        // release anymore (withdrawn language) drops out of the cache.
        for code in current.keys() {
            if !fresh.contains_key(code) {
                let _ = fs::remove_file(self.cache_dir.join(format!("{code}.json")));
            }
        }
        *self.packs.write().expect("i18n packs lock") = fresh;
        gate.releases_etag = etag;
        self.persist_manifest(gate.releases_etag.clone());
        Ok(())
    }

    /// Download one `<code>.json` asset via its `browser_download_url`,
    /// enforcing the 1 MiB cap while streaming, then validate the
    /// envelope. No Authorization header here — asset downloads are
    /// unmetered and public.
    async fn download_pack(
        &self,
        url: &str,
        code: &str,
        version: &str,
    ) -> anyhow::Result<LanguagePack> {
        let mut response = self
            .client
            .get(url)
            .header(header::USER_AGENT, USER_AGENT_VALUE)
            .send()
            .await
            .context("asset download request")?;
        if !response.status().is_success() {
            bail!("asset download returned HTTP {}", response.status());
        }
        if let Some(length) = response.content_length()
            && length > MAX_PACK_BYTES as u64
        {
            bail!("asset exceeds the 1 MiB pack cap ({length} bytes)");
        }
        let mut body: Vec<u8> = Vec::new();
        while let Some(chunk) = response.chunk().await.context("asset download body")? {
            if body.len() + chunk.len() > MAX_PACK_BYTES {
                bail!("asset exceeds the 1 MiB pack cap");
            }
            body.extend_from_slice(&chunk);
        }
        parse_and_validate(&body, code, Some(version))
    }

    /// Best-effort disk-cache write; a failure only warns (the memory
    /// cache still serves).
    fn persist_pack(&self, pack: &LanguagePack) {
        let path = self.cache_dir.join(format!("{}.json", pack.language));
        let bytes = match serde_json::to_vec(pack) {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!(error = %err, "i18n: could not serialize pack for the disk cache");
                return;
            }
        };
        if let Err(err) = fs::create_dir_all(&self.cache_dir).and_then(|()| fs::write(&path, bytes))
        {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "i18n: could not persist language pack to the disk cache"
            );
        }
    }

    /// Best-effort `manifest.json` write (stored releases-list ETag).
    fn persist_manifest(&self, releases_etag: Option<String>) {
        let manifest = Manifest { releases_etag };
        let bytes = match serde_json::to_vec(&manifest) {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!(error = %err, "i18n: could not serialize the disk-cache manifest");
                return;
            }
        };
        if let Err(err) = fs::create_dir_all(&self.cache_dir)
            .and_then(|()| fs::write(self.cache_dir.join(MANIFEST_FILE), bytes))
        {
            tracing::warn!(error = %err, "i18n: could not persist the disk-cache manifest");
        }
    }
}

/// `<dir(DATABASE_PATH)>/i18n` — mirrors the `rate_limit_db_path`
/// parent-dir derivation in `config.rs` (string-level `rsplit_once('/')`,
/// bare filename means the current directory).
fn cache_dir_for(database_path: &str) -> PathBuf {
    let dir = match database_path.rsplit_once('/') {
        Some((dir, _)) if !dir.is_empty() => dir,
        Some(_) => "/",
        None => ".",
    };
    PathBuf::from(format!("{}/i18n", dir.trim_end_matches('/')))
}

/// Timeouts keep a dead GitHub from hanging the on-demand refresh (which
/// runs inside a request); TLS is the crate-wide rustls stack (see the
/// reqwest features in `api/Cargo.toml`).
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("reqwest client")
}

/// Load the disk cache written by a previous run: every `<code>.json`
/// (validated like a fresh download) plus the manifest's stored ETag.
fn load_disk_cache(cache_dir: &Path) -> (HashMap<String, LanguagePack>, Option<String>) {
    let mut packs = HashMap::new();
    let mut releases_etag = None;
    let Ok(entries) = fs::read_dir(cache_dir) else {
        // No cache directory yet — a fresh instance.
        return (packs, releases_etag);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().and_then(|name| name.to_str()) == Some(MANIFEST_FILE) {
            releases_etag = fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok())
                .and_then(|manifest| manifest.releases_etag);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if !valid_code(stem) {
            continue;
        }
        if let Some(pack) = read_pack_file(&path, stem) {
            packs.insert(stem.to_string(), pack);
        }
    }
    (packs, releases_etag)
}

/// Read + validate one pack file (disk cache or `I18N_LOCAL_DIR`). Invalid
/// or oversized files are warned about and treated as absent.
fn read_pack_file(path: &Path, expected_code: &str) -> Option<LanguagePack> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > MAX_PACK_BYTES as u64 {
        tracing::warn!(
            path = %path.display(),
            "i18n: pack file exceeds the 1 MiB cap; ignoring"
        );
        return None;
    }
    let bytes = fs::read(path).ok()?;
    match parse_and_validate(&bytes, expected_code, None) {
        Ok(pack) => Some(pack),
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "i18n: ignoring invalid pack file"
            );
            None
        }
    }
}

/// `I18N_LOCAL_DIR` listing: every valid `<code>.json` in the directory,
/// re-read on each request, English excluded, sorted by code.
fn list_from_dir(dir: &Path) -> Vec<LanguageInfo> {
    let mut languages = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!(
                path = %dir.display(),
                error = %err,
                "i18n: could not read I18N_LOCAL_DIR"
            );
            return languages;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if stem == "en" || !valid_code(stem) {
            continue;
        }
        if let Some(pack) = read_pack_file(&path, stem) {
            languages.push(LanguageInfo::from(&pack));
        }
    }
    languages.sort_by(|a, b| a.code.cmp(&b.code));
    languages
}

/// Validate raw pack bytes into the envelope. `expected_code` comes from
/// the release tag / file stem; `expected_version` (release downloads
/// only) pins the envelope to the tag semver so the list projection, the
/// envelope, and the route-layer ETag can never disagree.
fn parse_and_validate(
    bytes: &[u8],
    expected_code: &str,
    expected_version: Option<&str>,
) -> anyhow::Result<LanguagePack> {
    if bytes.len() > MAX_PACK_BYTES {
        bail!("pack exceeds the 1 MiB cap ({} bytes)", bytes.len());
    }
    let pack: LanguagePack =
        serde_json::from_slice(bytes).context("pack is not a valid envelope")?;
    if pack.schema_version != 1 {
        bail!("unsupported schema_version {}", pack.schema_version);
    }
    if pack.language != expected_code {
        bail!(
            "pack language `{}` does not match expected code `{expected_code}`",
            pack.language
        );
    }
    if let Some(version) = expected_version
        && pack.version != version
    {
        bail!(
            "pack version `{}` does not match the release-tag version `{version}`",
            pack.version
        );
    }
    Ok(pack)
}

/// Parse a `lang-<code>-v<semver>` release tag into
/// `(code, numeric triple, raw semver text)`. Hand-rolled equivalent of
/// `^lang-([a-z]{2,8}(?:-[A-Za-z0-9]+)?)-v(\d+\.\d+\.\d+)$` — the frozen
/// dependency set has no regex crate. `rsplit_once("-v")` splits at the
/// *last* `-v`, so subtag codes like `pt-BR` survive.
fn parse_release_tag(tag: &str) -> Option<(String, [u64; 3], String)> {
    let rest = tag.strip_prefix("lang-")?;
    let (code, version) = rest.rsplit_once("-v")?;
    let triple = parse_semver(version)?;
    if !valid_code(code) {
        return None;
    }
    Some((code.to_string(), triple, version.to_string()))
}

/// `[a-z]{2,8}` optionally followed by `-[A-Za-z0-9]+` (one subtag max) —
/// the code grammar from the release-tag pattern.
fn valid_code(code: &str) -> bool {
    let (primary, subtag) = match code.split_once('-') {
        Some((primary, subtag)) => (primary, Some(subtag)),
        None => (code, None),
    };
    let primary_ok =
        (2..=8).contains(&primary.len()) && primary.bytes().all(|b| b.is_ascii_lowercase());
    let subtag_ok = match subtag {
        None => true,
        Some(subtag) => {
            !subtag.is_empty()
                && !subtag.contains('-')
                && subtag.bytes().all(|b| b.is_ascii_alphanumeric())
        }
    };
    primary_ok && subtag_ok
}

/// `\d+\.\d+\.\d+` as a numeric triple, compared lexicographically (which
/// for a `[u64; 3]` is exactly the semver ordering).
fn parse_semver(version: &str) -> Option<[u64; 3]> {
    let mut parts = version.split('.');
    let major = parse_component(parts.next()?)?;
    let minor = parse_component(parts.next()?)?;
    let patch = parse_component(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some([major, minor, patch])
}

/// All-digits component (`u64::from_str` alone would accept a leading `+`,
/// which `\d+` does not).
fn parse_component(raw: &str) -> Option<u64> {
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    raw.parse::<u64>().ok()
}

// ---------------------------------------------------------------------------
// Tests — wiremock fakes the GitHub API behind I18N_GITHUB_API_BASE
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rstest::rstest;
    use serde_json::{Value, json};
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    /// Releases path for the default `I18N_GITHUB_REPO`.
    const RELEASES_PATH: &str = "/repos/FayedPatel/nightlio/releases";

    /// Config pointed at the wiremock base with a tempdir database path
    /// (so the disk cache lands under `<tempdir>/i18n/`). TTL defaults to
    /// 0 so every request refreshes; tests that need the gate override it.
    fn test_config(dir: &Path, base: &str, extra: &[(&str, &str)]) -> Config {
        let mut vars: HashMap<String, String> = HashMap::from([
            ("APP_ENV".to_string(), "development".to_string()),
            ("I18N_GITHUB_API_BASE".to_string(), base.to_string()),
            ("I18N_REFRESH_SECS".to_string(), "0".to_string()),
        ]);
        for (key, value) in extra {
            vars.insert((*key).to_string(), (*value).to_string());
        }
        let lookup = move |key: &str| vars.get(key).cloned();
        let mut cfg = Config::from_lookup(&lookup);
        cfg.database_path = dir.join("nightlio.db").to_string_lossy().into_owned();
        cfg
    }

    /// A GitHub release object carrying one asset whose download URL
    /// points back at the mock server.
    fn release(tag: &str, asset: &str, base: &str) -> Value {
        json!({
            "tag_name": tag,
            "prerelease": true,
            "assets": [{
                "name": asset,
                "browser_download_url": format!("{base}/download/{tag}/{asset}"),
            }],
        })
    }

    /// A valid pack envelope; the `nav.home` leaf value encodes
    /// code+version so tests can tell exactly which pack build they are
    /// being served. `strings` is the canonical NESTED form (one object
    /// level per dot-path segment).
    fn pack_json(code: &str, name: &str, native: &str, version: &str) -> Value {
        json!({
            "schema_version": 1,
            "language": code,
            "name": name,
            "native_name": native,
            "version": version,
            "strings": { "nav": { "home": format!("Home-{code}-{version}") } },
        })
    }

    /// Walk a dot-path through a pack's nested `strings` tree to its leaf
    /// value, panicking (with the path) on any miss or shape surprise.
    fn leaf<'a>(pack: &'a LanguagePack, path: &str) -> &'a str {
        let mut segments = path.split('.');
        let first = segments.next().expect("non-empty path");
        let mut node = pack
            .strings
            .get(first)
            .unwrap_or_else(|| panic!("{path}: no `{first}` node"));
        for segment in segments {
            match node {
                StringsNode::Branch(map) => {
                    node = map
                        .get(segment)
                        .unwrap_or_else(|| panic!("{path}: no `{segment}` node"));
                }
                StringsNode::Leaf(_) => panic!("{path}: hit a leaf before `{segment}`"),
            }
        }
        match node {
            StringsNode::Leaf(value) => value,
            StringsNode::Branch(_) => panic!("{path}: not a leaf"),
        }
    }

    async fn mount_releases(server: &MockServer, body: &Value) {
        Mock::given(method("GET"))
            .and(path(RELEASES_PATH))
            .and(query_param("per_page", "100"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1..)
            .mount(server)
            .await;
    }

    async fn mount_asset(server: &MockServer, tag: &str, name: &str, body: &Value) {
        Mock::given(method("GET"))
            .and(path(format!("/download/{tag}/{name}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(server)
            .await;
    }

    // -- discovery + serving ------------------------------------------------

    #[tokio::test]
    async fn discovers_validates_and_serves_packs() {
        let dir = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        let base = server.uri();
        mount_releases(
            &server,
            &json!([
                release("lang-fr-v1.2.0", "fr.json", &base),
                release("lang-es-v1.0.0", "es.json", &base),
                release("lang-en-v1.0.1", "en.json", &base),
            ]),
        )
        .await;
        mount_asset(
            &server,
            "lang-es-v1.0.0",
            "es.json",
            &pack_json("es", "Spanish", "Español", "1.0.0"),
        )
        .await;
        mount_asset(
            &server,
            "lang-fr-v1.2.0",
            "fr.json",
            &pack_json("fr", "French", "Français", "1.2.0"),
        )
        .await;
        mount_asset(
            &server,
            "lang-en-v1.0.1",
            "en.json",
            &pack_json("en", "English", "English", "1.0.1"),
        )
        .await;

        let store = I18nStore::new(&test_config(dir.path(), &base, &[]));
        let languages = store.list_languages().await;
        // Sorted by code ascending; English is never listed.
        let codes: Vec<&str> = languages.iter().map(|l| l.code.as_str()).collect();
        assert_eq!(codes, ["es", "fr"]);
        assert_eq!(languages[0].name, "Spanish");
        assert_eq!(languages[0].native_name, "Español");
        assert_eq!(languages[0].version, "1.0.0");

        let es = store.get_pack("es").await.expect("es pack");
        assert_eq!(es.schema_version, 1);
        assert_eq!(leaf(&es, "nav.home"), "Home-es-1.0.0");
        // English is served even though it is never listed.
        let en = store.get_pack("en").await.expect("en pack");
        assert_eq!(en.version, "1.0.1");
        // Unknown code misses the cache.
        assert_eq!(store.get_pack("xx").await, None);

        // Disk cache written next to the database path.
        assert!(dir.path().join("i18n/es.json").is_file());
        assert!(dir.path().join("i18n/fr.json").is_file());
        assert!(dir.path().join("i18n/manifest.json").is_file());
        // TTL 0 forces a refresh per call above, but the version-match skip
        // means each asset was still downloaded exactly once (.expect(1)).
        server.verify().await;
    }

    #[tokio::test]
    async fn conditional_requests_replay_the_stored_etag() {
        let dir = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        let base = server.uri();
        Mock::given(method("GET"))
            .and(path(RELEASES_PATH))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("ETag", "W/\"releases-1\"")
                    .set_body_json(json!([release("lang-es-v1.0.0", "es.json", &base)])),
            )
            .expect(1)
            .mount(&server)
            .await;
        mount_asset(
            &server,
            "lang-es-v1.0.0",
            "es.json",
            &pack_json("es", "Spanish", "Español", "1.0.0"),
        )
        .await;

        let store = I18nStore::new(&test_config(dir.path(), &base, &[]));
        assert_eq!(store.list_languages().await.len(), 1);
        server.verify().await;

        // From now on the list request must carry If-None-Match and a 304
        // must leave the cached packs untouched (no asset mocks exist).
        server.reset().await;
        Mock::given(method("GET"))
            .and(path(RELEASES_PATH))
            .and(header("If-None-Match", "W/\"releases-1\""))
            .respond_with(ResponseTemplate::new(304))
            .expect(1..)
            .mount(&server)
            .await;
        assert_eq!(store.list_languages().await.len(), 1);
        assert_eq!(store.get_pack("es").await.unwrap().version, "1.0.0");
        server.verify().await;
    }

    #[tokio::test]
    async fn concurrent_requests_share_one_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        let base = server.uri();
        Mock::given(method("GET"))
            .and(path(RELEASES_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([release(
                "lang-es-v1.0.0",
                "es.json",
                &base
            )])))
            .expect(1)
            .mount(&server)
            .await;
        mount_asset(
            &server,
            "lang-es-v1.0.0",
            "es.json",
            &pack_json("es", "Spanish", "Español", "1.0.0"),
        )
        .await;

        // Real TTL: the tokio-Mutex gate must collapse the concurrent
        // callers into a single releases fetch (.expect(1) above).
        let store = I18nStore::new(&test_config(
            dir.path(),
            &base,
            &[("I18N_REFRESH_SECS", "3600")],
        ));
        let (a, b, pack) = tokio::join!(
            store.list_languages(),
            store.list_languages(),
            store.get_pack("es")
        );
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(pack.unwrap().version, "1.0.0");
        server.verify().await;
    }

    // -- failure handling ---------------------------------------------------

    #[tokio::test]
    async fn bad_new_release_keeps_the_previous_pack() {
        let dir = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        let base = server.uri();
        mount_releases(
            &server,
            &json!([release("lang-es-v1.0.0", "es.json", &base)]),
        )
        .await;
        mount_asset(
            &server,
            "lang-es-v1.0.0",
            "es.json",
            &pack_json("es", "Spanish", "Español", "1.0.0"),
        )
        .await;
        let store = I18nStore::new(&test_config(dir.path(), &base, &[]));
        assert_eq!(store.get_pack("es").await.unwrap().version, "1.0.0");
        server.verify().await;

        // A newer release whose asset fails validation (language mismatch)
        // is skipped and the previous good pack keeps serving.
        server.reset().await;
        mount_releases(
            &server,
            &json!([release("lang-es-v2.0.0", "es.json", &base)]),
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/download/lang-es-v2.0.0/es.json"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(pack_json("fr", "Spanish", "Español", "2.0.0")),
            )
            .expect(1..)
            .mount(&server)
            .await;
        let es = store.get_pack("es").await.expect("previous pack kept");
        assert_eq!(es.version, "1.0.0");
        assert_eq!(leaf(&es, "nav.home"), "Home-es-1.0.0");
        let languages = store.list_languages().await;
        assert_eq!(languages[0].version, "1.0.0");
    }

    #[tokio::test]
    async fn refresh_failure_serves_the_stale_cache() {
        let dir = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        let base = server.uri();
        mount_releases(
            &server,
            &json!([release("lang-es-v1.0.0", "es.json", &base)]),
        )
        .await;
        mount_asset(
            &server,
            "lang-es-v1.0.0",
            "es.json",
            &pack_json("es", "Spanish", "Español", "1.0.0"),
        )
        .await;
        let store = I18nStore::new(&test_config(dir.path(), &base, &[]));
        assert_eq!(store.list_languages().await.len(), 1);
        server.verify().await;

        // GitHub starts failing: serve stale forever.
        server.reset().await;
        Mock::given(method("GET"))
            .and(path(RELEASES_PATH))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        assert_eq!(store.list_languages().await.len(), 1);
        assert_eq!(store.get_pack("es").await.unwrap().version, "1.0.0");
    }

    #[tokio::test]
    async fn cold_cache_with_unreachable_github_yields_the_empty_state() {
        let dir = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(RELEASES_PATH))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let store = I18nStore::new(&test_config(dir.path(), &server.uri(), &[]));
        assert!(store.list_languages().await.is_empty());
        assert_eq!(store.get_pack("es").await, None);
    }

    #[tokio::test]
    async fn oversized_pack_asset_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        let base = server.uri();
        mount_releases(
            &server,
            &json!([release("lang-es-v1.0.0", "es.json", &base)]),
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/download/lang-es-v1.0.0/es.json"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; MAX_PACK_BYTES + 1]))
            .mount(&server)
            .await;
        let store = I18nStore::new(&test_config(dir.path(), &base, &[]));
        assert_eq!(store.get_pack("es").await, None);
        assert!(store.list_languages().await.is_empty());
    }

    // -- disk cache ---------------------------------------------------------

    #[tokio::test]
    async fn disk_cache_reloads_across_restarts_with_the_stored_etag() {
        let dir = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        let base = server.uri();
        Mock::given(method("GET"))
            .and(path(RELEASES_PATH))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("ETag", "W/\"releases-7\"")
                    .set_body_json(json!([release("lang-es-v1.0.0", "es.json", &base)])),
            )
            .expect(1)
            .mount(&server)
            .await;
        mount_asset(
            &server,
            "lang-es-v1.0.0",
            "es.json",
            &pack_json("es", "Spanish", "Español", "1.0.0"),
        )
        .await;
        let cfg = test_config(dir.path(), &base, &[]);
        let store = I18nStore::new(&cfg);
        assert_eq!(store.list_languages().await.len(), 1);
        server.verify().await;
        drop(store);

        // "Restart": a new store must load the packs from disk and replay
        // the persisted ETag; the 304 proves both survived.
        server.reset().await;
        Mock::given(method("GET"))
            .and(path(RELEASES_PATH))
            .and(header("If-None-Match", "W/\"releases-7\""))
            .respond_with(ResponseTemplate::new(304))
            .expect(1..)
            .mount(&server)
            .await;
        let restarted = I18nStore::new(&cfg);
        let languages = restarted.list_languages().await;
        assert_eq!(languages.len(), 1);
        assert_eq!(languages[0].code, "es");
        let es = restarted.get_pack("es").await.expect("pack from disk");
        assert_eq!(leaf(&es, "nav.home"), "Home-es-1.0.0");
        server.verify().await;
    }

    #[tokio::test]
    async fn offline_mode_serves_the_disk_cache_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("i18n");
        fs::create_dir_all(&cache).unwrap();
        fs::write(
            cache.join("es.json"),
            serde_json::to_vec(&pack_json("es", "Spanish", "Español", "1.0.0")).unwrap(),
        )
        .unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(RELEASES_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(0)
            .mount(&server)
            .await;
        let store = I18nStore::new(&test_config(
            dir.path(),
            &server.uri(),
            &[("I18N_OFFLINE", "1")],
        ));
        let languages = store.list_languages().await;
        assert_eq!(languages.len(), 1);
        assert_eq!(languages[0].code, "es");
        assert!(store.get_pack("es").await.is_some());
        server.verify().await;
    }

    // -- I18N_LOCAL_DIR -----------------------------------------------------

    #[tokio::test]
    async fn local_dir_takes_precedence_and_rereads_files_per_request() {
        let dir = tempfile::tempdir().unwrap();
        let packs_dir = dir.path().join("packs");
        fs::create_dir_all(&packs_dir).unwrap();
        fs::write(
            packs_dir.join("es.json"),
            serde_json::to_vec(&pack_json("es", "Spanish", "Español", "1.0.0")).unwrap(),
        )
        .unwrap();
        fs::write(
            packs_dir.join("en.json"),
            serde_json::to_vec(&pack_json("en", "English", "English", "1.0.0")).unwrap(),
        )
        .unwrap();
        fs::write(packs_dir.join("broken.json"), b"{not json").unwrap();

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(RELEASES_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(0) // precedence: GitHub is never contacted
            .mount(&server)
            .await;
        let store = I18nStore::new(&test_config(
            dir.path(),
            &server.uri(),
            &[("I18N_LOCAL_DIR", packs_dir.to_str().unwrap())],
        ));

        // en excluded from the list, broken file skipped, es listed.
        let languages = store.list_languages().await;
        let codes: Vec<&str> = languages.iter().map(|l| l.code.as_str()).collect();
        assert_eq!(codes, ["es"]);
        assert_eq!(
            leaf(&store.get_pack("es").await.unwrap(), "nav.home"),
            "Home-es-1.0.0"
        );
        // en still serves even though it is never listed.
        assert!(store.get_pack("en").await.is_some());

        // Hot-swap: doctoring the file changes the served pack on the next
        // request — no restart, no cache to invalidate.
        let mut doctored = pack_json("es", "Spanish", "Español", "1.0.1");
        doctored["strings"]["nav"]["home"] = json!("Hogar");
        fs::write(
            packs_dir.join("es.json"),
            serde_json::to_vec(&doctored).unwrap(),
        )
        .unwrap();
        let es = store.get_pack("es").await.unwrap();
        assert_eq!(leaf(&es, "nav.home"), "Hogar");
        assert_eq!(es.version, "1.0.1");
        server.verify().await;
    }

    // -- env seams ----------------------------------------------------------

    #[tokio::test]
    async fn repo_and_token_seams_shape_the_discovery_request() {
        let dir = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        let base = server.uri();
        Mock::given(method("GET"))
            .and(path("/repos/acme/l10n/releases"))
            .and(header("Authorization", "Bearer test-token"))
            .and(header("User-Agent", USER_AGENT_VALUE))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1..)
            .mount(&server)
            .await;
        let store = I18nStore::new(&test_config(
            dir.path(),
            &base,
            &[
                ("I18N_GITHUB_REPO", "acme/l10n"),
                ("I18N_GITHUB_TOKEN", "test-token"),
            ],
        ));
        assert!(store.list_languages().await.is_empty());
        server.verify().await;
    }

    // -- tag grammar + semver -----------------------------------------------

    #[tokio::test]
    async fn highest_semver_wins_and_invalid_tags_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        let base = server.uri();
        mount_releases(
            &server,
            &json!([
                release("lang-es-v1.9.0", "es.json", &base),
                release("lang-es-v1.10.0", "es.json", &base),
                release("lang-ES-v1.0.0", "ES.json", &base),
                release("lang-es-1.0.0", "es.json", &base),
                release("lang-x-v1.0.0", "x.json", &base),
                release("lang-waytoolong-v1.0.0", "waytoolong.json", &base),
                release("lang-es-v1.0", "es.json", &base),
                release("v0.6.0", "nightlio.tar.gz", &base),
            ]),
        )
        .await;
        // Only the winning release's asset may ever be fetched.
        mount_asset(
            &server,
            "lang-es-v1.10.0",
            "es.json",
            &pack_json("es", "Spanish", "Español", "1.10.0"),
        )
        .await;
        let store = I18nStore::new(&test_config(dir.path(), &base, &[]));
        let languages = store.list_languages().await;
        assert_eq!(languages.len(), 1);
        assert_eq!(languages[0].code, "es");
        assert_eq!(languages[0].version, "1.10.0");
        server.verify().await;
    }

    #[rstest]
    #[case("lang-es-v1.0.0", Some(("es", [1, 0, 0], "1.0.0")))]
    #[case("lang-pt-BR-v2.3.4", Some(("pt-BR", [2, 3, 4], "2.3.4")))]
    #[case("lang-yue-v0.1.0", Some(("yue", [0, 1, 0], "0.1.0")))]
    #[case("lang-sv-v1.0.0", Some(("sv", [1, 0, 0], "1.0.0")))]
    #[case("lang-ES-v1.0.0", None)] // uppercase primary
    #[case("lang-e-v1.0.0", None)] // primary too short
    #[case("lang-waytoolong-v1.0.0", None)] // primary too long
    #[case("lang-es-v1.0", None)] // two-part version
    #[case("lang-es-v1.0.0.0", None)] // four-part version
    #[case("lang-es-vX.0.0", None)] // non-numeric version
    #[case("lang-es-v1.0.0-rc1", None)] // prerelease suffix
    #[case("lang-es-1.0.0", None)] // missing the v
    #[case("es-v1.0.0", None)] // missing the lang- prefix
    #[case("lang--v1.0.0", None)] // empty code
    #[case("lang-es--v1.0.0", None)] // empty subtag
    #[case("lang-es-b-c-v1.0.0", None)] // two subtags
    #[case("v0.6.0", None)] // an app release, never a language
    fn release_tag_grammar(#[case] tag: &str, #[case] expected: Option<(&str, [u64; 3], &str)>) {
        let parsed = parse_release_tag(tag);
        match expected {
            Some((code, triple, version)) => {
                let (parsed_code, parsed_triple, parsed_version) = parsed.expect(tag);
                assert_eq!(parsed_code, code);
                assert_eq!(parsed_triple, triple);
                assert_eq!(parsed_version, version);
            }
            None => assert_eq!(parsed, None, "{tag}"),
        }
    }

    #[test]
    fn semver_triples_compare_numerically() {
        assert!(parse_semver("1.10.0").unwrap() > parse_semver("1.9.9").unwrap());
        assert!(parse_semver("2.0.0").unwrap() > parse_semver("1.99.99").unwrap());
        assert_eq!(parse_semver("01.2.3"), Some([1, 2, 3]));
        assert_eq!(parse_semver("1.2"), None);
        assert_eq!(parse_semver("+1.2.3"), None);
        assert_eq!(parse_semver("1..3"), None);
        assert_eq!(parse_semver(""), None);
    }

    // -- envelope validation ------------------------------------------------

    #[test]
    fn envelope_validation_accepts_the_contract_shape() {
        // Canonical nested form, including a three-level branch mirroring
        // src/i18n/en.json's common.day.* subtree.
        let mut payload = pack_json("es", "Spanish", "Español", "1.0.0");
        payload["strings"]["common"] = json!({ "day": { "sun": "Dom" } });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let pack = parse_and_validate(&bytes, "es", Some("1.0.0")).expect("valid pack");
        assert_eq!(pack.language, "es");
        assert_eq!(pack.native_name, "Español");
        assert_eq!(leaf(&pack, "nav.home"), "Home-es-1.0.0");
        assert_eq!(leaf(&pack, "common.day.sun"), "Dom");
    }

    /// A legacy FLAT dot-key map is ACCEPTED: it is just the degenerate
    /// nesting (a top-level "nav.home" key flattens to the same id, so
    /// rejecting it would buy nothing). The canonical form is nested and
    /// packs are served exactly as validated — this pack would serve flat,
    /// which the client-side flattener handles identically (owner
    /// decision, DECISIONS.md 2026-08-22 amendment).
    #[test]
    fn envelope_validation_accepts_the_legacy_flat_shape() {
        let bytes = serde_json::to_vec(&json!({
            "schema_version": 1, "language": "es", "name": "Spanish",
            "native_name": "Español", "version": "1.0.0",
            "strings": { "nav.home": "Inicio", "common.cancel": "Cancelar" }
        }))
        .unwrap();
        let pack = parse_and_validate(&bytes, "es", Some("1.0.0")).expect("flat accepted");
        // The dotted keys stay top-level leaves — no conversion happens.
        assert_eq!(
            pack.strings["nav.home"],
            StringsNode::Leaf("Inicio".to_string())
        );
        assert_eq!(
            pack.strings["common.cancel"],
            StringsNode::Leaf("Cancelar".to_string())
        );
    }

    #[rstest]
    #[case::schema_version_not_1(json!({
        "schema_version": 2, "language": "es", "name": "Spanish",
        "native_name": "Español", "version": "1.0.0", "strings": {}
    }))]
    #[case::language_mismatch(json!({
        "schema_version": 1, "language": "fr", "name": "Spanish",
        "native_name": "Español", "version": "1.0.0", "strings": {}
    }))]
    #[case::version_mismatch(json!({
        "schema_version": 1, "language": "es", "name": "Spanish",
        "native_name": "Español", "version": "9.9.9", "strings": {}
    }))]
    #[case::non_string_top_level_value(json!({
        "schema_version": 1, "language": "es", "name": "Spanish",
        "native_name": "Español", "version": "1.0.0", "strings": {"nav.count": 3}
    }))]
    #[case::non_string_nested_leaf(json!({
        "schema_version": 1, "language": "es", "name": "Spanish",
        "native_name": "Español", "version": "1.0.0", "strings": {"nav": {"count": 3}}
    }))]
    #[case::array_leaf(json!({
        "schema_version": 1, "language": "es", "name": "Spanish",
        "native_name": "Español", "version": "1.0.0", "strings": {"nav": {"home": ["x"]}}
    }))]
    #[case::null_leaf(json!({
        "schema_version": 1, "language": "es", "name": "Spanish",
        "native_name": "Español", "version": "1.0.0", "strings": {"nav": {"home": null}}
    }))]
    #[case::bool_leaf(json!({
        "schema_version": 1, "language": "es", "name": "Spanish",
        "native_name": "Español", "version": "1.0.0", "strings": {"nav": {"home": true}}
    }))]
    #[case::strings_not_an_object(json!({
        "schema_version": 1, "language": "es", "name": "Spanish",
        "native_name": "Español", "version": "1.0.0", "strings": "Inicio"
    }))]
    #[case::unknown_extra_key(json!({
        "schema_version": 1, "language": "es", "name": "Spanish",
        "native_name": "Español", "version": "1.0.0", "strings": {}, "extra": true
    }))]
    #[case::missing_strings(json!({
        "schema_version": 1, "language": "es", "name": "Spanish",
        "native_name": "Español", "version": "1.0.0"
    }))]
    #[case::bare_array(json!(["not", "an", "envelope"]))]
    fn envelope_validation_rejects_bad_shapes(#[case] payload: Value) {
        let bytes = serde_json::to_vec(&payload).unwrap();
        assert!(parse_and_validate(&bytes, "es", Some("1.0.0")).is_err());
    }

    #[test]
    fn envelope_validation_rejects_non_json_and_oversize() {
        assert!(parse_and_validate(b"not json", "es", None).is_err());
        let huge = vec![b'x'; MAX_PACK_BYTES + 1];
        assert!(parse_and_validate(&huge, "es", None).is_err());
    }

    // -- cache-dir derivation -----------------------------------------------

    #[rstest]
    #[case("/srv/night/db.sqlite", "/srv/night/i18n")]
    #[case("data/nightlio.db", "data/i18n")]
    #[case("db.sqlite", "./i18n")]
    #[case("/nightlio.db", "/i18n")]
    fn cache_dir_mirrors_the_rate_limit_derivation(#[case] db: &str, #[case] expected: &str) {
        assert_eq!(cache_dir_for(db), PathBuf::from(expected));
    }
}
