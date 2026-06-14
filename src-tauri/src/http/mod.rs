//! HTTP layer for the LLM Wiki server.
//!
//! Modules:
//! - `error`: uniform error response type used by every handler.
//! - `auth`: login, logout, whoami, session middleware (Task 2.7+).
//! - `events`: per-session SSE stream (Task 2.9).
//! - `embed`: rust-embed frontend serving (Task 2.10).

pub mod error;

use std::sync::Arc;

use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use crate::auth::sessions::Sessions;
use crate::auth::users::Users;
use crate::config::ServerConfig;
use crate::storage::session_bus::SessionBus;
use crate::storage::user_data::UserData;

#[derive(Clone)]
pub struct AppState {
    pub users: Arc<Users>,
    pub sessions: Sessions,
    pub user_data: UserData,
    pub session_bus: SessionBus,
    pub config: Arc<ServerConfig>,
}

/// The main authenticated router. Auth middleware is layered on by the
/// caller in `bin/llm_wiki_server.rs` so the same router can be mounted
/// twice — once with auth, once without (legacy 127.0.0.1:19828).
pub fn main_router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use tower::ServiceExt; // for `oneshot`

    fn build_state() -> (TempDir, AppState) {
        let dir = TempDir::new().unwrap();
        let users_path = dir.path().join("users.toml");
        std::fs::write(&users_path, "").unwrap();
        let users = Users::load(&users_path).unwrap();
        let sessions = Sessions::open(&dir.path().join("sessions")).unwrap();
        let user_data = UserData::new(dir.path().to_path_buf());
        let bus = SessionBus::new();
        let cfg = ServerConfig {
            port: 8080,
            projects_root: PathBuf::from("./projects"),
            data_root: dir.path().to_path_buf(),
            legacy_19828_enabled: true,
            session_cookie_name: "test_session".into(),
        };
        let state = AppState {
            users: Arc::new(users),
            sessions,
            user_data,
            session_bus: bus,
            config: Arc::new(cfg),
        };
        (dir, state)
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let (_dir, state) = build_state();
        let app = main_router(state);
        let resp = app
            .oneshot(Request::builder().uri("/api/v1/health").body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["status"], "ok");
    }
}
