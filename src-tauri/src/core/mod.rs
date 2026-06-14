//! Pure-Rust business logic. No axum, no Tauri, no AppHandle.
//!
//! Functions in this module are called by:
//! - `src-tauri/src/commands/*.rs` (Tauri command wrappers, for the desktop app)
//! - `src-tauri/src/http/*.rs` (axum handlers, for the browser/LAN app, Phase 4)
//!
//! Streamed events (ingest progress, file-watcher updates, LLM tokens) are
//! emitted via the `EventSink` trait in `core::events`.

pub mod events;
pub mod extract;
pub mod files;
pub mod fs_ops;
pub mod search;
pub mod vectorstore;
pub mod wiki;
