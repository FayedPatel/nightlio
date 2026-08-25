//! Shared application state passed to every axum handler via
//! `axum::extract::State`.

use std::sync::Arc;

use crate::config::Config;
use crate::db::DbHandle;
use crate::i18n::I18nStore;

/// Cloned per-request by axum; every field is a cheap handle.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: DbHandle,
    /// Language-pack store for the unauthenticated `/api/i18n` family
    /// (v0.6.0). Construction never touches the network — the store loads
    /// its disk cache synchronously and fetches lazily on demand.
    pub i18n: Arc<I18nStore>,
}

impl AppState {
    pub fn new(config: Config, db: DbHandle) -> Self {
        let i18n = Arc::new(I18nStore::new(&config));
        AppState {
            config: Arc::new(config),
            db,
            i18n,
        }
    }
}
