//! HTTP handlers for project enumeration, opening, and creation.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::core::project::{self, project_id_from_canonical_path};
use crate::http::auth::AuthUser;
use crate::http::error::ApiError;
use crate::http::AppState;
use crate::storage::paths::resolve_under;

pub fn projects_router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/projects/list", get(list))
        .route("/api/v1/projects/open", post(open))
        .route("/api/v1/projects/create", post(create))
}

// ── Response shapes ──────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct ProjectSummary {
    id: String,
    name: String,
    path: String,
}

#[derive(Debug, Serialize)]
struct ProjectResult {
    project_id: String,
    name: String,
    path: String,
}

// ── list ─────────────────────────────────────────────────────────────────────

async fn list(
    State(state): State<AppState>,
    AuthUser(_user): AuthUser,
) -> Result<Json<Vec<ProjectSummary>>, ApiError> {
    let root = &state.config.projects_root;
    let mut out = Vec::new();
    if !root.exists() {
        return Ok(Json(out));
    }
    let canon_root = root
        .canonicalize()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    for entry in
        std::fs::read_dir(&canon_root).map_err(|e| ApiError::internal(e.to_string()))?
    {
        let entry = entry.map_err(|e| ApiError::internal(e.to_string()))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Heuristic: a valid wiki project has schema.md + wiki/ subdir.
        let has_schema =
            path.join("schema.md").exists() || path.join(".llm-wiki/schema.md").exists();
        let has_wiki = path.join("wiki").is_dir();
        if !has_schema || !has_wiki {
            continue;
        }
        let id = project_id_from_canonical_path(&path);
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = path
            .strip_prefix(&canon_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        out.push(ProjectSummary { id, name, path: rel });
    }
    Ok(Json(out))
}

// ── open ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct OpenRequest {
    path: String,
}

async fn open(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<OpenRequest>,
) -> Result<Json<ProjectResult>, ApiError> {
    let root = &state.config.projects_root;
    // Accept either a relative path under `projects_root` OR an absolute path
    // that happens to be inside `projects_root`. The "recently opened" UI in
    // the React frontend stores absolute paths (a holdover from the desktop
    // era); strip the projects_root prefix to get the relative form, then
    // route through `resolve_under` for path safety.
    let req_path: &str = &req.path;
    let rel: String = if std::path::Path::new(req_path).is_absolute() {
        match root.canonicalize().ok().and_then(|root_canon| {
            std::path::Path::new(req_path)
                .canonicalize()
                .ok()
                .and_then(|p| p.strip_prefix(&root_canon).ok().map(|s| s.to_path_buf()))
        }) {
            Some(rel_path) => rel_path.to_string_lossy().to_string(),
            None => {
                return Err(ApiError::bad_request(
                    "PATH_ESCAPE",
                    "absolute path is not under projects_root",
                )
                .with_details(serde_json::json!({ "requested": req.path })));
            }
        }
    } else {
        req_path.to_string()
    };
    let resolved = resolve_under(root, &rel).map_err(|e| {
        ApiError::bad_request("PATH_ESCAPE", e.to_string())
            .with_details(serde_json::json!({ "requested": req.path }))
    })?;

    let wiki = project::open_project(resolved.to_string_lossy().to_string())?;
    let id = project_id_from_canonical_path(&resolved);
    let _ = state.user_data.add_recently_opened(&user.id, &id);

    Ok(Json(ProjectResult {
        project_id: id,
        name: wiki.name,
        path: wiki.path,
    }))
}

// ── create ───────────────────────────────────────────────────────────────────

/// Request body for project creation.
///
/// `name` is the directory name for the new project; it will be created under
/// `config.projects_root`.  `scenario_template` is accepted for forward-
/// compatibility but currently unused (the single built-in template is always
/// applied by `core::project::create_project`).
#[derive(Debug, Deserialize)]
struct CreateRequest {
    name: String,
    #[serde(default)]
    scenario_template: Option<String>,
}

async fn create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<CreateRequest>,
) -> Result<Json<ProjectResult>, ApiError> {
    // Reject names that contain path separators or `..` to prevent traversal.
    if req.name.contains('/') || req.name.contains('\\') || req.name == ".." || req.name == "." {
        return Err(ApiError::bad_request(
            "PATH_ESCAPE",
            "name must be a single directory component with no separators",
        ));
    }

    let root = &state.config.projects_root;

    // `create_project(name, path)` creates `<path>/<name>`.
    let parent_str = root.to_string_lossy().to_string();

    // scenario_template is reserved for future use.
    let _scenario = req
        .scenario_template
        .unwrap_or_else(|| "general".to_string());

    let wiki = project::create_project(req.name.clone(), parent_str)?;

    // Canonicalize for a stable project_id.
    let target = root.join(&req.name);
    let canon = target.canonicalize().unwrap_or(target.clone());
    let id = project_id_from_canonical_path(&canon);
    let _ = state.user_data.add_recently_opened(&user.id, &id);

    Ok(Json(ProjectResult {
        project_id: id,
        name: wiki.name,
        path: wiki.path,
    }))
}
