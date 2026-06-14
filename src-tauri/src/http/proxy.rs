//! HTTP handler for generic OpenAI-compatible LLM proxy.
//!
//! The browser doesn't hold the user's API key. It sends the request body
//! to this endpoint and the server forwards it to the user's configured
//! provider. Supports both non-streaming (200 JSON response) and streaming
//! (202 + SSE `proxy:token` events).

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use tower_cookies::Cookies;

use crate::core::events::EventSink;
use crate::http::auth::AuthUser;
use crate::http::error::ApiError;
use crate::http::session_event_sink::SessionEventSink;
use crate::http::AppState;

pub fn proxy_router() -> Router<AppState> {
    Router::new().route("/api/v1/proxy/llm", post(proxy))
}

#[derive(Debug, Deserialize)]
struct ProxyRequest {
    #[serde(default)]
    stream: bool,
    body: serde_json::Value,
}

async fn proxy(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    cookies: Cookies,
    Json(req): Json<ProxyRequest>,
) -> Result<axum::response::Response, ApiError> {
    use axum::response::IntoResponse;

    // Load user's provider config.
    let user_config = state
        .user_data
        .load_config(&user.id)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let provider_cfg = crate::http::chat::provider_config_from_user(&user_config)
        .map_err(|msg| ApiError::bad_request("LLM_PROVIDER_NOT_CONFIGURED", msg))?;

    if !req.stream {
        // Non-streaming path: just await and return the upstream JSON.
        let result = state
            .llm_client
            .chat_completion(&provider_cfg, req.body)
            .await?;
        return Ok((StatusCode::OK, Json(result)).into_response());
    }

    // Streaming path: spawn, return 202 + request_id.
    let session_id = cookies
        .get(&state.config.session_cookie_name)
        .map(|c| c.value().to_string())
        .ok_or_else(ApiError::unauthenticated)?;
    let sink = Arc::new(SessionEventSink::new(state.session_bus.clone(), session_id))
        as Arc<dyn EventSink + Send + Sync + 'static>;

    let request_id = crate::http::chat::uuid_short();
    let client = state.llm_client.clone();
    let request_id_owned = request_id.clone();

    let tagged_sink = TaggedSink {
        inner: sink.clone(),
        request_id: request_id_owned.clone(),
    };

    tokio::spawn(async move {
        let result = client
            .chat_completion_stream(&provider_cfg, req.body, &tagged_sink)
            .await;
        match result {
            Ok(()) => {
                sink.emit("proxy:done", serde_json::json!({ "request_id": request_id_owned }));
            }
            Err(e) => {
                sink.emit(
                    "proxy:error",
                    serde_json::json!({
                        "request_id": request_id_owned,
                        "error": format!("{e}"),
                    }),
                );
            }
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "request_id": request_id })),
    )
        .into_response())
}

/// EventSink wrapper that:
/// 1. Adds `request_id` to every emitted payload (so the client can
///    correlate token streams from concurrent proxy calls).
/// 2. Renames `chat:token` → `proxy:token` (LlmClient emits `chat:token`
///    as its convention; we relabel for the proxy endpoint).
struct TaggedSink {
    inner: Arc<dyn EventSink + Send + Sync + 'static>,
    request_id: String,
}

impl EventSink for TaggedSink {
    fn emit(&self, event_type: &str, payload: serde_json::Value) {
        let mut p = payload;
        if let Some(obj) = p.as_object_mut() {
            obj.insert(
                "request_id".to_string(),
                serde_json::Value::String(self.request_id.clone()),
            );
        }
        let renamed = if event_type == "chat:token" {
            "proxy:token"
        } else {
            event_type
        };
        self.inner.emit(renamed, p);
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::http::Request;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use crate::auth::sessions::Sessions;
    use crate::auth::users::{hash_password, Users};
    use crate::config::ServerConfig;
    use crate::http::{main_router, AppState};
    use crate::storage::session_bus::SessionBus;
    use crate::storage::user_data::UserData;

    fn build_state(username: &str, password: &str) -> (TempDir, AppState) {
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

        let projects_root = dir.path().join("projects");
        std::fs::create_dir_all(&projects_root).unwrap();

        let cfg = ServerConfig {
            port: 8080,
            projects_root,
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

    async fn do_login(app: axum::Router, username: &str, password: &str) -> String {
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
        assert_eq!(resp.status(), 200, "login must succeed");
        resp.headers()
            .get(axum::http::header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    }

    // ── proxy_without_cookie_is_401 ───────────────────────────────────────────

    #[tokio::test]
    async fn proxy_without_cookie_is_401() {
        let (_dir, state) = build_state("alice", "pw");
        let app = main_router(state);
        let body = serde_json::json!({
            "stream": false,
            "body": { "messages": [] }
        })
        .to_string();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/proxy/llm")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
    }

    // ── proxy_without_llm_config_returns_400_with_correct_code ───────────────

    #[tokio::test]
    async fn proxy_without_llm_config_returns_400_with_correct_code() {
        let (_dir, state) = build_state("alice", "pw");
        let app = main_router(state.clone());
        let cookie = do_login(app.clone(), "alice", "pw").await;

        // No LLM config saved for alice → should get 400.
        let body = serde_json::json!({
            "stream": false,
            "body": { "messages": [{"role": "user", "content": "hi"}] }
        })
        .to_string();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/proxy/llm")
                    .header("content-type", "application/json")
                    .header("cookie", cookie)
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
        let bytes = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"]["code"], "LLM_PROVIDER_NOT_CONFIGURED");
    }

    // ── proxy_non_streaming_with_mock_upstream_returns_body ───────────────────

    #[tokio::test]
    async fn proxy_non_streaming_with_mock_upstream_returns_body() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"answer":42}"#)
            .create_async()
            .await;

        let (_dir, state) = build_state("alice", "pw");

        // Point alice's LLM config at the mockito server.
        state
            .user_data
            .save_config(
                "alice",
                &serde_json::json!({
                    "llm": {
                        "base_url": server.url(),
                        "model": "test-model"
                    }
                }),
            )
            .unwrap();

        let app = main_router(state.clone());
        let cookie = do_login(app.clone(), "alice", "pw").await;

        let body = serde_json::json!({
            "stream": false,
            "body": { "messages": [{"role": "user", "content": "hi"}] }
        })
        .to_string();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/proxy/llm")
                    .header("content-type", "application/json")
                    .header("cookie", cookie)
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let bytes = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["answer"], 42);
    }

    // ── proxy_streaming_returns_202_and_request_id ────────────────────────────

    #[tokio::test]
    async fn proxy_streaming_returns_202_and_request_id() {
        let (_dir, state) = build_state("alice", "pw");

        // Pre-populate a valid LLM config for alice.
        state
            .user_data
            .save_config(
                "alice",
                &serde_json::json!({
                    "llm": {
                        "base_url": "http://127.0.0.1:1",  // unreachable — we only check 202
                        "model": "test-model"
                    }
                }),
            )
            .unwrap();

        let app = main_router(state.clone());
        let cookie = do_login(app.clone(), "alice", "pw").await;

        let body = serde_json::json!({
            "stream": true,
            "body": { "messages": [{"role": "user", "content": "hi"}] }
        })
        .to_string();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/proxy/llm")
                    .header("content-type", "application/json")
                    .header("cookie", cookie)
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 202);
        let bytes = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            v["request_id"].as_str().is_some(),
            "response must contain a request_id string"
        );
    }
}
