//! HTTP handlers for wiki page CRUD (with ETag-based optimistic concurrency),
//! search, and graph.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::http::auth::AuthUser;
use crate::http::error::ApiError;
use crate::http::AppState;
use crate::storage::paths::resolve_under;

pub fn wiki_router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/wiki/page", get(read_page).put(write_page))
        .route("/api/v1/search", post(search))
        .route("/api/v1/graph", get(graph))
}

// ── ETag helper ─────────────────────────────────────────────────────────────

fn etag_for(content: &[u8]) -> String {
    blake3::hash(content).to_hex().as_str()[..16].to_string()
}

// ── Path helpers ─────────────────────────────────────────────────────────────

/// Resolve project root; the project directory must already exist.
fn resolve_project_root(state: &AppState, project_path: &str) -> Result<std::path::PathBuf, ApiError> {
    resolve_under(&state.config.projects_root, project_path).map_err(|e| {
        ApiError::bad_request("PATH_ESCAPE", e.to_string())
            .with_details(serde_json::json!({ "requested": project_path }))
    })
}

/// Resolve page path when the file must already exist (for reads and ETag checks).
fn resolve_existing_page(
    project_root: &std::path::Path,
    page_path: &str,
) -> Result<std::path::PathBuf, ApiError> {
    resolve_under(project_root, page_path).map_err(|e| {
        ApiError::bad_request("PATH_ESCAPE", e.to_string())
            .with_details(serde_json::json!({ "requested": page_path }))
    })
}

/// Resolve page path for writes: validate path components without requiring
/// the file to exist. This prevents path traversal without failing on new files.
fn resolve_write_page(
    project_root: &std::path::Path,
    page_path: &str,
) -> Result<std::path::PathBuf, ApiError> {
    use std::path::{Component, Path};

    let trimmed = page_path.trim();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request("PATH_ESCAPE", "page_path must not be empty"));
    }
    let req_path = Path::new(trimmed);
    if req_path.is_absolute() {
        return Err(ApiError::bad_request("PATH_ESCAPE", "page_path must be relative"));
    }
    for component in req_path.components() {
        match component {
            Component::ParentDir => {
                return Err(ApiError::bad_request("PATH_ESCAPE", "page_path contains .. segment"))
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(ApiError::bad_request("PATH_ESCAPE", "page_path must be relative"))
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    // Post-creation canonical check: join with project root (which *does* exist).
    // We verify the joined (non-canonicalized) path stays under project root.
    let project_canon = project_root
        .canonicalize()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let joined = project_canon.join(req_path);
    // Normalize without canonicalize (file may not exist yet).
    let normalized = normalize_path_no_canonicalize(&joined);
    if !normalized.starts_with(&project_canon) {
        return Err(ApiError::bad_request(
            "PATH_ESCAPE",
            "page_path escapes project root",
        ));
    }
    Ok(normalized)
}

/// Normalize a path (resolve `.` components) without calling `canonicalize`.
/// This works for paths whose file may not exist yet.
fn normalize_path_no_canonicalize(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::{Component, PathBuf};
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

// ── GET /api/v1/wiki/page ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct PageQuery {
    project_path: String,
    page_path: String,
}

async fn read_page(
    State(state): State<AppState>,
    AuthUser(_user): AuthUser,
    Query(q): Query<PageQuery>,
) -> Result<Response, ApiError> {
    let project_root = resolve_project_root(&state, &q.project_path)?;
    let path = resolve_existing_page(&project_root, &q.page_path)?;

    let bytes = tokio::fs::read(&path).await.map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ApiError::new(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("page not found: {}", q.page_path),
        ),
        _ => ApiError::internal(e.to_string()),
    })?;

    let content = String::from_utf8_lossy(&bytes).to_string();
    let etag = etag_for(&bytes);

    let body = Json(serde_json::json!({ "content": content, "etag": etag }));
    let mut resp = body.into_response();
    resp.headers_mut().insert(
        axum::http::header::ETAG,
        axum::http::HeaderValue::from_str(&format!("\"{etag}\"")).unwrap(),
    );
    Ok(resp)
}

// ── PUT /api/v1/wiki/page ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct WritePageRequest {
    project_path: String,
    page_path: String,
    content: String,
}

async fn write_page(
    State(state): State<AppState>,
    AuthUser(_user): AuthUser,
    headers: HeaderMap,
    Json(req): Json<WritePageRequest>,
) -> Result<Response, ApiError> {
    // Require If-Match header (quote-strip per HTTP spec).
    let if_match = headers
        .get(axum::http::header::IF_MATCH)
        .and_then(|h| h.to_str().ok())
        .map(|s| s.trim_matches('"').to_string())
        .ok_or_else(|| {
            ApiError::bad_request(
                "BAD_REQUEST",
                "If-Match header required for wiki page writes",
            )
        })?;

    let project_root = resolve_project_root(&state, &req.project_path)?;
    // Use write-path resolution (file may not exist yet for new pages,
    // but existing pages need ETag check before write).
    let path = resolve_write_page(&project_root, &req.page_path)?;

    // Read current content to compute current ETag.
    let current_bytes = tokio::fs::read(&path).await.map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ApiError::new(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("page not found: {}", req.page_path),
        ),
        _ => ApiError::internal(e.to_string()),
    })?;

    let current_etag = etag_for(&current_bytes);
    if current_etag != if_match {
        return Err(ApiError::new(
            StatusCode::PRECONDITION_FAILED,
            "WIKI_PAGE_STALE",
            "page was modified since you loaded it",
        )
        .with_details(serde_json::json!({ "current_etag": current_etag })));
    }

    let new_bytes = req.content.as_bytes();
    let new_etag = etag_for(new_bytes);

    // Atomic write: write to temp file then rename.
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    let tmp = path.with_extension("md.tmp");
    tokio::fs::write(&tmp, new_bytes)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    tokio::fs::rename(&tmp, &path)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let mut resp =
        (StatusCode::OK, Json(serde_json::json!({ "etag": new_etag }))).into_response();
    resp.headers_mut().insert(
        axum::http::header::ETAG,
        axum::http::HeaderValue::from_str(&format!("\"{new_etag}\"")).unwrap(),
    );
    Ok(resp)
}

// ── POST /api/v1/search ──────────────────────────────────────────────────────

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
    AuthUser(_user): AuthUser,
    Json(req): Json<SearchRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let project_root = resolve_project_root(&state, &req.project_path)?;

    // Call core::search::search_project with its full signature.
    // We do not pass embedding_config from the HTTP layer; keyword-only
    // fallback is always available. Callers needing vector search should
    // supply query_embedding pre-computed on the client side.
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

// ── GET /api/v1/graph ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct GraphQuery {
    project_path: String,
}

async fn graph(
    State(state): State<AppState>,
    AuthUser(_user): AuthUser,
    Query(q): Query<GraphQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Validate the project path (returns 400 PATH_ESCAPE on bad input).
    let _project_root = resolve_project_root(&state, &q.project_path)?;

    // TODO(phase-5): when core exposes a graph-building function (e.g.
    // `core::graph::compute_graph`), wire it here and return the node/edge
    // payload. For now the graph is computed entirely client-side using
    // Sigma + graphology, so a backend graph endpoint is not yet needed.
    Err(ApiError::new(
        StatusCode::NOT_IMPLEMENTED,
        "NOT_IMPLEMENTED",
        "graph endpoint not yet implemented; frontend computes the graph client-side",
    ))
}
