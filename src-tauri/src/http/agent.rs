//! Agent-facing read-only endpoints, mounted on both the main listener
//! (auth required) and the legacy 127.0.0.1:19828 listener (no auth).
//!
//! Handlers do NOT use `AuthUser` so they remain reachable on the legacy
//! listener. Per-user state (chat history, user config) is intentionally
//! out of reach here. These endpoints are project-scoped, not user-scoped.
//!
//! Routes exposed:
//!   GET  /api/v1/agent/projects            — list valid wiki projects
//!   POST /api/v1/agent/search              — keyword/vector search, no auth required
//!   GET  /api/v1/agent/file?project_path=&path= — raw file bytes

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::http::error::ApiError;
use crate::http::AppState;
use crate::storage::paths::{resolve_under, PathError};

pub fn agent_router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/agent/projects", get(projects))
        .route("/api/v1/agent/search", post(search))
        .route("/api/v1/agent/file", get(file))
}

// ── GET /api/v1/agent/projects ───────────────────────────────────────────────

async fn projects(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let root = &state.config.projects_root;
    if !root.exists() {
        return Ok(Json(serde_json::json!({ "projects": [] })));
    }
    let canon_root = root
        .canonicalize()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut out = Vec::new();
    for entry in
        std::fs::read_dir(&canon_root).map_err(|e| ApiError::internal(e.to_string()))?
    {
        let entry = entry.map_err(|e| ApiError::internal(e.to_string()))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // A valid wiki project has schema.md + wiki/ subdir.
        let has_schema =
            path.join("schema.md").exists() || path.join(".llm-wiki/schema.md").exists();
        let has_wiki = path.join("wiki").is_dir();
        if !(has_schema && has_wiki) {
            continue;
        }
        let id = crate::core::project::project_id_from_canonical_path(&path);
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = path
            .strip_prefix(&canon_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        out.push(serde_json::json!({ "id": id, "name": name, "path": rel }));
    }
    Ok(Json(serde_json::json!({ "projects": out })))
}

// ── POST /api/v1/agent/search ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SearchRequest {
    project_path: String,
    query: String,
    #[serde(default)]
    top_k: Option<usize>,
    #[serde(default)]
    include_content: Option<bool>,
}

async fn search(
    State(state): State<AppState>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let project_root =
        resolve_under(&state.config.projects_root, &req.project_path).map_err(|e| {
            ApiError::bad_request("PATH_ESCAPE", e.to_string())
                .with_details(serde_json::json!({ "requested": req.project_path }))
        })?;

    let result = crate::core::search::search_project(
        project_root.to_string_lossy().to_string(),
        req.query,
        req.top_k,
        req.include_content,
        None, // query_embedding — not supplied via this endpoint
        None, // embedding_config — not supplied via this endpoint
    )
    .await?;

    let value = serde_json::to_value(&result).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(value))
}

// ── GET /api/v1/agent/file ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct FileQuery {
    project_path: String,
    path: String,
}

async fn file(
    State(state): State<AppState>,
    Query(q): Query<FileQuery>,
) -> Result<Response, ApiError> {
    let project_root =
        resolve_under(&state.config.projects_root, &q.project_path).map_err(|e| {
            ApiError::bad_request("PATH_ESCAPE", e.to_string())
                .with_details(serde_json::json!({ "requested": q.project_path }))
        })?;

    let file_path = resolve_under(&project_root, &q.path).map_err(|e| match e {
        PathError::NotFound => ApiError::new(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("file not found: {}", q.path),
        ),
        _ => ApiError::bad_request("PATH_ESCAPE", e.to_string())
            .with_details(serde_json::json!({ "requested": q.path })),
    })?;

    if !file_path.is_file() {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("file not found: {}", q.path),
        ));
    }

    let bytes = tokio::fs::read(&file_path).await.map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ApiError::new(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("file not found: {}", q.path),
        ),
        _ => ApiError::internal(e.to_string()),
    })?;

    let mime = mime_guess::from_path(&file_path).first_or_octet_stream();
    let resp = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        .body(Body::from(bytes))
        .unwrap();
    Ok(resp)
}
