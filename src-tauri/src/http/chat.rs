//! HTTP handlers for chat — conversation list, load, and send (token streaming).
//!
//! Endpoints:
//! - GET  /api/v1/chat/conversations?project_path=<rel>
//!   → `{ "conversations": [{"id": "...", "modified_unix": ...}, ...] }`
//! - GET  /api/v1/chat/conversation?project_path=<rel>&conversation_id=<id>
//!   → the raw conversation JSON blob stored by `UserData`
//! - POST /api/v1/chat/send
//!   → 202 `{ "request_id": "..." }`, streams tokens via `chat:token` SSE
//!     events; emits `chat:done` or `chat:error` on completion.
//!
//! # User-config shape for LLM provider
//!
//! The handler reads `user_config["llm"]` expecting:
//! ```json
//! {
//!   "llm": {
//!     "base_url":      "https://api.openai.com",
//!     "api_key":       "sk-...",
//!     "model":         "gpt-4o-mini",
//!     "extra_headers": { "X-Custom": "value" }  // optional
//!   }
//! }
//! ```
//! If the `llm` key is absent or missing `base_url` / `model`, the handler
//! returns 400 with code `LLM_PROVIDER_NOT_CONFIGURED`.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use tower_cookies::Cookies;

use crate::core::events::EventSink;
use crate::core::llm_client::ProviderConfig;
use crate::http::auth::AuthUser;
use crate::http::error::ApiError;
use crate::http::session_event_sink::SessionEventSink;
use crate::http::AppState;
use crate::storage::paths::resolve_under;

pub fn chat_router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/chat/conversations", get(list_conversations))
        .route("/api/v1/chat/conversation", get(load_conversation))
        .route("/api/v1/chat/send", post(send))
}

// ── list_conversations ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ListConvQuery {
    project_path: String,
}

async fn list_conversations(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<ListConvQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let project_root =
        resolve_under(&state.config.projects_root, &q.project_path).map_err(|e| {
            ApiError::bad_request("PATH_ESCAPE", e.to_string())
                .with_details(serde_json::json!({ "requested": q.project_path }))
        })?;
    let project_id = crate::core::project::project_id_from_canonical_path(&project_root);
    let metas = state
        .user_data
        .list_conversations(&user.id, &project_id)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let json: Vec<serde_json::Value> = metas
        .into_iter()
        .map(|m| serde_json::json!({"id": m.id, "modified_unix": m.modified_unix}))
        .collect();
    Ok(Json(serde_json::json!({ "conversations": json })))
}

// ── load_conversation ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct LoadConvQuery {
    project_path: String,
    conversation_id: String,
}

async fn load_conversation(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<LoadConvQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let project_root =
        resolve_under(&state.config.projects_root, &q.project_path).map_err(|e| {
            ApiError::bad_request("PATH_ESCAPE", e.to_string())
                .with_details(serde_json::json!({ "requested": q.project_path }))
        })?;
    let project_id = crate::core::project::project_id_from_canonical_path(&project_root);
    let conv = state
        .user_data
        .load_conversation(&user.id, &project_id, &q.conversation_id)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(conv))
}

// ── send ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SendRequest {
    project_path: String,
    conversation_id: String,
    messages: Vec<serde_json::Value>,
}

async fn send(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    cookies: Cookies,
    Json(req): Json<SendRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let project_root =
        resolve_under(&state.config.projects_root, &req.project_path).map_err(|e| {
            ApiError::bad_request("PATH_ESCAPE", e.to_string())
                .with_details(serde_json::json!({ "requested": req.project_path }))
        })?;
    let project_id = crate::core::project::project_id_from_canonical_path(&project_root);

    // Load user config and extract LLM provider settings.
    let user_config = state
        .user_data
        .load_config(&user.id)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let provider_cfg = provider_config_from_user(&user_config).map_err(|msg| {
        ApiError::bad_request("LLM_PROVIDER_NOT_CONFIGURED", msg)
    })?;

    // Obtain the session id to route SSE events back to the caller's stream.
    let session_id = cookies
        .get(&state.config.session_cookie_name)
        .map(|c| c.value().to_string())
        .ok_or_else(ApiError::unauthenticated)?;

    let sink = Arc::new(SessionEventSink::new(state.session_bus.clone(), session_id))
        as Arc<dyn EventSink + Send + Sync + 'static>;

    let request_id = uuid_short();

    // Clone everything the background task needs before moving into `spawn`.
    let client = state.llm_client.clone();
    let user_data = state.user_data.clone();
    let user_id = user.id.clone();
    let conversation_id = req.conversation_id.clone();
    let project_id_owned = project_id.clone();
    let new_messages = req.messages.clone();
    let request_id_owned = request_id.clone();

    // Build the body to send upstream — just forward the messages array.
    let llm_body = serde_json::json!({ "messages": req.messages });

    tokio::spawn(async move {
        // Wrap the raw sink in a ContentAccumulator that:
        //   1. Captures the assistant's delta tokens for persistence.
        //   2. Injects `request_id` into every forwarded event so the client
        //      can correlate tokens back to this particular request.
        let assistant_content = Arc::new(parking_lot::Mutex::new(String::new()));
        let acc_sink = Arc::new(ContentAccumulator {
            inner: sink.clone(),
            content: assistant_content.clone(),
            request_id: request_id_owned.clone(),
        });

        let result = client
            .chat_completion_stream(&provider_cfg, llm_body, acc_sink.as_ref())
            .await;

        match result {
            Ok(()) => {
                let final_content = assistant_content.lock().clone();

                // Persist: load existing conversation (empty if new), append
                // the user's messages then the assistant's reply.
                let mut conv = user_data
                    .load_conversation(&user_id, &project_id_owned, &conversation_id)
                    .unwrap_or_else(|_| serde_json::json!({ "messages": [] }));

                if let Some(msgs) = conv
                    .get_mut("messages")
                    .and_then(|v| v.as_array_mut())
                {
                    for m in &new_messages {
                        msgs.push(m.clone());
                    }
                    msgs.push(serde_json::json!({
                        "role": "assistant",
                        "content": final_content
                    }));
                }

                let _ = user_data.save_conversation(
                    &user_id,
                    &project_id_owned,
                    &conversation_id,
                    &conv,
                );
                sink.emit(
                    "chat:done",
                    serde_json::json!({ "request_id": request_id_owned }),
                );
            }
            Err(e) => {
                sink.emit(
                    "chat:error",
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
    ))
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Extract `ProviderConfig` from the user's persisted config JSON.
///
/// Expected shape (see module doc):
/// ```json
/// { "llm": { "base_url": "...", "api_key": "...", "model": "..." } }
/// ```
pub(crate) fn provider_config_from_user(cfg: &serde_json::Value) -> Result<ProviderConfig, String> {
    let llm = cfg.get("llm").ok_or("missing llm config")?;
    let base_url = llm
        .get("base_url")
        .and_then(|v| v.as_str())
        .ok_or("missing llm.base_url")?
        .to_string();
    let api_key = llm
        .get("api_key")
        .and_then(|v| v.as_str())
        .map(String::from);
    let model = llm
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or("missing llm.model")?
        .to_string();
    let extra_headers = llm
        .get("extra_headers")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    Ok(ProviderConfig {
        base_url,
        api_key,
        model,
        extra_headers,
    })
}

/// Generate a short random identifier (11 URL-safe base64 chars ≈ 8 bytes).
pub(crate) fn uuid_short() -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use rand::RngCore;
    let mut bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// An `EventSink` decorator that:
/// - Accumulates `choices[0].delta.content` from `chat:token` events into a
///   shared buffer (for final persistence).
/// - Injects `request_id` into every forwarded event payload.
struct ContentAccumulator {
    inner: Arc<dyn EventSink + Send + Sync + 'static>,
    content: Arc<parking_lot::Mutex<String>>,
    request_id: String,
}

impl EventSink for ContentAccumulator {
    fn emit(&self, event_type: &str, payload: serde_json::Value) {
        // Accumulate assistant delta content before forwarding.
        if event_type == "chat:token" {
            if let Some(delta) = payload
                .pointer("/choices/0/delta/content")
                .and_then(|v| v.as_str())
            {
                self.content.lock().push_str(delta);
            }
        }

        // Inject request_id so the client can correlate tokens to this request.
        let mut p = payload;
        if let Some(obj) = p.as_object_mut() {
            obj.insert(
                "request_id".to_string(),
                serde_json::Value::String(self.request_id.clone()),
            );
        }
        self.inner.emit(event_type, p);
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use crate::auth::sessions::Sessions;
    use crate::auth::users::{hash_password, Users};
    use crate::config::ServerConfig;
    use crate::http::main_router;
    use crate::storage::session_bus::SessionBus;
    use crate::storage::user_data::UserData;

    // ── test state builder ────────────────────────────────────────────────────

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

        // Create a projects root with a minimal project so resolve_under works.
        let projects_root = dir.path().join("projects");
        let proj_dir = projects_root.join("myproj");
        std::fs::create_dir_all(proj_dir.join("wiki")).unwrap();
        std::fs::write(proj_dir.join("schema.md"), b"# Schema\n").unwrap();

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

    // ── chat_conversations_without_cookie_is_401 ──────────────────────────────

    #[tokio::test]
    async fn chat_conversations_without_cookie_is_401() {
        let (_dir, state) = build_state("alice", "pw");
        let app = main_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/chat/conversations?project_path=myproj")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
    }

    // ── chat_list_returns_empty_for_new_project ───────────────────────────────

    #[tokio::test]
    async fn chat_list_returns_empty_for_new_project() {
        let (_dir, state) = build_state("alice", "pw");
        let app = main_router(state.clone());
        let cookie = do_login(app.clone(), "alice", "pw").await;

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/chat/conversations?project_path=myproj")
                    .header("cookie", cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v["conversations"].as_array().unwrap().is_empty());
    }

    // ── chat_send_without_llm_config_returns_400_with_correct_code ───────────

    #[tokio::test]
    async fn chat_send_without_llm_config_returns_400_with_correct_code() {
        let (_dir, state) = build_state("alice", "pw");
        let app = main_router(state.clone());
        let cookie = do_login(app.clone(), "alice", "pw").await;

        // No LLM config saved for alice → should get 400.
        let body = serde_json::json!({
            "project_path": "myproj",
            "conversation_id": "conv-abc",
            "messages": [{"role": "user", "content": "Hello"}]
        })
        .to_string();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/chat/send")
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

    // ── chat_send_with_minimal_config_returns_202 ─────────────────────────────

    #[tokio::test]
    async fn chat_send_with_minimal_config_returns_202() {
        let (_dir, state) = build_state("alice", "pw");

        // Pre-populate a valid LLM config for alice so the handler doesn't bail early.
        // The background task will attempt an HTTP call to a non-existent server but
        // we only assert the synchronous 202 response here.
        state
            .user_data
            .save_config(
                "alice",
                &serde_json::json!({
                    "llm": {
                        "base_url": "http://127.0.0.1:1",  // unreachable — that's fine
                        "model": "test-model"
                    }
                }),
            )
            .unwrap();

        let app = main_router(state.clone());
        let cookie = do_login(app.clone(), "alice", "pw").await;

        let body = serde_json::json!({
            "project_path": "myproj",
            "conversation_id": "conv-xyz",
            "messages": [{"role": "user", "content": "Hello"}]
        })
        .to_string();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/chat/send")
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

    // ── provider_config_from_user unit tests ──────────────────────────────────

    #[test]
    fn provider_config_from_user_succeeds_with_full_config() {
        let cfg = serde_json::json!({
            "llm": {
                "base_url": "https://api.openai.com",
                "api_key": "sk-test",
                "model": "gpt-4o-mini",
                "extra_headers": { "X-Foo": "bar" }
            }
        });
        let pc = provider_config_from_user(&cfg).unwrap();
        assert_eq!(pc.base_url, "https://api.openai.com");
        assert_eq!(pc.api_key, Some("sk-test".to_string()));
        assert_eq!(pc.model, "gpt-4o-mini");
        assert_eq!(pc.extra_headers.get("X-Foo").map(String::as_str), Some("bar"));
    }

    #[test]
    fn provider_config_from_user_fails_when_llm_key_absent() {
        let cfg = serde_json::json!({});
        assert!(provider_config_from_user(&cfg).is_err());
    }

    #[test]
    fn provider_config_from_user_fails_when_model_missing() {
        let cfg = serde_json::json!({ "llm": { "base_url": "http://x" } });
        assert!(provider_config_from_user(&cfg).is_err());
    }

    #[test]
    fn provider_config_from_user_api_key_optional() {
        let cfg = serde_json::json!({
            "llm": { "base_url": "http://localhost", "model": "llama3" }
        });
        let pc = provider_config_from_user(&cfg).unwrap();
        assert!(pc.api_key.is_none());
    }

    // ── content_accumulator collects delta content ────────────────────────────

    #[test]
    fn content_accumulator_collects_delta_content() {
        use crate::core::events::CapturingEventSink;

        let inner = Arc::new(CapturingEventSink::default());
        let content = Arc::new(parking_lot::Mutex::new(String::new()));
        let acc = ContentAccumulator {
            inner: inner.clone(),
            content: content.clone(),
            request_id: "req-1".to_string(),
        };

        acc.emit(
            "chat:token",
            serde_json::json!({"choices": [{"delta": {"content": "Hello"}}]}),
        );
        acc.emit(
            "chat:token",
            serde_json::json!({"choices": [{"delta": {"content": " world"}}]}),
        );

        assert_eq!(content.lock().as_str(), "Hello world");

        // Every forwarded event must carry request_id.
        let events = inner.snapshot();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].1["request_id"], "req-1");
        assert_eq!(events[1].1["request_id"], "req-1");
    }
}
