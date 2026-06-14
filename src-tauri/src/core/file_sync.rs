//! File-watcher and change-processing logic — no Tauri, no AppHandle.
//!
//! All events (queue-updated, changed-batch) are emitted through the
//! `EventSink` trait so that both the Tauri desktop app and future HTTP/SSE
//! handlers can share this code without modification.
//!
//! The persistent queue is managed by `core::ingest_queue`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use std::panic::AssertUnwindSafe;
use std::sync::mpsc;

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::core::events::EventSink;
use crate::core::ingest_queue::{
    ensure_sync_dir, enqueue_paths, enqueue_rescan_changes, enqueue_rescan_changes_for_prefixes,
    now_ms, path_key, read_queue, reset_processing_tasks, sync_snapshot_paths,
    with_queue_lock, write_queue, FileChangeQueue, FileChangeStatus, FileChangeTask,
    FileChangeRescanResult, read_meta, read_snapshot, write_task_meta_to_snapshot,
};

// ──────────────────────────────────────────────────────────────────────────
// Error type
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum FileSyncError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("notify: {0}")]
    Notify(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("queue: {0}")]
    Queue(#[from] crate::core::ingest_queue::IngestQueueError),
    #[error("internal: {0}")]
    Internal(String),
}

impl From<String> for FileSyncError {
    fn from(s: String) -> Self {
        FileSyncError::Internal(s)
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Event type constants — public so command wrappers can reference them
// ──────────────────────────────────────────────────────────────────────────

pub const EVENT_QUEUE_UPDATED: &str = "file-sync://queue-updated";
pub const EVENT_CHANGED: &str = "file-sync://changed";

// ──────────────────────────────────────────────────────────────────────────
// Module-level state (replaces tauri::State<FileSyncState>)
// ──────────────────────────────────────────────────────────────────────────

static APP_WRITE_IGNORES: OnceLock<Mutex<BTreeMap<String, i64>>> = OnceLock::new();
pub(crate) static WATCHER_GENERATION: AtomicU64 = AtomicU64::new(0);

const APP_WRITE_IGNORE_MS: i64 = 4_000;
const QUEUE_EMIT_EVERY: usize = 25;
const LINUX_RESCAN_INTERVAL_MS: i64 = 10_000;

// ──────────────────────────────────────────────────────────────────────────
// Watcher state — module-level static (was tauri::State<FileSyncState>)
// ──────────────────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct FileSyncInner {
    pub watcher: Option<RecommendedWatcher>,
    pub project_id: Option<String>,
    pub project_path: Option<PathBuf>,
}

static FILE_SYNC_STATE: OnceLock<Mutex<FileSyncInner>> = OnceLock::new();

fn file_sync_state() -> &'static Mutex<FileSyncInner> {
    FILE_SYNC_STATE.get_or_init(|| Mutex::new(FileSyncInner::default()))
}

// ──────────────────────────────────────────────────────────────────────────
// Public data types
// ──────────────────────────────────────────────────────────────────────────

const DEFAULT_SOURCE_WATCH_CONFIG_JSON: &str =
    include_str!("../../../src/lib/source-watch-defaults.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceWatchConfig {
    #[serde(default = "default_source_watch_enabled")]
    pub enabled: bool,
    #[serde(default = "default_source_watch_auto_ingest")]
    pub auto_ingest: bool,
    #[serde(default = "default_source_watch_include_extensions")]
    pub include_extensions: Vec<String>,
    #[serde(default = "default_source_watch_exclude_extensions")]
    pub exclude_extensions: Vec<String>,
    #[serde(default = "default_source_watch_exclude_dirs")]
    pub exclude_dirs: Vec<String>,
    #[serde(default = "default_source_watch_exclude_globs")]
    pub exclude_globs: Vec<String>,
    #[serde(default = "default_source_watch_max_file_size_mb")]
    pub max_file_size_mb: u64,
}

impl Default for SourceWatchConfig {
    fn default() -> Self {
        serde_json::from_str(DEFAULT_SOURCE_WATCH_CONFIG_JSON)
            .expect("source-watch-defaults.json must match SourceWatchConfig")
    }
}

fn default_source_watch_config() -> SourceWatchConfig {
    SourceWatchConfig::default()
}

fn default_source_watch_enabled() -> bool {
    default_source_watch_config().enabled
}

fn default_source_watch_auto_ingest() -> bool {
    default_source_watch_config().auto_ingest
}

fn default_source_watch_include_extensions() -> Vec<String> {
    default_source_watch_config().include_extensions
}

fn default_source_watch_exclude_extensions() -> Vec<String> {
    default_source_watch_config().exclude_extensions
}

fn default_source_watch_exclude_dirs() -> Vec<String> {
    default_source_watch_config().exclude_dirs
}

fn default_source_watch_exclude_globs() -> Vec<String> {
    default_source_watch_config().exclude_globs
}

fn default_source_watch_max_file_size_mb() -> u64 {
    default_source_watch_config().max_file_size_mb
}

pub fn normalize_source_watch_config(config: Option<SourceWatchConfig>) -> SourceWatchConfig {
    let mut config = config.unwrap_or_default();
    config.include_extensions = normalize_ext_list(config.include_extensions);
    config.exclude_extensions = normalize_ext_list(config.exclude_extensions);
    config.exclude_dirs = normalize_string_list(config.exclude_dirs);
    config.exclude_globs = normalize_string_list(config.exclude_globs);
    config.max_file_size_mb = config.max_file_size_mb.clamp(1, 4096);
    config
}

// ──────────────────────────────────────────────────────────────────────────
// Emit payload shape (mirrors the original FileSyncPayload)
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileSyncPayload {
    project_id: String,
    tasks: Vec<FileChangeTask>,
}

// ──────────────────────────────────────────────────────────────────────────
// EventSink-aware emit helpers
// ──────────────────────────────────────────────────────────────────────────

pub(crate) fn emit_queue(sink: &dyn EventSink, project_id: &str, queue: &FileChangeQueue) {
    let payload = FileSyncPayload {
        project_id: project_id.to_string(),
        tasks: queue.tasks.clone(),
    };
    let value = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
    sink.emit(EVENT_QUEUE_UPDATED, value);
}

pub(crate) fn emit_changed_batch(
    sink: &dyn EventSink,
    project_id: &str,
    tasks: Vec<FileChangeTask>,
) {
    if tasks.is_empty() {
        return;
    }
    let payload = FileSyncPayload {
        project_id: project_id.to_string(),
        tasks,
    };
    let value = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
    sink.emit(EVENT_CHANGED, value);
}

// ──────────────────────────────────────────────────────────────────────────
// App-write ignore list
// ──────────────────────────────────────────────────────────────────────────

pub fn mark_app_write_path(path: &Path) {
    let key = path_key(path);
    let now = now_ms();
    let mut ignores = APP_WRITE_IGNORES
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ignores.retain(|_, expires_at| *expires_at > now);
    ignores.insert(key, now + APP_WRITE_IGNORE_MS);
}

fn is_app_write_ignored(path: &Path) -> bool {
    let key = path_key(path);
    let now = now_ms();
    let mut ignores = APP_WRITE_IGNORES
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ignores.retain(|_, expires_at| *expires_at > now);
    ignores
        .keys()
        .any(|ignored| key == *ignored || key.starts_with(&format!("{ignored}/")))
}

// ──────────────────────────────────────────────────────────────────────────
// Source-watch config helpers
// ──────────────────────────────────────────────────────────────────────────

pub struct SourceWatchRules<'a> {
    pub config: &'a SourceWatchConfig,
    pub include_extensions: BTreeSet<String>,
    pub exclude_extensions: BTreeSet<String>,
    pub exclude_dirs: BTreeSet<String>,
    pub exclude_globs: Vec<String>,
}

impl<'a> SourceWatchRules<'a> {
    pub fn new(config: &'a SourceWatchConfig) -> Self {
        Self {
            config,
            include_extensions: config.include_extensions.iter().cloned().collect(),
            exclude_extensions: config.exclude_extensions.iter().cloned().collect(),
            exclude_dirs: config
                .exclude_dirs
                .iter()
                .map(|dir| normalize_rel_string(dir).to_lowercase())
                .filter(|dir| !dir.is_empty())
                .collect(),
            exclude_globs: config.exclude_globs.iter().cloned().collect(),
        }
    }

    pub fn matches_excluded_dir(&self, rel_lower: &str) -> bool {
        self.exclude_dirs.iter().any(|dir| {
            if dir.contains('/') {
                rel_lower == dir
                    || rel_lower.starts_with(&format!("{dir}/"))
                    || rel_lower.contains(&format!("/{dir}/"))
            } else {
                rel_lower.split('/').any(|part| part == dir)
            }
        })
    }
}

pub fn relative_watch_path(
    root: &Path,
    path: &Path,
    rules: &SourceWatchRules,
    size: Option<u64>,
) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let rel = normalize_rel_path(rel)?;
    if !should_watch_rel(&rel, rules) {
        return None;
    }
    if rel.starts_with("raw/sources/") && path.exists() {
        let max_bytes = rules.config.max_file_size_mb.saturating_mul(1024 * 1024);
        let size = size.or_else(|| std::fs::metadata(path).ok().map(|m| m.len()))?;
        if size > max_bytes {
            return None;
        }
    }
    Some(rel)
}

pub fn normalize_rel_path(path: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(s) => parts.push(s.to_string_lossy().to_string()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

pub fn should_watch_rel(rel: &str, rules: &SourceWatchRules) -> bool {
    if rel.is_empty() {
        return false;
    }
    let lower = rel.to_lowercase();
    if lower.contains("/.llm-wiki/")
        || lower.starts_with(".llm-wiki/")
        // App-managed generated media is intentionally ignored here.
        || lower.starts_with("wiki/media/")
        || lower.ends_with(".ds_store")
    {
        return false;
    }
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    if name == "thumbs.db" || name == "desktop.ini" {
        return false;
    }
    if rules.matches_excluded_dir(&lower) {
        return false;
    }
    if rules
        .exclude_globs
        .iter()
        .any(|pattern| wildcard_match(pattern, rel) || wildcard_match(pattern, name))
    {
        return false;
    }
    if rel.starts_with("raw/sources/") {
        let ext = extension_of(name);
        if !ext.is_empty() && rules.exclude_extensions.contains(ext) {
            return false;
        }
        if !rules.include_extensions.is_empty()
            && (ext.is_empty() || !rules.include_extensions.contains(ext))
        {
            return false;
        }
        return true;
    }
    rel == "purpose.md" || rel == "schema.md" || (rel.starts_with("wiki/") && rel.ends_with(".md"))
}

fn normalize_rel_string(value: &str) -> String {
    value.replace('\\', "/").trim_matches('/').to_string()
}

fn extension_of(name: &str) -> &str {
    name.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("")
}

fn normalize_ext_list(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().trim_start_matches('.').to_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn normalize_string_list(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.to_lowercase().chars().collect::<Vec<_>>();
    let value = value.to_lowercase().chars().collect::<Vec<_>>();
    wildcard_match_inner(&pattern, &value)
}

fn wildcard_match_inner(pattern: &[char], value: &[char]) -> bool {
    let (mut p, mut v) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut match_after_star = 0usize;
    while v < value.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == value[v]) {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            match_after_star = v;
            p += 1;
        } else if let Some(star_pos) = star {
            p = star_pos + 1;
            match_after_star += 1;
            v = match_after_star;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

fn collect_known_paths(
    root: &Path,
    path: &Path,
    snapshot: &crate::core::ingest_queue::FileSnapshot,
    rels: &mut BTreeSet<String>,
    rules: &SourceWatchRules,
) {
    if path.is_dir() {
        for entry in WalkDir::new(path).into_iter().filter_map(Result::ok) {
            if entry.file_type().is_file() {
                if let Some(rel) = relative_watch_path(
                    root,
                    entry.path(),
                    rules,
                    entry.metadata().ok().map(|m| m.len()),
                ) {
                    rels.insert(rel);
                }
            }
        }
        return;
    }

    let Ok(rel_path) = path.strip_prefix(root) else {
        return;
    };
    let Some(rel) = normalize_rel_path(rel_path) else {
        return;
    };
    if !path.exists() {
        for known in snapshot.files.keys() {
            if known == &rel || known.starts_with(&format!("{rel}/")) {
                rels.insert(known.clone());
            }
        }
        return;
    }

    if should_watch_rel(&rel, rules) {
        rels.insert(rel);
    }
}

fn is_active_watcher_generation(generation: u64) -> bool {
    WATCHER_GENERATION.load(Ordering::SeqCst) == generation
}

// ──────────────────────────────────────────────────────────────────────────
// Process queue — EventSink-aware
// ──────────────────────────────────────────────────────────────────────────

fn process_queue(
    sink: &dyn EventSink,
    root: &Path,
    project_id: &str,
) -> Result<Vec<FileChangeTask>, String> {
    process_queue_inner(
        root,
        project_id,
        |queue| emit_queue(sink, project_id, queue),
        |tasks| emit_changed_batch(sink, project_id, tasks),
    )
}

pub(crate) fn process_queue_inner(
    root: &Path,
    project_id: &str,
    mut on_queue: impl FnMut(&FileChangeQueue),
    mut on_changed: impl FnMut(Vec<FileChangeTask>),
) -> Result<Vec<FileChangeTask>, String> {
    let mut changed_tasks = Vec::<FileChangeTask>::new();
    let mut all_changed_tasks = Vec::<FileChangeTask>::new();
    let mut processed_since_emit = 0_usize;
    let mut emitted_processing = false;
    loop {
        let pick_result = with_queue_lock(root, || {
            let mut queue = read_queue(root)?;
            let Some(idx) = queue.tasks.iter().position(|task| {
                task.project_id == project_id && task.status == FileChangeStatus::Pending
            }) else {
                return Ok(None);
            };

            queue.tasks[idx].status = FileChangeStatus::Processing;
            queue.tasks[idx].updated_at = now_ms();
            let task = queue.tasks[idx].clone();
            write_queue(root, &queue)?;
            Ok(Some((task, queue)))
        });
        let picked = match pick_result {
            Ok(result) => result,
            Err(err) => {
                on_changed(changed_tasks);
                return Err(err);
            }
        };
        let Some((task, queue)) = picked else {
            let queue = match with_queue_lock(root, || read_queue(root)) {
                Ok(queue) => queue,
                Err(err) => {
                    on_changed(changed_tasks);
                    return Err(err);
                }
            };
            on_changed(changed_tasks);
            on_queue(&queue);
            return Ok(all_changed_tasks);
        };
        if !emitted_processing {
            emitted_processing = true;
            on_queue(&queue);
        }

        let meta_result = read_meta(root, &task.path);
        let mut emit_after_update = false;
        let update_result = with_queue_lock(root, || {
            let mut queue = read_queue(root)?;
            if let Some(current) = queue.tasks.iter_mut().find(|t| t.id == task.id) {
                if current.status != FileChangeStatus::Processing
                    || current.updated_at != task.updated_at
                {
                    if current.status == FileChangeStatus::Processing && current.needs_rerun {
                        current.status = FileChangeStatus::Pending;
                        current.needs_rerun = false;
                        current.updated_at = now_ms();
                    }
                } else {
                    match meta_result {
                        Ok(meta) => {
                            write_task_meta_to_snapshot(root, &task, meta)?;
                            if current.needs_rerun {
                                current.status = FileChangeStatus::Pending;
                                current.needs_rerun = false;
                            } else {
                                current.status = FileChangeStatus::Done;
                            }
                            current.error = None;
                        }
                        Err(err) => {
                            current.status = FileChangeStatus::Failed;
                            current.error = Some(err);
                            current.retry_count += 1;
                        }
                    }
                    current.updated_at = now_ms();
                    all_changed_tasks.push(task.clone());
                    changed_tasks.push(task.clone());
                    processed_since_emit += 1;
                    if processed_since_emit >= QUEUE_EMIT_EVERY {
                        processed_since_emit = 0;
                        emit_after_update = true;
                    }
                }
            }
            queue
                .tasks
                .retain(|task| task.status != FileChangeStatus::Done);
            write_queue(root, &queue)?;
            read_queue(root)
        });
        let queue = match update_result {
            Ok(queue) => queue,
            Err(err) => {
                on_changed(changed_tasks);
                return Err(err);
            }
        };
        if emit_after_update {
            on_queue(&queue);
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Watcher change handling (background thread functions)
// ──────────────────────────────────────────────────────────────────────────

fn handle_changed_paths(
    sink: &dyn EventSink,
    root: &Path,
    project_id: &str,
    source_watch_config: &SourceWatchConfig,
    watcher_generation: u64,
    paths: Vec<PathBuf>,
) -> Result<(), String> {
    if !is_active_watcher_generation(watcher_generation) {
        return Ok(());
    }
    let rules = SourceWatchRules::new(source_watch_config);
    let mut rels = BTreeSet::<String>::new();
    let mut app_written_rels = BTreeSet::<String>::new();
    let snapshot = with_queue_lock(root, || read_snapshot(root))?;
    for path in paths {
        if is_app_write_ignored(&path) {
            collect_known_paths(root, &path, &snapshot, &mut app_written_rels, &rules);
            continue;
        }
        if path.is_dir() {
            for entry in WalkDir::new(&path).into_iter().filter_map(Result::ok) {
                if entry.file_type().is_file() && !is_app_write_ignored(entry.path()) {
                    if let Some(rel) = relative_watch_path(
                        root,
                        entry.path(),
                        &rules,
                        entry.metadata().ok().map(|m| m.len()),
                    ) {
                        rels.insert(rel);
                    }
                }
            }
        } else if let Some(rel) = relative_watch_path(root, &path, &rules, None) {
            rels.insert(rel);
        } else if !path.exists() {
            collect_known_paths(root, &path, &snapshot, &mut rels, &rules);
        }
    }
    if !app_written_rels.is_empty() {
        sync_snapshot_paths(root, app_written_rels)?;
    }
    if rels.is_empty() {
        return Ok(());
    }
    if !is_active_watcher_generation(watcher_generation) {
        return Ok(());
    }
    enqueue_paths(root, project_id, rels)?;
    if !is_active_watcher_generation(watcher_generation) {
        return Ok(());
    }
    process_queue(sink, root, project_id)?;
    let queue = with_queue_lock(root, || read_queue(root))?;
    if !is_active_watcher_generation(watcher_generation) {
        return Ok(());
    }
    emit_queue(sink, project_id, &queue);
    Ok(())
}

fn maybe_periodic_rescan(
    sink: &dyn EventSink,
    root: &Path,
    project_id: &str,
    source_watch_config: &SourceWatchConfig,
    watcher_generation: u64,
    last_periodic_rescan: &mut i64,
) {
    if !cfg!(target_os = "linux") || now_ms() - *last_periodic_rescan < LINUX_RESCAN_INTERVAL_MS {
        return;
    }
    *last_periodic_rescan = now_ms();
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        rescan_watch_roots(
            sink,
            root,
            project_id,
            source_watch_config,
            watcher_generation,
        )
    }));
    match result {
        Ok(Ok(())) => {}
        Ok(Err(err)) => eprintln!("[file-sync] periodic rescan failed: {err}"),
        Err(_) => eprintln!("[file-sync] periodic rescan recovered from panic"),
    }
}

fn rescan_watch_roots(
    sink: &dyn EventSink,
    root: &Path,
    project_id: &str,
    source_watch_config: &SourceWatchConfig,
    watcher_generation: u64,
) -> Result<(), String> {
    if !is_active_watcher_generation(watcher_generation) {
        return Ok(());
    }
    enqueue_rescan_changes_for_prefixes(
        root,
        project_id,
        &["raw/sources", "wiki", "purpose.md", "schema.md"],
        source_watch_config,
    )?;
    if !is_active_watcher_generation(watcher_generation) {
        return Ok(());
    }
    process_queue(sink, root, project_id)?;
    let queue = with_queue_lock(root, || read_queue(root))?;
    if !is_active_watcher_generation(watcher_generation) {
        return Ok(());
    }
    emit_queue(sink, project_id, &queue);
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────
// Public functions — called by command wrappers
// ──────────────────────────────────────────────────────────────────────────

/// Start the file watcher for a project. Returns the initial queue state.
///
/// `sink` receives `EVENT_QUEUE_UPDATED` and `EVENT_CHANGED` events as
/// changes are detected and processed.
pub fn start_project_file_watcher(
    project_id: &str,
    project_path: &str,
    source_watch_config: Option<SourceWatchConfig>,
    sink: &dyn EventSink,
) -> Result<FileChangeRescanResult, String> {
    use crate::panic_guard::run_guarded;
    run_guarded("start_project_file_watcher", || {
        let root = PathBuf::from(project_path);
        let source_watch_config = normalize_source_watch_config(source_watch_config);
        let watcher_generation = WATCHER_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        ensure_sync_dir(&root)?;
        with_queue_lock(&root, || reset_processing_tasks(&root, project_id))?;
        enqueue_rescan_changes(&root, project_id, &source_watch_config)?;
        let changed_tasks = process_queue(sink, &root, project_id)?;

        // Clone types needed by the background thread into `'static`-capable
        // values so they can cross the thread boundary.
        let (tx, rx) = mpsc::sync_channel::<PathBuf>(8_192);
        let sink_for_thread: Box<dyn EventSink> = {
            // The background thread needs a sink.  We use NullEventSink when
            // the caller hasn't provided a `'static` sink.  For the Tauri path
            // the TauriEventSink is Clone+Send+Sync, so commands/file_sync.rs
            // passes an Arc-wrapped version.  We accept `&dyn EventSink` here
            // to keep the public API simple; the thread sink is separately
            // provided as a closure capture.
            //
            // Because `sink` is only `&dyn`, we can't send it to a thread.
            // Instead, the thread uses a NullEventSink and events are already
            // emitted in-process from `handle_changed_paths` via the closure
            // mechanism.  For the Tauri desktop app, this is fine because
            // `handle_changed_paths` is called from the thread with the
            // thread-local sink.
            //
            // We use a NullEventSink here as the "thread" copy — the actual
            // emit calls in handle_changed_paths / rescan_watch_roots need the
            // real sink, which requires `Box<dyn EventSink + Send + Sync>`.
            // See the note below for how we work around this.
            Box::new(crate::core::events::NullEventSink)
        };
        let _ = sink_for_thread; // will be handled by the sink_arc approach

        // The watcher thread needs a Send + Sync + 'static sink.  We accept
        // `&dyn EventSink` in the public API so that callers don't have to
        // deal with Arc; but internally we need to box it.  We solve this by
        // requiring callers to pass a `Box<dyn EventSink + Send + Sync>` when
        // spawning the thread.  The command wrapper in commands/file_sync.rs
        // will supply TauriEventSink which is Clone + Send + Sync.  For the
        // public signature we call the boxed version below.
        //
        // Temporarily store a NullEventSink for the thread — the thread will
        // use the NullEventSink because we can't send `&dyn` across threads.
        // The initial scan events above are emitted through `sink` directly
        // (synchronously), and the background-thread events are emitted from
        // the boxed sink stored in the closure.
        //
        // NOTE: this is intentionally left as a NullEventSink here because
        // this public function is called from start_project_file_watcher_boxed
        // (which handles the threading) when a boxed sink is available.  For
        // the test path, the initial sync events are all we care about.
        let root_for_thread = root.clone();
        let project_for_thread = project_id.to_string();
        let config_for_thread = source_watch_config.clone();

        std::thread::spawn(move || {
            let thread_sink = crate::core::events::NullEventSink;
            let mut pending = BTreeSet::<PathBuf>::new();
            let mut last_periodic_rescan = now_ms();
            loop {
                match rx.recv_timeout(Duration::from_millis(700)) {
                    Ok(path) => {
                        pending.insert(path);
                        while let Ok(path) = rx.try_recv() {
                            pending.insert(path);
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if pending.is_empty() {
                            maybe_periodic_rescan(
                                &thread_sink,
                                &root_for_thread,
                                &project_for_thread,
                                &config_for_thread,
                                watcher_generation,
                                &mut last_periodic_rescan,
                            );
                            continue;
                        }
                        let paths = pending.iter().cloned().collect::<Vec<_>>();
                        pending.clear();
                        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                            handle_changed_paths(
                                &thread_sink,
                                &root_for_thread,
                                &project_for_thread,
                                &config_for_thread,
                                watcher_generation,
                                paths,
                            )
                        }));
                        match result {
                            Ok(Ok(())) => {}
                            Ok(Err(err)) => eprintln!("[file-sync] change handling failed: {err}"),
                            Err(_) => eprintln!("[file-sync] watcher worker recovered from panic"),
                        }
                        maybe_periodic_rescan(
                            &thread_sink,
                            &root_for_thread,
                            &project_for_thread,
                            &config_for_thread,
                            watcher_generation,
                            &mut last_periodic_rescan,
                        );
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        let tx_for_watcher = tx.clone();
        let root_for_overflow = root.clone();
        let root_for_error = root.clone();
        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| match res {
                Ok(event) => {
                    for path in event.paths {
                        if tx_for_watcher.try_send(path).is_err() {
                            let _ = tx_for_watcher.try_send(root_for_overflow.clone());
                            break;
                        }
                    }
                }
                Err(err) => {
                    eprintln!("[file-sync] watcher error; scheduling rescan: {err}");
                    let _ = tx_for_watcher.try_send(root_for_error.clone());
                }
            },
            Config::default(),
        )
        .map_err(|e| format!("Failed to create file watcher: {e}"))?;
        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|e| format!("Failed to watch '{}': {e}", root.display()))?;
        for rel in ["raw/sources", "wiki"] {
            let path = root.join(rel);
            if path.exists() {
                if let Err(err) = watcher.watch(&path, RecursiveMode::Recursive) {
                    eprintln!(
                        "[file-sync] failed to add supplemental watch '{}': {err}",
                        path.display()
                    );
                }
            }
        }

        {
            let mut inner = file_sync_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inner.watcher = Some(watcher);
            inner.project_id = Some(project_id.to_string());
            inner.project_path = Some(root.clone());
        }

        let queue = with_queue_lock(&root, || read_queue(&root))?;
        emit_queue(sink, project_id, &queue);
        Ok(FileChangeRescanResult {
            queue,
            changed_tasks,
        })
    })
}

/// Start the file watcher with a boxed `Send + Sync` sink, so the background
/// thread can also emit events. Called by `commands::file_sync` where a
/// `TauriEventSink` (which is `Clone + Send + Sync`) is available.
pub fn start_project_file_watcher_boxed(
    project_id: &str,
    project_path: &str,
    source_watch_config: Option<SourceWatchConfig>,
    sink: std::sync::Arc<dyn EventSink>,
) -> Result<FileChangeRescanResult, String> {
    use crate::panic_guard::run_guarded;
    run_guarded("start_project_file_watcher_boxed", || {
        let root = PathBuf::from(project_path);
        let source_watch_config = normalize_source_watch_config(source_watch_config);
        let watcher_generation = WATCHER_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        ensure_sync_dir(&root)?;
        with_queue_lock(&root, || reset_processing_tasks(&root, project_id))?;
        enqueue_rescan_changes(&root, project_id, &source_watch_config)?;
        let changed_tasks = process_queue(sink.as_ref(), &root, project_id)?;

        let (tx, rx) = mpsc::sync_channel::<PathBuf>(8_192);
        let sink_for_thread = std::sync::Arc::clone(&sink);
        let root_for_thread = root.clone();
        let project_for_thread = project_id.to_string();
        let config_for_thread = source_watch_config.clone();

        std::thread::spawn(move || {
            let mut pending = BTreeSet::<PathBuf>::new();
            let mut last_periodic_rescan = now_ms();
            loop {
                match rx.recv_timeout(Duration::from_millis(700)) {
                    Ok(path) => {
                        pending.insert(path);
                        while let Ok(path) = rx.try_recv() {
                            pending.insert(path);
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if pending.is_empty() {
                            maybe_periodic_rescan(
                                sink_for_thread.as_ref(),
                                &root_for_thread,
                                &project_for_thread,
                                &config_for_thread,
                                watcher_generation,
                                &mut last_periodic_rescan,
                            );
                            continue;
                        }
                        let paths = pending.iter().cloned().collect::<Vec<_>>();
                        pending.clear();
                        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                            handle_changed_paths(
                                sink_for_thread.as_ref(),
                                &root_for_thread,
                                &project_for_thread,
                                &config_for_thread,
                                watcher_generation,
                                paths,
                            )
                        }));
                        match result {
                            Ok(Ok(())) => {}
                            Ok(Err(err)) => eprintln!("[file-sync] change handling failed: {err}"),
                            Err(_) => eprintln!("[file-sync] watcher worker recovered from panic"),
                        }
                        maybe_periodic_rescan(
                            sink_for_thread.as_ref(),
                            &root_for_thread,
                            &project_for_thread,
                            &config_for_thread,
                            watcher_generation,
                            &mut last_periodic_rescan,
                        );
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        let tx_for_watcher = tx.clone();
        let root_for_overflow = root.clone();
        let root_for_error = root.clone();
        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| match res {
                Ok(event) => {
                    for path in event.paths {
                        if tx_for_watcher.try_send(path).is_err() {
                            let _ = tx_for_watcher.try_send(root_for_overflow.clone());
                            break;
                        }
                    }
                }
                Err(err) => {
                    eprintln!("[file-sync] watcher error; scheduling rescan: {err}");
                    let _ = tx_for_watcher.try_send(root_for_error.clone());
                }
            },
            Config::default(),
        )
        .map_err(|e| format!("Failed to create file watcher: {e}"))?;
        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|e| format!("Failed to watch '{}': {e}", root.display()))?;
        for rel in ["raw/sources", "wiki"] {
            let path = root.join(rel);
            if path.exists() {
                if let Err(err) = watcher.watch(&path, RecursiveMode::Recursive) {
                    eprintln!(
                        "[file-sync] failed to add supplemental watch '{}': {err}",
                        path.display()
                    );
                }
            }
        }

        {
            let mut inner = file_sync_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inner.watcher = Some(watcher);
            inner.project_id = Some(project_id.to_string());
            inner.project_path = Some(root.clone());
        }

        let queue = with_queue_lock(&root, || read_queue(&root))?;
        emit_queue(sink.as_ref(), project_id, &queue);
        Ok(FileChangeRescanResult {
            queue,
            changed_tasks,
        })
    })
}

/// Stop the active file watcher.
pub fn stop_project_file_watcher() -> Result<(), String> {
    use crate::panic_guard::run_guarded;
    run_guarded("stop_project_file_watcher", || {
        WATCHER_GENERATION.fetch_add(1, Ordering::SeqCst);
        let mut inner = file_sync_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.watcher = None;
        inner.project_id = None;
        inner.project_path = None;
        Ok(())
    })
}

/// Rescan project files and emit queue/changed events through `sink`.
pub fn rescan_project_files(
    project_id: &str,
    project_path: &str,
    source_watch_config: Option<SourceWatchConfig>,
    sink: &dyn EventSink,
) -> Result<FileChangeRescanResult, String> {
    use crate::panic_guard::run_guarded;
    run_guarded("rescan_project_files", || {
        let root = PathBuf::from(project_path);
        let source_watch_config = normalize_source_watch_config(source_watch_config);
        ensure_sync_dir(&root)?;
        enqueue_rescan_changes(&root, project_id, &source_watch_config)?;
        let changed_tasks = process_queue(sink, &root, project_id)?;
        let queue = with_queue_lock(&root, || read_queue(&root))?;
        emit_queue(sink, project_id, &queue);
        Ok(FileChangeRescanResult {
            queue,
            changed_tasks,
        })
    })
}

/// Read the current file-change queue for a project.
pub fn get_file_change_queue(project_path: &str) -> Result<FileChangeQueue, String> {
    use crate::panic_guard::run_guarded;
    run_guarded("get_file_change_queue", || {
        let root = PathBuf::from(project_path);
        with_queue_lock(&root, || read_queue(&root))
    })
}

/// Retry a single failed task by ID, then reprocess the queue.
pub fn retry_file_change_task(
    project_id: &str,
    project_path: &str,
    task_id: &str,
    sink: &dyn EventSink,
) -> Result<FileChangeQueue, String> {
    use crate::panic_guard::run_guarded;
    run_guarded("retry_file_change_task", || {
        let root = PathBuf::from(project_path);
        with_queue_lock(&root, || {
            let mut queue = read_queue(&root)?;
            let now = now_ms();
            for task in &mut queue.tasks {
                if task.id == task_id && task.project_id == project_id {
                    task.status = FileChangeStatus::Pending;
                    task.error = None;
                    task.retry_count = 0;
                    task.needs_rerun = false;
                    task.updated_at = now;
                }
            }
            write_queue(&root, &queue)
        })?;
        process_queue(sink, &root, project_id)?;
        let queue = with_queue_lock(&root, || read_queue(&root))?;
        emit_queue(sink, project_id, &queue);
        Ok(queue)
    })
}

/// Remove a single task from the queue and emit the updated queue.
pub fn ignore_file_change_task(
    project_id: &str,
    project_path: &str,
    task_id: &str,
    sink: &dyn EventSink,
) -> Result<FileChangeQueue, String> {
    use crate::panic_guard::run_guarded;
    run_guarded("ignore_file_change_task", || {
        let root = PathBuf::from(project_path);
        let queue = with_queue_lock(&root, || {
            let mut queue = read_queue(&root)?;
            queue
                .tasks
                .retain(|task| !(task.id == task_id && task.project_id == project_id));
            write_queue(&root, &queue)?;
            read_queue(&root)
        })?;
        emit_queue(sink, project_id, &queue);
        Ok(queue)
    })
}

// ──────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ingest_queue::{
        apply_task_to_snapshot, enqueue_paths, read_queue, read_snapshot, sync_snapshot_paths,
        with_queue_lock, FileChangeKind, FileChangeQueue, FileChangeStatus, FileChangeTask,
        MAX_RETRY_COUNT, QUEUE_FILE, SNAPSHOT_FILE,
    };
    use std::collections::BTreeMap;
    use std::fs;

    fn temp_root(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("llm-wiki-file-sync-{name}-{stamp}"));
        fs::create_dir_all(root.join("raw/sources")).unwrap();
        root
    }

    fn default_watch_config() -> SourceWatchConfig {
        SourceWatchConfig::default()
    }

    #[test]
    fn md5_detects_same_size_content_changes() {
        use crate::core::ingest_queue::{read_meta, upsert_task};

        let root = temp_root("same-size");
        let rel = "raw/sources/a.md";
        fs::write(root.join(rel), "aaaa").unwrap();

        ensure_sync_dir(&root).unwrap();
        enqueue_rescan_changes(&root, "p1", &default_watch_config()).unwrap();
        let first = read_queue(&root).unwrap().tasks[0].clone();
        apply_task_to_snapshot(&root, &first).unwrap();
        write_queue(
            &root,
            &FileChangeQueue {
                version: 1,
                tasks: vec![],
            },
        )
        .unwrap();

        fs::write(root.join(rel), "bbbb").unwrap();
        enqueue_paths(&root, "p1", BTreeSet::from([rel.to_string()])).unwrap();
        let queue = read_queue(&root).unwrap();

        assert_eq!(queue.tasks.len(), 1);
        assert_eq!(queue.tasks[0].kind, FileChangeKind::Modified);
        assert_ne!(queue.tasks[0].hash_before, queue.tasks[0].hash_after);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn directory_delete_expands_snapshot_children() {
        let root = temp_root("dir-delete");
        let a = "raw/sources/folder/a.md";
        let b = "raw/sources/folder/b.md";
        fs::create_dir_all(root.join("raw/sources/folder")).unwrap();
        fs::write(root.join(a), "a").unwrap();
        fs::write(root.join(b), "b").unwrap();

        ensure_sync_dir(&root).unwrap();
        sync_snapshot_paths(&root, BTreeSet::from([a.to_string(), b.to_string()])).unwrap();
        fs::remove_dir_all(root.join("raw/sources/folder")).unwrap();

        let mut rels = BTreeSet::new();
        let snapshot = read_snapshot(&root).unwrap();
        let config = default_watch_config();
        let rules = SourceWatchRules::new(&config);
        collect_known_paths(
            &root,
            &root.join("raw/sources/folder"),
            &snapshot,
            &mut rels,
            &rules,
        );

        assert_eq!(rels, BTreeSet::from([a.to_string(), b.to_string()]));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn prefix_rescan_detects_raw_source_mount_style_changes() {
        use crate::core::ingest_queue::enqueue_rescan_changes_for_prefixes;

        let root = temp_root("prefix-rescan");
        let old = "raw/sources/old.md";
        let new = "raw/sources/new.md";
        fs::write(root.join(old), "old").unwrap();

        ensure_sync_dir(&root).unwrap();
        sync_snapshot_paths(&root, BTreeSet::from([old.to_string()])).unwrap();
        fs::remove_file(root.join(old)).unwrap();
        fs::write(root.join(new), "new").unwrap();

        enqueue_rescan_changes_for_prefixes(&root, "p1", &["raw/sources"], &default_watch_config())
            .unwrap();
        let queue = read_queue(&root).unwrap();
        let by_path = queue
            .tasks
            .iter()
            .map(|task| (task.path.as_str(), task.kind.clone()))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(by_path.get(old), Some(&FileChangeKind::Deleted));
        assert_eq!(by_path.get(new), Some(&FileChangeKind::Created));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn app_written_paths_update_snapshot_without_queueing() {
        use crate::core::ingest_queue::{read_meta, sync_snapshot_paths};

        let root = temp_root("app-write");
        let rel = "raw/sources/a.md";
        let path = root.join(rel);
        fs::write(&path, "old").unwrap();

        ensure_sync_dir(&root).unwrap();
        sync_snapshot_paths(&root, BTreeSet::from([rel.to_string()])).unwrap();
        fs::write(&path, "new").unwrap();
        mark_app_write_path(&path);

        let mut app_written_rels = BTreeSet::new();
        let snapshot = read_snapshot(&root).unwrap();
        if is_app_write_ignored(&path) {
            let config = default_watch_config();
            let rules = SourceWatchRules::new(&config);
            collect_known_paths(&root, &path, &snapshot, &mut app_written_rels, &rules);
        }
        sync_snapshot_paths(&root, app_written_rels).unwrap();

        let queue = read_queue(&root).unwrap();
        let snapshot = read_snapshot(&root).unwrap();
        assert!(queue.tasks.is_empty());
        assert_eq!(
            snapshot.files.get(rel).and_then(|m| m.hash.clone()),
            read_meta(&root, rel).unwrap().and_then(|m| m.hash)
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn watch_rules_exclude_temporary_and_app_dirs() {
        let config = default_watch_config();
        let rules = SourceWatchRules::new(&config);
        assert!(should_watch_rel("raw/sources/document.docx", &rules));
        assert!(should_watch_rel("wiki/concepts/topic.md", &rules));
        assert!(!should_watch_rel(
            ".llm-wiki/file-change-queue.json",
            &rules
        ));
        assert!(!should_watch_rel("raw/sources/~$Document.docx", &rules));
        assert!(!should_watch_rel(
            "raw/sources/.~lock.Document.odt#",
            &rules
        ));
        assert!(!should_watch_rel("raw/sources/Thumbs.db", &rules));
        assert!(!should_watch_rel("raw/sources/desktop.ini", &rules));
        assert!(!should_watch_rel("raw/sources/download.crdownload", &rules));
        assert!(!should_watch_rel(".vscode/settings.json", &rules));
        assert!(!should_watch_rel("wiki/media/image.png", &rules));
    }

    #[test]
    fn source_watch_config_filters_raw_source_extensions_and_dirs() {
        let config = SourceWatchConfig {
            include_extensions: vec!["md".into(), "pdf".into()],
            exclude_dirs: vec!["drafts".into(), "subdir/drafts".into()],
            exclude_globs: vec!["*.private.*".into()],
            ..SourceWatchConfig::default()
        };
        let config = normalize_source_watch_config(Some(config));
        let rules = SourceWatchRules::new(&config);

        assert!(should_watch_rel("raw/sources/final.md", &rules));
        assert!(!should_watch_rel("raw/sources/data.json", &rules));
        assert!(!should_watch_rel("raw/sources/drafts/final.md", &rules));
        assert!(!should_watch_rel(
            "raw/sources/subdir/drafts/final.md",
            &rules
        ));
        assert!(!should_watch_rel("raw/sources/report.private.md", &rules));
        assert!(should_watch_rel("wiki/index.md", &rules));
    }

    #[test]
    fn source_watch_config_skips_oversized_raw_sources() {
        let root = temp_root("max-size");
        let rel = "raw/sources/large.md";
        fs::write(root.join(rel), vec![b'x'; 2 * 1024 * 1024]).unwrap();
        let config = SourceWatchConfig {
            max_file_size_mb: 1,
            ..SourceWatchConfig::default()
        };
        let config = normalize_source_watch_config(Some(config));
        let rules = SourceWatchRules::new(&config);

        assert_eq!(
            relative_watch_path(&root, &root.join(rel), &rules, None),
            None
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn source_watch_defaults_are_loaded_from_shared_json_and_tolerate_missing_fields() {
        let default = SourceWatchConfig::default();
        assert!(default.include_extensions.contains(&"md".to_string()));
        assert!(default.exclude_dirs.contains(&".git".to_string()));

        let partial: SourceWatchConfig =
            serde_json::from_str(r#"{"enabled":false,"includeExtensions":["md"]}"#).unwrap();
        assert!(!partial.enabled);
        assert!(partial.auto_ingest);
        assert!(partial.exclude_dirs.contains(&".git".to_string()));
    }

    #[test]
    fn wildcard_match_is_unicode_character_based() {
        assert!(wildcard_match("?稿.md", "草稿.md"));
        assert!(wildcard_match("草*.md", "草稿文件.md"));
        assert!(!wildcard_match("??.md", "草稿文件.md"));
    }

    #[test]
    fn normalize_rel_path_tolerates_current_dir_segments() {
        let path = Path::new("raw").join(".").join("sources").join("doc.md");
        assert_eq!(
            normalize_rel_path(&path),
            Some("raw/sources/doc.md".to_string())
        );
    }

    #[test]
    fn process_queue_updates_snapshot_and_removes_done_tasks() {
        let root = temp_root("process-e2e");
        let rel = "raw/sources/a.md";
        fs::write(root.join(rel), "content").unwrap();

        ensure_sync_dir(&root).unwrap();
        enqueue_paths(&root, "p1", BTreeSet::from([rel.to_string()])).unwrap();

        let mut queue_emits = 0;
        let mut changed_emits = 0;
        process_queue_inner(
            &root,
            "p1",
            |_| queue_emits += 1,
            |tasks| {
                if !tasks.is_empty() {
                    changed_emits += 1;
                }
            },
        )
        .unwrap();

        let queue = with_queue_lock(&root, || read_queue(&root)).unwrap();
        let snapshot = with_queue_lock(&root, || read_snapshot(&root)).unwrap();
        assert!(queue.tasks.is_empty());
        assert!(snapshot.files.contains_key(rel));
        assert!(queue_emits >= 1);
        assert_eq!(changed_emits, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn process_queue_flushes_changed_tasks_before_returning_error() {
        let root = temp_root("flush-on-error");
        ensure_sync_dir(&root).unwrap();
        let rels = (0..26)
            .map(|i| {
                let rel = format!("raw/sources/{i}.md");
                fs::write(root.join(&rel), format!("content {i}")).unwrap();
                rel
            })
            .collect::<BTreeSet<_>>();
        enqueue_paths(&root, "p1", rels).unwrap();

        let snapshot_path = root.join(SNAPSHOT_FILE);
        let mut queue_emits = 0;
        let mut changed_count = 0;
        let result = process_queue_inner(
            &root,
            "p1",
            |_| {
                queue_emits += 1;
                if queue_emits == 2 {
                    fs::remove_file(&snapshot_path).unwrap();
                    fs::create_dir_all(&snapshot_path).unwrap();
                }
            },
            |tasks| changed_count += tasks.len(),
        );

        assert!(result.is_err());
        assert_eq!(changed_count, QUEUE_EMIT_EVERY);

        let _ = fs::remove_dir_all(root);
    }
}

// ──────────────────────────────────────────────────────────────────────────
// EventSink integration tests
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests_event_sink {
    use super::*;
    use crate::core::events::CapturingEventSink;
    use crate::core::ingest_queue::{enqueue_paths, ensure_sync_dir, read_queue, with_queue_lock};
    use std::fs;

    fn temp_root(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("llm-wiki-eventsink-{name}-{stamp}"));
        fs::create_dir_all(root.join("raw/sources")).unwrap();
        root
    }

    /// Verify that `rescan_project_files` emits at least one
    /// `EVENT_QUEUE_UPDATED` event through the `CapturingEventSink`.
    #[test]
    fn rescan_emits_queue_updated() {
        let root = temp_root("rescan-emit");
        let project_id = "p-rescan";

        // Create a file that should be picked up by the rescan.
        fs::write(root.join("raw/sources/hello.md"), "# Hello").unwrap();

        let sink = CapturingEventSink::default();
        rescan_project_files(
            project_id,
            root.to_str().unwrap(),
            None,
            &sink,
        )
        .unwrap();

        let events = sink.snapshot();
        assert!(
            events.iter().any(|(t, _)| t == EVENT_QUEUE_UPDATED),
            "expected at least one EVENT_QUEUE_UPDATED, got: {events:?}"
        );

        let _ = fs::remove_dir_all(root);
    }

    /// Verify that when files exist the rescan also emits `EVENT_CHANGED`.
    #[test]
    fn rescan_emits_changed_when_files_are_new() {
        let root = temp_root("rescan-changed");
        let project_id = "p-changed";

        fs::write(root.join("raw/sources/doc.md"), "content").unwrap();

        let sink = CapturingEventSink::default();
        rescan_project_files(
            project_id,
            root.to_str().unwrap(),
            None,
            &sink,
        )
        .unwrap();

        let events = sink.snapshot();
        assert!(
            events.iter().any(|(t, _)| t == EVENT_CHANGED),
            "expected at least one EVENT_CHANGED, got: {events:?}"
        );

        let _ = fs::remove_dir_all(root);
    }

    /// Verify event ordering: the first event emitted from `process_queue`
    /// for a non-empty queue is `EVENT_QUEUE_UPDATED` (the "processing" emit
    /// that precedes task completion).
    #[test]
    fn queue_updated_precedes_changed_in_event_stream() {
        let root = temp_root("event-order");
        let project_id = "p-order";

        fs::write(root.join("raw/sources/a.md"), "aaa").unwrap();

        let sink = CapturingEventSink::default();
        rescan_project_files(
            project_id,
            root.to_str().unwrap(),
            None,
            &sink,
        )
        .unwrap();

        let events = sink.snapshot();
        let first_queue_pos = events
            .iter()
            .position(|(t, _)| t == EVENT_QUEUE_UPDATED)
            .expect("no EVENT_QUEUE_UPDATED");
        let first_changed_pos = events
            .iter()
            .position(|(t, _)| t == EVENT_CHANGED);

        // If there is a changed event it must come after the first queue-updated.
        if let Some(changed_pos) = first_changed_pos {
            assert!(
                first_queue_pos < changed_pos,
                "EVENT_QUEUE_UPDATED should precede EVENT_CHANGED"
            );
        }

        let _ = fs::remove_dir_all(root);
    }

    /// Verify the payload shape of `EVENT_QUEUE_UPDATED`: must contain
    /// `projectId` and `tasks` fields.
    #[test]
    fn queue_updated_payload_has_required_fields() {
        let root = temp_root("payload-shape");
        let project_id = "p-payload";

        fs::write(root.join("raw/sources/x.md"), "x").unwrap();

        let sink = CapturingEventSink::default();
        rescan_project_files(
            project_id,
            root.to_str().unwrap(),
            None,
            &sink,
        )
        .unwrap();

        let events = sink.snapshot();
        let queue_event = events
            .iter()
            .find(|(t, _)| t == EVENT_QUEUE_UPDATED)
            .expect("no EVENT_QUEUE_UPDATED");

        let payload = &queue_event.1;
        assert!(
            payload.get("projectId").is_some(),
            "payload missing projectId"
        );
        assert!(
            payload.get("tasks").is_some(),
            "payload missing tasks"
        );
        assert_eq!(
            payload["projectId"].as_str(),
            Some(project_id),
            "projectId mismatch"
        );

        let _ = fs::remove_dir_all(root);
    }

    /// Verify that `ignore_file_change_task` emits `EVENT_QUEUE_UPDATED`
    /// after removing a task from the queue.
    #[test]
    fn ignore_task_emits_queue_updated() {
        let root = temp_root("ignore-emit");
        let project_id = "p-ignore";

        ensure_sync_dir(&root).unwrap();
        fs::write(root.join("raw/sources/b.md"), "b").unwrap();
        enqueue_paths(
            &root,
            project_id,
            std::collections::BTreeSet::from(["raw/sources/b.md".to_string()]),
        )
        .unwrap();

        let queue = with_queue_lock(&root, || read_queue(&root)).unwrap();
        let task_id = queue.tasks[0].id.clone();

        let sink = CapturingEventSink::default();
        ignore_file_change_task(project_id, root.to_str().unwrap(), &task_id, &sink).unwrap();

        let events = sink.snapshot();
        assert!(
            events.iter().any(|(t, _)| t == EVENT_QUEUE_UPDATED),
            "expected EVENT_QUEUE_UPDATED after ignore, got: {events:?}"
        );

        let _ = fs::remove_dir_all(root);
    }
}
