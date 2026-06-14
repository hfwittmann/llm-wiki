//! HTTP layer for the LLM Wiki server.
//!
//! Modules:
//! - `error`: uniform error response type used by every handler.
//! - `auth`: login, logout, whoami, session middleware (Task 2.7+).
//! - `events`: per-session SSE stream (Task 2.9).
//! - `embed`: rust-embed frontend serving (Task 2.10).
//! - `error_mapping`: `From<XError> for ApiError` impls for every `core::*` error (Task 4.1).
//! - `session_event_sink`: `SessionEventSink` — routes `EventSink::emit` to the session's SSE stream (Task 4.1).

pub mod error;
pub mod auth;
pub mod events;
pub mod embed;
pub mod error_mapping;
pub mod session_event_sink;
pub mod projects;
pub mod sources;
pub mod wiki;
pub mod chat;
pub mod config;
pub mod fs_browser;
pub mod files;
pub mod proxy;

use std::sync::Arc;

use axum::middleware::from_fn_with_state;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use tower_cookies::CookieManagerLayer;

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
    pub llm_client: Arc<crate::core::llm_client::LlmClient>,
}

pub fn main_router(state: AppState) -> Router {
    let authed = Router::new()
        .route("/api/v1/health", get(health))
        .merge(auth::auth_router())
        .merge(projects::projects_router())
        .merge(sources::sources_router())
        .merge(wiki::wiki_router())
        .merge(chat::chat_router())
        .merge(config::config_router())
        .merge(fs_browser::fs_browser_router())
        .merge(files::files_router())
        .merge(proxy::proxy_router())
        .route("/api/v1/events", get(events::events_handler))
        // Session middleware: extract cookie, inject User if valid.
        .route_layer(from_fn_with_state(state.clone(), auth::session_middleware))
        .with_state(state.clone());

    Router::new()
        .merge(authed)
        .fallback(embed::spa_fallback)
        // Cookie layer needs to be outermost so cookies are parsed before
        // the session middleware runs.
        .layer(CookieManagerLayer::new())
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

/// Router for the legacy 127.0.0.1:19828 listener: same handlers as
/// `main_router` but without the session middleware. Phase 4 will narrow
/// this to the agent-facing subset.
pub fn legacy_router(state: AppState) -> Router {
    let r = Router::new()
        .route("/api/v1/health", get(health))
        .with_state(state);
    Router::new().merge(r).layer(CookieManagerLayer::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use tower::ServiceExt; // for `oneshot`

    use crate::auth::users::hash_password;

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
            llm_client: Arc::new(crate::core::llm_client::LlmClient::new()),
        };
        (dir, state)
    }

    fn build_state_with_user(
        username: &str,
        password: &str,
    ) -> (TempDir, AppState) {
        let dir = TempDir::new().unwrap();
        let hash = hash_password(password).unwrap();
        let users_path = dir.path().join("users.toml");
        std::fs::write(
            &users_path,
            format!("[users.{username}]\npassword_hash = \"{hash}\"\n"),
        )
        .unwrap();
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
            llm_client: Arc::new(crate::core::llm_client::LlmClient::new()),
        };
        (dir, state)
    }

    fn extract_set_cookie(resp: &axum::response::Response) -> String {
        resp.headers()
            .get(axum::http::header::SET_COOKIE)
            .expect("set-cookie present")
            .to_str()
            .unwrap()
            .to_string()
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

    #[tokio::test]
    async fn whoami_without_cookie_is_401() {
        let (_dir, state) = build_state_with_user("alice", "pw");
        let app = main_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/auth/whoami")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
    }

    #[tokio::test]
    async fn login_with_wrong_password_is_401() {
        let (_dir, state) = build_state_with_user("alice", "pw");
        let app = main_router(state);
        let body = r#"{"username":"alice","password":"wrong"}"#;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "INVALID_CREDENTIALS");
    }

    #[tokio::test]
    async fn login_then_whoami_with_cookie_works() {
        let (_dir, state) = build_state_with_user("alice", "pw");
        let app = main_router(state.clone());

        let body = r#"{"username":"alice","password":"pw"}"#;
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let set_cookie = extract_set_cookie(&resp);
        assert!(set_cookie.contains("test_session="));
        assert!(set_cookie.contains("HttpOnly"));
        assert!(set_cookie.contains("SameSite=Lax"));
        let cookie_value = set_cookie.split(';').next().unwrap().to_string();

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/auth/whoami")
                    .header("cookie", cookie_value)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["user_id"], "alice");
        assert_eq!(v["username"], "alice");
        assert!(v["recently_opened"].is_array());
    }

    #[tokio::test]
    async fn logout_invalidates_session_immediately() {
        let (_dir, state) = build_state_with_user("alice", "pw");
        let app = main_router(state.clone());

        // log in
        let body = r#"{"username":"alice","password":"pw"}"#;
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let cookie = extract_set_cookie(&resp).split(';').next().unwrap().to_string();

        // log out
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/logout")
                    .header("cookie", &cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 204);

        // whoami with the now-revoked cookie → 401
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/auth/whoami")
                    .header("cookie", &cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
    }

    #[tokio::test]
    async fn events_without_cookie_is_401() {
        let (_dir, state) = build_state_with_user("alice", "pw");
        let app = main_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/events")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
    }

    #[tokio::test]
    async fn events_with_valid_session_registers_in_bus() {
        let (_dir, state) = build_state_with_user("alice", "pw");
        let app = main_router(state.clone());

        // Log in to get a cookie
        let body = r#"{"username":"alice","password":"pw"}"#;
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let cookie = extract_set_cookie(&resp).split(';').next().unwrap().to_string();

        // Spawn the SSE request in a task and let it run far enough to register.
        // We hold the response alive so the SSE body stream (and its guard) is
        // not dropped before we can observe the registration.
        let bus = state.session_bus.clone();
        let app_cloned = app.clone();
        let cookie_cloned = cookie.clone();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(async move {
            let resp = app_cloned
                .oneshot(
                    Request::builder()
                        .uri("/api/v1/events")
                        .header("cookie", cookie_cloned)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            // Signal that we have the response, then wait for the test to finish
            // checking before we drop the response (and its SSE body stream).
            let _ = tx.send(());
            // Hold resp alive until the receiving end drops rx (test done or timeout).
            let _resp = resp;
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        });

        // Wait for the spawned task to signal it has received the response.
        let _ = rx.await;

        assert!(bus.registered_count() >= 1, "session was not registered in bus");

        handle.abort();
    }

    #[tokio::test]
    async fn projects_list_without_cookie_is_401() {
        let (_dir, state) = build_state_with_user("alice", "pw");
        let app = main_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/projects/list")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
    }

    #[tokio::test]
    async fn projects_open_rejects_path_traversal() {
        let (_dir, state) = build_state_with_user("alice", "pw");
        let app = main_router(state.clone());

        // Log in
        let body = r#"{"username":"alice","password":"pw"}"#;
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let cookie = extract_set_cookie(&resp).split(';').next().unwrap().to_string();

        // open with .. should fail
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/projects/open")
                    .header("content-type", "application/json")
                    .header("cookie", cookie)
                    .body(axum::body::Body::from(r#"{"path": "../etc"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "PATH_ESCAPE");
    }

    #[tokio::test]
    async fn projects_list_returns_empty_when_root_empty() {
        let (dir, state) = build_state_with_user("alice", "pw");
        // Create the projects_root directory (tests use "./projects" which may not exist;
        // we use a real temp dir here by overriding the state).
        let projects_root = dir.path().join("projects");
        std::fs::create_dir_all(&projects_root).unwrap();
        let cfg = crate::config::ServerConfig {
            port: 8080,
            projects_root,
            data_root: dir.path().to_path_buf(),
            legacy_19828_enabled: true,
            session_cookie_name: "test_session".into(),
        };
        let state = AppState {
            config: std::sync::Arc::new(cfg),
            ..state
        };
        let app = main_router(state.clone());

        let body = r#"{"username":"alice","password":"pw"}"#;
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let cookie = extract_set_cookie(&resp).split(';').next().unwrap().to_string();

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/projects/list")
                    .header("cookie", cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let arr: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert!(arr.is_empty());
    }

    #[tokio::test]
    async fn unknown_route_falls_back_to_index_html() {
        let (_dir, state) = build_state_with_user("alice", "pw");
        let app = main_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/some/spa/route")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 16384).await.unwrap();
        let s = String::from_utf8_lossy(&body);
        assert!(s.contains("<!DOCTYPE html>"));
        assert!(s.contains("<html"));
    }

    // ── Wiki handler helpers ──────────────────────────────────────────────────

    /// Build a state whose `projects_root` points at a real temp directory,
    /// and create one valid project under it (`proj/schema.md` + `proj/wiki/`).
    fn build_state_with_user_and_projects_root(
        username: &str,
        password: &str,
    ) -> (TempDir, AppState, PathBuf) {
        let dir = TempDir::new().unwrap();
        let hash = crate::auth::users::hash_password(password).unwrap();
        let users_path = dir.path().join("users.toml");
        std::fs::write(
            &users_path,
            format!("[users.{username}]\npassword_hash = \"{hash}\"\n"),
        )
        .unwrap();
        let users = crate::auth::users::Users::load(&users_path).unwrap();
        let sessions = crate::auth::sessions::Sessions::open(&dir.path().join("sessions")).unwrap();
        let user_data = crate::storage::user_data::UserData::new(dir.path().to_path_buf());
        let bus = crate::storage::session_bus::SessionBus::new();

        let projects_root = dir.path().join("projects");
        std::fs::create_dir_all(&projects_root).unwrap();

        // Create a minimal valid project.
        let proj_dir = projects_root.join("proj");
        std::fs::create_dir_all(proj_dir.join("wiki")).unwrap();
        std::fs::write(proj_dir.join("schema.md"), b"# Schema\n").unwrap();
        std::fs::write(proj_dir.join("wiki/foo.md"), b"# Foo\n\nHello world.\n").unwrap();

        let cfg = crate::config::ServerConfig {
            port: 8080,
            projects_root: projects_root.clone(),
            data_root: dir.path().to_path_buf(),
            legacy_19828_enabled: true,
            session_cookie_name: "test_session".into(),
        };
        let state = AppState {
            users: std::sync::Arc::new(users),
            sessions,
            user_data,
            session_bus: bus,
            config: std::sync::Arc::new(cfg),
            llm_client: std::sync::Arc::new(crate::core::llm_client::LlmClient::new()),
        };
        (dir, state, projects_root)
    }

    async fn login(app: axum::Router, username: &str, password: &str) -> String {
        let body = format!(r#"{{"username":"{username}","password":"{password}"}}"#);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "login failed");
        extract_set_cookie(&resp).split(';').next().unwrap().to_string()
    }

    // ── Wiki tests ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn wiki_page_read_without_cookie_is_401() {
        let (_dir, state, _root) = build_state_with_user_and_projects_root("alice", "pw");
        let app = main_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/wiki/page?project_path=proj&page_path=wiki/foo.md")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
    }

    #[tokio::test]
    async fn wiki_page_read_returns_content_and_etag() {
        let (_dir, state, _root) = build_state_with_user_and_projects_root("alice", "pw");
        let app = main_router(state.clone());
        let cookie = login(app.clone(), "alice", "pw").await;

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/wiki/page?project_path=proj&page_path=wiki/foo.md")
                    .header("cookie", cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        // ETag response header must be present.
        let etag_header = resp
            .headers()
            .get(axum::http::header::ETAG)
            .expect("ETag header present")
            .to_str()
            .unwrap()
            .to_string();
        assert!(etag_header.starts_with('"'), "ETag should be quoted");
        assert!(etag_header.ends_with('"'), "ETag should be quoted");

        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v["content"].as_str().unwrap().contains("Hello world"));
        let etag_in_body = v["etag"].as_str().unwrap();
        assert_eq!(etag_in_body.len(), 16, "etag must be 16 hex chars");
        // Body etag must match the stripped header value.
        let stripped = etag_header.trim_matches('"');
        assert_eq!(etag_in_body, stripped);
    }

    #[tokio::test]
    async fn wiki_page_write_without_if_match_is_400() {
        let (_dir, state, _root) = build_state_with_user_and_projects_root("alice", "pw");
        let app = main_router(state.clone());
        let cookie = login(app.clone(), "alice", "pw").await;

        let body = serde_json::json!({
            "project_path": "proj",
            "page_path": "wiki/foo.md",
            "content": "# Updated\n"
        })
        .to_string();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/v1/wiki/page")
                    .header("content-type", "application/json")
                    .header("cookie", cookie)
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "BAD_REQUEST");
    }

    #[tokio::test]
    async fn wiki_page_write_with_matching_etag_succeeds() {
        let (_dir, state, _root) = build_state_with_user_and_projects_root("alice", "pw");
        let app = main_router(state.clone());
        let cookie = login(app.clone(), "alice", "pw").await;

        // First read to get the current ETag.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/wiki/page?project_path=proj&page_path=wiki/foo.md")
                    .header("cookie", &cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let etag_header = resp
            .headers()
            .get(axum::http::header::ETAG)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // PUT with the correct If-Match.
        let body = serde_json::json!({
            "project_path": "proj",
            "page_path": "wiki/foo.md",
            "content": "# Updated\n\nNew content.\n"
        })
        .to_string();
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/v1/wiki/page")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .header("if-match", &etag_header)
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let new_etag_header = resp
            .headers()
            .get(axum::http::header::ETAG)
            .expect("new ETag in response")
            .to_str()
            .unwrap()
            .to_string();

        // Re-read: content and etag must be updated.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/wiki/page?project_path=proj&page_path=wiki/foo.md")
                    .header("cookie", &cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v["content"].as_str().unwrap().contains("New content"));
        // ETag must have changed from the original read.
        assert_ne!(new_etag_header, etag_header);
    }

    #[tokio::test]
    async fn wiki_page_write_with_stale_etag_returns_412() {
        let (_dir, state, _root) = build_state_with_user_and_projects_root("alice", "pw");
        let app = main_router(state.clone());
        let cookie = login(app.clone(), "alice", "pw").await;

        let body = serde_json::json!({
            "project_path": "proj",
            "page_path": "wiki/foo.md",
            "content": "# Conflicting update\n"
        })
        .to_string();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/v1/wiki/page")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .header("if-match", "\"0000000000000000\"") // wrong etag
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 412);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "WIKI_PAGE_STALE");
        assert!(v["error"]["details"]["current_etag"].is_string());
    }

    #[tokio::test]
    async fn wiki_page_rejects_path_traversal() {
        let (_dir, state, _root) = build_state_with_user_and_projects_root("alice", "pw");
        let app = main_router(state.clone());
        let cookie = login(app.clone(), "alice", "pw").await;

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/wiki/page?project_path=proj&page_path=../../../etc/passwd")
                    .header("cookie", cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "PATH_ESCAPE");
    }

    // ── Config tests (Task 4.6) ───────────────────────────────────────────────

    #[tokio::test]
    async fn config_get_without_cookie_is_401() {
        let (_dir, state) = build_state_with_user("alice", "pw");
        let app = main_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/config")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
    }

    #[tokio::test]
    async fn config_get_for_new_user_returns_empty_object() {
        let (_dir, state) = build_state_with_user("alice", "pw");
        let app = main_router(state.clone());
        let cookie = login(app.clone(), "alice", "pw").await;

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/config")
                    .header("cookie", cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v.is_object());
        assert_eq!(v.as_object().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn config_put_roundtrips_to_get() {
        let (_dir, state) = build_state_with_user("alice", "pw");
        let app = main_router(state.clone());
        let cookie = login(app.clone(), "alice", "pw").await;

        // PUT a config value.
        let payload = serde_json::json!({ "llm": { "model": "gpt-4o" } }).to_string();
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/v1/config")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(axum::body::Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        // GET must return the same value.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/config")
                    .header("cookie", &cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["llm"]["model"], "gpt-4o");
    }

    #[tokio::test]
    async fn config_isolation_alice_does_not_see_bob() {
        let dir = TempDir::new().unwrap();
        // Build a state with two users.
        let users_path = dir.path().join("users.toml");
        let alice_hash = hash_password("pw").unwrap();
        let bob_hash = hash_password("pw").unwrap();
        std::fs::write(
            &users_path,
            format!(
                "[users.alice]\npassword_hash = \"{alice_hash}\"\n\
                 [users.bob]\npassword_hash = \"{bob_hash}\"\n"
            ),
        )
        .unwrap();
        let users = crate::auth::users::Users::load(&users_path).unwrap();
        let sessions = crate::auth::sessions::Sessions::open(&dir.path().join("sessions")).unwrap();
        let user_data = UserData::new(dir.path().to_path_buf());
        let bus = crate::storage::session_bus::SessionBus::new();
        let projects_root = dir.path().join("projects");
        std::fs::create_dir_all(&projects_root).unwrap();
        let cfg = crate::config::ServerConfig {
            port: 8080,
            projects_root,
            data_root: dir.path().to_path_buf(),
            legacy_19828_enabled: true,
            session_cookie_name: "test_session".into(),
        };
        let state = AppState {
            users: std::sync::Arc::new(users),
            sessions,
            user_data,
            session_bus: bus,
            config: std::sync::Arc::new(cfg),
            llm_client: std::sync::Arc::new(crate::core::llm_client::LlmClient::new()),
        };

        let app = main_router(state.clone());
        let alice_cookie = login(app.clone(), "alice", "pw").await;
        let bob_cookie = login(app.clone(), "bob", "pw").await;

        // Alice saves her config.
        let payload = serde_json::json!({ "who": "alice" }).to_string();
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/v1/config")
                    .header("content-type", "application/json")
                    .header("cookie", &alice_cookie)
                    .body(axum::body::Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        // Bob reads config — must be empty, not alice's.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/config")
                    .header("cookie", &bob_cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // Bob's config must NOT contain alice's "who" key.
        assert!(v.get("who").is_none(), "bob must not see alice's config");
    }

    // ── fs_browser tests (Task 4.7) ───────────────────────────────────────────

    /// Build a state with a real temp projects_root (and a valid user).
    fn build_state_with_real_projects_root(
        username: &str,
        password: &str,
    ) -> (TempDir, AppState, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let hash = hash_password(password).unwrap();
        let users_path = dir.path().join("users.toml");
        std::fs::write(
            &users_path,
            format!("[users.{username}]\npassword_hash = \"{hash}\"\n"),
        )
        .unwrap();
        let users = crate::auth::users::Users::load(&users_path).unwrap();
        let sessions =
            crate::auth::sessions::Sessions::open(&dir.path().join("sessions")).unwrap();
        let user_data = UserData::new(dir.path().to_path_buf());
        let bus = crate::storage::session_bus::SessionBus::new();
        let projects_root = dir.path().join("projects");
        std::fs::create_dir_all(&projects_root).unwrap();
        let cfg = crate::config::ServerConfig {
            port: 8080,
            projects_root: projects_root.clone(),
            data_root: dir.path().to_path_buf(),
            legacy_19828_enabled: true,
            session_cookie_name: "test_session".into(),
        };
        let state = AppState {
            users: std::sync::Arc::new(users),
            sessions,
            user_data,
            session_bus: bus,
            config: std::sync::Arc::new(cfg),
            llm_client: std::sync::Arc::new(crate::core::llm_client::LlmClient::new()),
        };
        (dir, state, projects_root)
    }

    #[tokio::test]
    async fn fs_list_without_cookie_is_401() {
        let (_dir, state, _root) = build_state_with_real_projects_root("alice", "pw");
        let app = main_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/fs/list")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
    }

    #[tokio::test]
    async fn fs_list_root_returns_entries() {
        let (_dir, state, projects_root) =
            build_state_with_real_projects_root("alice", "pw");
        // Create a project subdir (with schema.md so is_project = true) and a plain dir.
        let proj_dir = projects_root.join("myproj");
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::write(proj_dir.join("schema.md"), b"# Schema\n").unwrap();
        std::fs::create_dir_all(projects_root.join("plain_dir")).unwrap();

        let app = main_router(state.clone());
        let cookie = login(app.clone(), "alice", "pw").await;

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/fs/list")
                    .header("cookie", cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let entries = v["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);

        let proj_entry = entries.iter().find(|e| e["name"] == "myproj").unwrap();
        assert_eq!(proj_entry["is_dir"], true);
        assert_eq!(proj_entry["is_project"], true);

        let plain_entry = entries.iter().find(|e| e["name"] == "plain_dir").unwrap();
        assert_eq!(plain_entry["is_dir"], true);
        assert_eq!(plain_entry["is_project"], false);
    }

    #[tokio::test]
    async fn fs_list_path_escape_rejected() {
        let (_dir, state, _root) = build_state_with_real_projects_root("alice", "pw");
        let app = main_router(state.clone());
        let cookie = login(app.clone(), "alice", "pw").await;

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/fs/list?path=../etc")
                    .header("cookie", cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "PATH_ESCAPE");
    }

    #[tokio::test]
    async fn fs_mkdir_creates_subdir_then_list_shows_it() {
        let (_dir, state, _root) = build_state_with_real_projects_root("alice", "pw");
        let app = main_router(state.clone());
        let cookie = login(app.clone(), "alice", "pw").await;

        // Create a new subdirectory via mkdir.
        let body = serde_json::json!({ "path": "newdir/sub" }).to_string();
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/fs/mkdir")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 201);

        // List root — must include "newdir".
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/fs/list")
                    .header("cookie", &cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let entries = v["entries"].as_array().unwrap();
        assert!(
            entries.iter().any(|e| e["name"] == "newdir"),
            "newdir must appear in listing"
        );
    }

    #[tokio::test]
    async fn fs_mkdir_rejects_path_traversal() {
        let (_dir, state, _root) = build_state_with_real_projects_root("alice", "pw");
        let app = main_router(state.clone());
        let cookie = login(app.clone(), "alice", "pw").await;

        let body = serde_json::json!({ "path": "../evil" }).to_string();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/fs/mkdir")
                    .header("content-type", "application/json")
                    .header("cookie", cookie)
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "PATH_ESCAPE");
    }

    // ── files/raw tests (Task 4.8) ────────────────────────────────────────────

    #[tokio::test]
    async fn files_raw_without_cookie_is_401() {
        let (_dir, state, _root) = build_state_with_real_projects_root("alice", "pw");
        let app = main_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/files/raw?project_path=proj&path=file.md")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
    }

    #[tokio::test]
    async fn files_raw_returns_content_with_correct_content_type() {
        let (_dir, state, projects_root) =
            build_state_with_real_projects_root("alice", "pw");
        // Create a minimal project with a markdown file.
        let proj_dir = projects_root.join("proj");
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::write(proj_dir.join("schema.md"), b"# Schema\n").unwrap();
        std::fs::write(proj_dir.join("README.md"), b"# Hello\n").unwrap();

        let app = main_router(state.clone());
        let cookie = login(app.clone(), "alice", "pw").await;

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/files/raw?project_path=proj&path=README.md")
                    .header("cookie", cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let ct = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .expect("content-type present")
            .to_str()
            .unwrap()
            .to_string();
        assert!(ct.contains("markdown") || ct.contains("text"), "expected text or markdown content-type, got {ct}");
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        assert!(body.starts_with(b"# Hello"));
    }

    #[tokio::test]
    async fn files_raw_rejects_path_traversal() {
        let (_dir, state, projects_root) =
            build_state_with_real_projects_root("alice", "pw");
        // Create the project dir so the project_path resolves.
        let proj_dir = projects_root.join("proj");
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::write(proj_dir.join("schema.md"), b"# Schema\n").unwrap();

        let app = main_router(state.clone());
        let cookie = login(app.clone(), "alice", "pw").await;

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/files/raw?project_path=proj&path=../../../etc/passwd")
                    .header("cookie", cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "PATH_ESCAPE");
    }

    #[tokio::test]
    async fn files_raw_returns_404_for_missing_file() {
        let (_dir, state, projects_root) =
            build_state_with_real_projects_root("alice", "pw");
        let proj_dir = projects_root.join("proj");
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::write(proj_dir.join("schema.md"), b"# Schema\n").unwrap();

        let app = main_router(state.clone());
        let cookie = login(app.clone(), "alice", "pw").await;

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/files/raw?project_path=proj&path=nonexistent.md")
                    .header("cookie", cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "NOT_FOUND");
    }
}
