//! Shared application state passed to every axum handler via
//! `axum::extract::State`.

use std::sync::Arc;

use crate::config::Config;
use crate::db::DbPool;

/// Cloned per-request by axum; both fields are cheap handles.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub pool: DbPool,
}

impl AppState {
    pub fn new(config: Config, pool: DbPool) -> Self {
        AppState {
            config: Arc::new(config),
            pool,
        }
    }
}
