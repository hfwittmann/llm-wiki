//! Thin Tauri command wrappers for `core::file_sync`.
//!
//! Each of the 6 commands constructs a `TauriEventSink` and forwards to the
//! corresponding pure-Rust function in `core::file_sync`.  No business logic
//! lives here.

use std::sync::Arc;

use tauri::AppHandle;

use crate::commands::tauri_event_sink::TauriEventSink;
use crate::core::file_sync::{
    SourceWatchConfig,
    get_file_change_queue as core_get_file_change_queue,
    ignore_file_change_task as core_ignore_file_change_task,
    rescan_project_files as core_rescan_project_files,
    retry_file_change_task as core_retry_file_change_task,
    start_project_file_watcher_boxed,
    stop_project_file_watcher as core_stop_project_file_watcher,
};
use crate::core::ingest_queue::{FileChangeQueue, FileChangeRescanResult};

#[tauri::command]
pub fn start_project_file_watcher(
    app: AppHandle,
    project_id: String,
    project_path: String,
    source_watch_config: Option<SourceWatchConfig>,
) -> Result<FileChangeRescanResult, String> {
    let sink = Arc::new(TauriEventSink::new(app));
    start_project_file_watcher_boxed(&project_id, &project_path, source_watch_config, sink)
}

#[tauri::command]
pub fn stop_project_file_watcher() -> Result<(), String> {
    core_stop_project_file_watcher()
}

#[tauri::command]
pub fn rescan_project_files(
    app: AppHandle,
    project_id: String,
    project_path: String,
    source_watch_config: Option<SourceWatchConfig>,
) -> Result<FileChangeRescanResult, String> {
    let sink = TauriEventSink::new(app);
    core_rescan_project_files(&project_id, &project_path, source_watch_config, &sink)
}

#[tauri::command]
pub fn get_file_change_queue(project_path: String) -> Result<FileChangeQueue, String> {
    core_get_file_change_queue(&project_path)
}

#[tauri::command]
pub fn retry_file_change_task(
    app: AppHandle,
    project_id: String,
    project_path: String,
    task_id: String,
) -> Result<FileChangeQueue, String> {
    let sink = TauriEventSink::new(app);
    core_retry_file_change_task(&project_id, &project_path, &task_id, &sink)
}

#[tauri::command]
pub fn ignore_file_change_task(
    app: AppHandle,
    project_id: String,
    project_path: String,
    task_id: String,
) -> Result<FileChangeQueue, String> {
    let sink = TauriEventSink::new(app);
    core_ignore_file_change_task(&project_id, &project_path, &task_id, &sink)
}
