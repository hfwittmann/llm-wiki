//! Bundles the frontend `dist/` into the binary via rust-embed and serves
//! it with an SPA fallback: unknown paths return `index.html`.
//!
//! The folder path is `../dist/` — the same project-root `dist/` that Vite
//! writes to via `npm run build` (matching `tauri.conf.json`'s
//! `"frontendDist": "../dist"`). This way both the Tauri shell and the
//! HTTP server consume the same Vite output.

use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../dist/"]
struct Frontend;

pub async fn spa_fallback(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    // API routes already handled by other layers; fallback shouldn't see them
    // but we double-check.
    if path.starts_with("api/") {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    if let Some(asset) = Frontend::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return Response::builder()
            .header(header::CONTENT_TYPE, mime.as_ref())
            .body(Body::from(asset.data.into_owned()))
            .unwrap();
    }

    // SPA fallback → index.html
    if let Some(index) = Frontend::get("index.html") {
        return Response::builder()
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from(index.data.into_owned()))
            .unwrap();
    }

    (StatusCode::NOT_FOUND, "not found").into_response()
}
