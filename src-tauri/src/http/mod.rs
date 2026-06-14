//! HTTP layer for the LLM Wiki server.
//!
//! Modules:
//! - `error`: uniform error response type used by every handler.
//! - `auth`: login, logout, whoami, session middleware (Task 2.7+).
//! - `events`: per-session SSE stream (Task 2.9).
//! - `embed`: rust-embed frontend serving (Task 2.10).

pub mod error;
pub mod auth;
pub mod events;

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
}

pub fn main_router(state: AppState) -> Router {
    let authed = Router::new()
        .route("/api/v1/health", get(health))
        .merge(auth::auth_router())
        .route("/api/v1/events", get(events::events_handler))
        // Session middleware: extract cookie, inject User if valid.
        .route_layer(from_fn_with_state(state.clone(), auth::session_middleware))
        .with_state(state.clone());

    Router::new()
        .merge(authed)
        // Cookie layer needs to be outermost so cookies are parsed before
        // the session middleware runs.
        .layer(CookieManagerLayer::new())
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
}
