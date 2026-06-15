//! HTTP handler for file preview bytes.

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;

use crate::http::auth::AuthUser;
use crate::http::error::ApiError;
use crate::http::AppState;
use crate::storage::paths::{resolve_under, resolve_project_path, PathError};

pub fn files_router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/files/raw", get(raw))
        .route("/api/v1/files/extracted-text", get(extracted_text))
}

#[derive(Debug, Deserialize)]
struct RawQuery {
    /// Two accepted shapes, in priority order:
    ///   (a) `project_path` + `path` — project_path can be absolute (under
    ///       projects_root) or relative; `path` is project-relative.
    ///   (b) Only `path` — must be an absolute path under projects_root.
    /// Legacy callers in the migrated frontend send (b); newer callers should
    /// prefer (a) because it's path-safer.
    #[serde(default)]
    project_path: Option<String>,
    path: String,
}

async fn raw(
    State(state): State<AppState>,
    AuthUser(_user): AuthUser,
    Query(q): Query<RawQuery>,
) -> Result<Response, ApiError> {
    let projects_root = &state.config.projects_root;

    let file_path = match q.project_path.as_deref() {
        Some(pp) if !pp.is_empty() => {
            let project_root = resolve_project_path(projects_root, pp).map_err(|e| {
                ApiError::bad_request("PATH_ESCAPE", e.to_string())
                    .with_details(serde_json::json!({ "requested": pp }))
            })?;
            resolve_under(&project_root, &q.path).map_err(|e| match e {
                PathError::NotFound => ApiError::new(
                    StatusCode::NOT_FOUND,
                    "NOT_FOUND",
                    format!("file not found: {}", q.path),
                ),
                _ => ApiError::bad_request("PATH_ESCAPE", e.to_string())
                    .with_details(serde_json::json!({ "requested": q.path })),
            })?
        }
        _ => {
            // Single absolute path — must be under projects_root.
            resolve_project_path(projects_root, &q.path).map_err(|e| match e {
                PathError::NotFound => ApiError::new(
                    StatusCode::NOT_FOUND,
                    "NOT_FOUND",
                    format!("file not found: {}", q.path),
                ),
                _ => ApiError::bad_request("PATH_ESCAPE", e.to_string())
                    .with_details(serde_json::json!({ "requested": q.path })),
            })?
        }
    };

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
    // No caching headers for now — the frontend can re-fetch as needed.
    Ok(resp)
}

// ── Extracted text preview ───────────────────────────────────────────────────
//
// Plain-text extraction of PDF (and eventually DOCX/PPTX) for the preview
// pane. The desktop pipeline used pdfium-via-Tauri for this; we now expose
// it as an HTTP endpoint so the React preview can request text directly
// instead of trying to render raw binary bytes as a string.

async fn extracted_text(
    State(state): State<AppState>,
    AuthUser(_user): AuthUser,
    Query(q): Query<RawQuery>,
) -> Result<Response, ApiError> {
    let projects_root = &state.config.projects_root;

    let file_path = match q.project_path.as_deref() {
        Some(pp) if !pp.is_empty() => {
            let project_root = resolve_project_path(projects_root, pp).map_err(|e| {
                ApiError::bad_request("PATH_ESCAPE", e.to_string())
                    .with_details(serde_json::json!({ "requested": pp }))
            })?;
            resolve_under(&project_root, &q.path).map_err(|e| match e {
                PathError::NotFound => ApiError::new(
                    StatusCode::NOT_FOUND,
                    "NOT_FOUND",
                    format!("file not found: {}", q.path),
                ),
                _ => ApiError::bad_request("PATH_ESCAPE", e.to_string())
                    .with_details(serde_json::json!({ "requested": q.path })),
            })?
        }
        _ => resolve_project_path(projects_root, &q.path).map_err(|e| match e {
            PathError::NotFound => ApiError::new(
                StatusCode::NOT_FOUND,
                "NOT_FOUND",
                format!("file not found: {}", q.path),
            ),
            _ => ApiError::bad_request("PATH_ESCAPE", e.to_string())
                .with_details(serde_json::json!({ "requested": q.path })),
        })?,
    };

    if !file_path.is_file() {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("file not found: {}", q.path),
        ));
    }

    let ext = file_path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();

    let path_string = file_path.to_string_lossy().to_string();
    // Dispatch to the right extractor by extension. pdfium calls block; run
    // them on a worker thread so the axum runtime stays responsive.
    let text = match ext.as_str() {
        "pdf" => {
            tokio::task::spawn_blocking(move || {
                crate::core::files::extract_pdf_text(&path_string, false)
            })
            .await
            .map_err(|e| ApiError::internal(format!("blocking task panicked: {e}")))?
            .map_err(|msg| ApiError::internal(format!("pdf extract: {msg}")))?
        }
        // DOCX/XLSX/PPTX/ODT/ODS/ODP — not yet wired here. Fall back to an
        // empty body with a 415 so the preview can show "no text preview".
        _ => {
            return Err(ApiError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "UNSUPPORTED",
                format!("no text extractor for .{}", ext),
            ));
        }
    };

    let resp = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(text))
        .unwrap();
    Ok(resp)
}
