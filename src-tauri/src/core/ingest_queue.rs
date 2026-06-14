//! Persistent serial change-queue used by `core::file_sync`.
//!
//! Holds the file-change queue and file-snapshot data types, plus all the
//! mutation helpers that add tasks, persist state to disk, and read it back.
//! The queue is kept as a JSON file inside `<project_root>/.llm-wiki/` and
//! is protected by a per-root `Mutex` stored in a module-level `OnceLock`.
//!
//! Phase 4 `core::sources` may also use these helpers.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};

// ──────────────────────────────────────────────────────────────────────────
// Error type
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum IngestQueueError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(String),
    #[error("internal: {0}")]
    Internal(String),
}

impl IngestQueueError {
    pub fn internal(msg: impl Into<String>) -> Self {
        IngestQueueError::Internal(msg.into())
    }
}

// Convenience alias so callers can use `?` with functions that return
// `Result<_, String>` (the legacy signature used inside this module).
impl From<String> for IngestQueueError {
    fn from(s: String) -> Self {
        IngestQueueError::Internal(s)
    }
}

// ──────────────────────────────────────────────────────────────────────────
// File paths / limits
// ──────────────────────────────────────────────────────────────────────────

pub(crate) const SNAPSHOT_FILE: &str = ".llm-wiki/file-snapshot.json";
pub(crate) const QUEUE_FILE: &str = ".llm-wiki/file-change-queue.json";
pub(crate) const MAX_HASH_BYTES: u64 = 32 * 1024 * 1024;
pub(crate) const MAX_RETRY_COUNT: u32 = 3;

// ──────────────────────────────────────────────────────────────────────────
// Module-level state
// ──────────────────────────────────────────────────────────────────────────

static QUEUE_LOCKS: OnceLock<Mutex<BTreeMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

// ──────────────────────────────────────────────────────────────────────────
// Public data types
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMeta {
    pub hash: Option<String>,
    pub size: u64,
    pub mtime_ms: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSnapshot {
    pub version: u32,
    pub updated_at: i64,
    pub files: BTreeMap<String, FileMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FileChangeKind {
    Created,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FileChangeStatus {
    Pending,
    Processing,
    Done,
    Failed,
    Superseded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeTask {
    pub id: String,
    pub project_id: String,
    pub path: String,
    pub kind: FileChangeKind,
    pub status: FileChangeStatus,
    pub hash_before: Option<String>,
    pub hash_after: Option<String>,
    pub size: Option<u64>,
    pub mtime_ms: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub retry_count: u32,
    pub error: Option<String>,
    pub needs_rerun: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeQueue {
    pub version: u32,
    pub tasks: Vec<FileChangeTask>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeRescanResult {
    pub queue: FileChangeQueue,
    pub changed_tasks: Vec<FileChangeTask>,
}

// ──────────────────────────────────────────────────────────────────────────
// Queue / snapshot persistence
// ──────────────────────────────────────────────────────────────────────────

pub fn read_snapshot(root: &Path) -> Result<FileSnapshot, String> {
    read_json(root.join(SNAPSHOT_FILE)).map(|mut s: FileSnapshot| {
        if s.version == 0 {
            s.version = 1;
        }
        s
    })
}

pub fn write_snapshot(root: &Path, snapshot: &FileSnapshot) -> Result<(), String> {
    write_json(root.join(SNAPSHOT_FILE), snapshot)
}

pub fn read_queue(root: &Path) -> Result<FileChangeQueue, String> {
    read_json(root.join(QUEUE_FILE)).map(|mut q: FileChangeQueue| {
        if q.version == 0 {
            q.version = 1;
        }
        q
    })
}

pub fn write_queue(root: &Path, queue: &FileChangeQueue) -> Result<(), String> {
    write_json(root.join(QUEUE_FILE), queue)
}

fn read_json<T>(path: PathBuf) -> Result<T, String>
where
    T: Default + for<'de> Deserialize<'de>,
{
    if !path.exists() {
        return Ok(T::default());
    }
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read '{}': {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("Failed to parse '{}': {e}", path.display()))
}

fn write_json<T: Serialize>(path: PathBuf, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create '{}': {e}", parent.display()))?;
    }
    let text =
        serde_json::to_string_pretty(value).map_err(|e| format!("JSON encode failed: {e}"))?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "file-sync.json".to_string());
    let tmp_path = path.with_file_name(format!(
        ".{file_name}.{}.tmp",
        chrono::Utc::now()
            .timestamp_nanos_opt()
            .unwrap_or_else(now_ms)
    ));
    fs::write(&tmp_path, text)
        .map_err(|e| format!("Failed to write '{}': {e}", tmp_path.display()))?;
    #[cfg(windows)]
    {
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|e| format!("Failed to replace '{}': {e}", path.display()))?;
        }
    }
    fs::rename(&tmp_path, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        format!(
            "Failed to move '{}' to '{}': {e}",
            tmp_path.display(),
            path.display()
        )
    })
}

// ──────────────────────────────────────────────────────────────────────────
// Per-root queue locking
// ──────────────────────────────────────────────────────────────────────────

pub fn queue_lock_for(root: &Path) -> Arc<Mutex<()>> {
    let key = path_key(root);
    let mut locks = QUEUE_LOCKS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks
        .entry(key)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

pub fn with_queue_lock<T>(root: &Path, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let lock = queue_lock_for(root);
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    f()
}

// ──────────────────────────────────────────────────────────────────────────
// Path helpers
// ──────────────────────────────────────────────────────────────────────────

pub fn path_key(path: &Path) -> String {
    if let Ok(canonical) = path.canonicalize() {
        return normalize_path_key(&canonical);
    }

    let mut existing = path.to_path_buf();
    let mut suffix = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name().map(|name| name.to_os_string()) else {
            return normalize_path_key(path);
        };
        suffix.push(name);
        if !existing.pop() {
            return normalize_path_key(path);
        }
    }

    let Ok(mut canonical) = existing.canonicalize() else {
        return normalize_path_key(path);
    };
    for part in suffix.iter().rev() {
        canonical.push(part);
    }
    normalize_path_key(&canonical)
}

pub fn normalize_path_key(path: &Path) -> String {
    normalize_key(&path.to_string_lossy().replace('\\', "/"))
}

pub fn normalize_key(path: &str) -> String {
    if cfg!(windows) {
        path.to_lowercase()
    } else {
        path.to_string()
    }
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn stable_path_hash(path: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(path.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    digest[..12].to_string()
}

// ──────────────────────────────────────────────────────────────────────────
// File metadata
// ──────────────────────────────────────────────────────────────────────────

pub fn read_meta(root: &Path, rel: &str) -> Result<Option<FileMeta>, String> {
    let path = root.join(rel);
    if !path.exists() {
        return Ok(None);
    }
    let meta = fs::metadata(&path).map_err(|e| format!("metadata failed for {rel}: {e}"))?;
    if !meta.is_file() {
        return Ok(None);
    }
    let size = meta.len();
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let hash = if size <= MAX_HASH_BYTES {
        Some(md5_file(&path)?)
    } else {
        None
    };
    Ok(Some(FileMeta {
        hash,
        size,
        mtime_ms,
    }))
}

pub fn read_meta_fast(root: &Path, rel: &str) -> Result<Option<FileMeta>, String> {
    let path = root.join(rel);
    if !path.exists() {
        return Ok(None);
    }
    let meta = fs::metadata(&path).map_err(|e| format!("metadata failed for {rel}: {e}"))?;
    if !meta.is_file() {
        return Ok(None);
    }
    let size = meta.len();
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Ok(Some(FileMeta {
        hash: None,
        size,
        mtime_ms,
    }))
}

fn md5_file(path: &Path) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|e| format!("open failed for '{}': {e}", path.display()))?;
    let mut hasher = Md5::new();
    let mut buf = [0_u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("read failed for '{}': {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

// ──────────────────────────────────────────────────────────────────────────
// Queue mutation: upsert / enqueue
// ──────────────────────────────────────────────────────────────────────────

pub fn merge_kind(existing: &FileChangeKind, incoming: &FileChangeKind) -> FileChangeKind {
    match (existing, incoming) {
        (FileChangeKind::Deleted, FileChangeKind::Created)
        | (FileChangeKind::Created, FileChangeKind::Deleted)
        | (_, FileChangeKind::Modified) => FileChangeKind::Modified,
        (_, kind) => kind.clone(),
    }
}

pub fn upsert_task(
    queue: &mut FileChangeQueue,
    project_id: &str,
    rel: &str,
    kind: FileChangeKind,
    old: Option<FileMeta>,
    new: Option<FileMeta>,
    now: i64,
) {
    if let Some(task) = queue.tasks.iter_mut().find(|t| {
        t.project_id == project_id
            && normalize_key(&t.path) == normalize_key(rel)
            && matches!(
                t.status,
                FileChangeStatus::Pending | FileChangeStatus::Processing | FileChangeStatus::Failed
            )
    }) {
        task.kind = merge_kind(&task.kind, &kind);
        task.hash_after = new.as_ref().and_then(|m| m.hash.clone());
        task.size = new.as_ref().map(|m| m.size);
        task.mtime_ms = new.as_ref().map(|m| m.mtime_ms);
        task.updated_at = now;
        if task.status == FileChangeStatus::Failed {
            if task.retry_count < MAX_RETRY_COUNT {
                task.status = FileChangeStatus::Pending;
                task.error = None;
            } else {
                task.error = Some(format!("Retry limit reached ({MAX_RETRY_COUNT})"));
            }
        } else if task.status == FileChangeStatus::Processing {
            task.needs_rerun = true;
            task.error = None;
        } else {
            task.error = None;
        }
        return;
    }

    queue.tasks.push(FileChangeTask {
        id: format!("change_{}_{}", now, stable_path_hash(rel)),
        project_id: project_id.to_string(),
        path: rel.to_string(),
        kind,
        status: FileChangeStatus::Pending,
        hash_before: old.and_then(|m| m.hash),
        hash_after: new.as_ref().and_then(|m| m.hash.clone()),
        size: new.as_ref().map(|m| m.size),
        mtime_ms: new.as_ref().map(|m| m.mtime_ms),
        created_at: now,
        updated_at: now,
        retry_count: 0,
        error: None,
        needs_rerun: false,
    });
}

pub fn enqueue_paths(root: &Path, project_id: &str, rels: BTreeSet<String>) -> Result<(), String> {
    let snapshot = with_queue_lock(root, || read_snapshot(root))?;
    let now = now_ms();
    let mut changes = Vec::new();

    for rel in rels {
        let old = snapshot.files.get(&rel).cloned();
        // Intentional TOCTOU trade-off: `read_meta` can be expensive
        // because it may hash file contents, so it runs outside the queue
        // lock. If another worker updates the snapshot before this task is
        // enqueued, the task may be redundant; processing it is harmless and
        // self-corrects by writing the current on-disk meta back to snapshot.
        let new = read_meta(root, &rel)?;
        if old.as_ref().map(|m| (&m.hash, m.size)) == new.as_ref().map(|m| (&m.hash, m.size)) {
            continue;
        }

        let kind = match (&old, &new) {
            (None, Some(_)) => FileChangeKind::Created,
            (Some(_), None) => FileChangeKind::Deleted,
            (Some(_), Some(_)) => FileChangeKind::Modified,
            (None, None) => continue,
        };
        changes.push((rel, kind, old, new));
    }

    if changes.is_empty() {
        return Ok(());
    }

    with_queue_lock(root, || {
        let mut queue = read_queue(root)?;
        for (rel, kind, old, new) in changes {
            upsert_task(&mut queue, project_id, &rel, kind, old, new, now);
        }
        write_queue(root, &queue)
    })
}

pub fn enqueue_rescan_changes(
    root: &Path,
    project_id: &str,
    source_watch_config: &crate::core::file_sync::SourceWatchConfig,
) -> Result<(), String> {
    use crate::core::file_sync::{SourceWatchRules, relative_watch_path};
    use walkdir::WalkDir;

    let rules = SourceWatchRules::new(source_watch_config);
    let mut rels = BTreeSet::<String>::new();
    for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
        if entry.file_type().is_file() {
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

    let snapshot = with_queue_lock(root, || read_snapshot(root))?;
    for rel in snapshot.files.keys() {
        if !root.join(rel).exists() {
            rels.insert(rel.clone());
        }
    }
    enqueue_paths(root, project_id, rels)
}

pub fn enqueue_rescan_changes_for_prefixes(
    root: &Path,
    project_id: &str,
    prefixes: &[&str],
    source_watch_config: &crate::core::file_sync::SourceWatchConfig,
) -> Result<(), String> {
    use crate::core::file_sync::{SourceWatchRules, relative_watch_path};
    use walkdir::WalkDir;

    let rules = SourceWatchRules::new(source_watch_config);
    let mut rels = BTreeSet::<String>::new();
    let snapshot = with_queue_lock(root, || read_snapshot(root))?;
    for prefix in prefixes {
        let path = root.join(prefix);
        if path.is_file() {
            if let Some(rel) = relative_watch_path(
                root,
                &path,
                &rules,
                fs::metadata(&path).ok().map(|m| m.len()),
            ) {
                let old = snapshot.files.get(&rel);
                let fast = read_meta_fast(root, &rel)?;
                if old.map(|m| (m.size, m.mtime_ms)) != fast.as_ref().map(|m| (m.size, m.mtime_ms))
                {
                    rels.insert(rel);
                }
            }
        } else if path.exists() {
            for entry in WalkDir::new(&path).into_iter().filter_map(Result::ok) {
                if entry.file_type().is_file() {
                    if let Some(rel) = relative_watch_path(
                        root,
                        entry.path(),
                        &rules,
                        entry.metadata().ok().map(|m| m.len()),
                    ) {
                        let old = snapshot.files.get(&rel);
                        let fast = read_meta_fast(root, &rel)?;
                        if old.map(|m| (m.size, m.mtime_ms))
                            != fast.as_ref().map(|m| (m.size, m.mtime_ms))
                        {
                            rels.insert(rel);
                        }
                    }
                }
            }
        }
    }

    for rel in snapshot.files.keys() {
        if prefixes
            .iter()
            .any(|prefix| rel == *prefix || rel.starts_with(&format!("{prefix}/")))
            && !root.join(rel).exists()
        {
            rels.insert(rel.clone());
        }
    }

    enqueue_paths(root, project_id, rels)
}

pub fn sync_snapshot_paths(root: &Path, rels: BTreeSet<String>) -> Result<(), String> {
    let metas = rels
        .into_iter()
        .map(|rel| read_meta(root, &rel).map(|meta| (rel, meta)))
        .collect::<Result<Vec<_>, _>>()?;

    with_queue_lock(root, || {
        let mut snapshot = read_snapshot(root)?;
        for (rel, meta) in metas {
            match meta {
                Some(meta) => {
                    snapshot.files.insert(rel, meta);
                }
                None => {
                    snapshot.files.remove(&rel);
                }
            }
        }
        snapshot.version = 1;
        snapshot.updated_at = now_ms();
        write_snapshot(root, &snapshot)
    })
}

pub fn reset_processing_tasks(root: &Path, project_id: &str) -> Result<(), String> {
    let mut queue = read_queue(root)?;
    let mut changed = false;
    queue.tasks.retain(|task| task.project_id == project_id);
    for task in &mut queue.tasks {
        if task.status == FileChangeStatus::Processing {
            task.status = FileChangeStatus::Pending;
            task.needs_rerun = false;
            task.error = None;
            task.updated_at = now_ms();
            changed = true;
        }
    }
    if changed {
        write_queue(root, &queue)?;
    }
    Ok(())
}

pub fn write_task_meta_to_snapshot(
    root: &Path,
    task: &FileChangeTask,
    meta: Option<FileMeta>,
) -> Result<(), String> {
    let mut snapshot = read_snapshot(root)?;
    match meta {
        Some(meta) => {
            snapshot.files.insert(task.path.clone(), meta);
        }
        None => {
            snapshot.files.remove(&task.path);
        }
    }
    snapshot.version = 1;
    snapshot.updated_at = now_ms();
    write_snapshot(root, &snapshot)
}

pub fn ensure_sync_dir(root: &Path) -> Result<(), String> {
    fs::create_dir_all(root.join(".llm-wiki"))
        .map_err(|e| format!("Failed to create .llm-wiki: {e}"))
}

// ──────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub fn apply_task_to_snapshot(root: &Path, task: &FileChangeTask) -> Result<(), String> {
    let meta = read_meta(root, &task.path)?;
    with_queue_lock(root, || write_task_meta_to_snapshot(root, task, meta))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("llm-wiki-ingest-queue-{name}-{stamp}"));
        fs::create_dir_all(root.join("raw/sources")).unwrap();
        root
    }

    #[test]
    fn repeated_changes_upsert_one_pending_task() {
        let root = temp_root("dedupe");
        let rel = "raw/sources/a.md";
        fs::write(root.join(rel), "one").unwrap();

        ensure_sync_dir(&root).unwrap();
        enqueue_paths(&root, "p1", BTreeSet::from([rel.to_string()])).unwrap();
        fs::write(root.join(rel), "two").unwrap();
        enqueue_paths(&root, "p1", BTreeSet::from([rel.to_string()])).unwrap();

        let queue = read_queue(&root).unwrap();
        assert_eq!(queue.tasks.len(), 1);
        assert_eq!(queue.tasks[0].status, FileChangeStatus::Pending);
        assert_eq!(queue.tasks[0].path, rel);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn retry_limit_keeps_failed_task_failed_on_new_changes() {
        let root = temp_root("retry-limit");
        let rel = "raw/sources/a.md";
        fs::write(root.join(rel), "one").unwrap();

        ensure_sync_dir(&root).unwrap();
        let mut queue = FileChangeQueue {
            version: 1,
            tasks: vec![FileChangeTask {
                id: "t1".into(),
                project_id: "p1".into(),
                path: rel.into(),
                kind: FileChangeKind::Modified,
                status: FileChangeStatus::Failed,
                hash_before: None,
                hash_after: None,
                size: None,
                mtime_ms: None,
                created_at: 1,
                updated_at: 1,
                retry_count: MAX_RETRY_COUNT,
                error: Some("failed".into()),
                needs_rerun: false,
            }],
        };
        upsert_task(
            &mut queue,
            "p1",
            rel,
            FileChangeKind::Modified,
            None,
            read_meta(&root, rel).unwrap(),
            now_ms(),
        );

        assert_eq!(queue.tasks.len(), 1);
        assert_eq!(queue.tasks[0].status, FileChangeStatus::Failed);
        assert_eq!(queue.tasks[0].retry_count, MAX_RETRY_COUNT);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn concurrent_enqueue_paths_do_not_drop_tasks() {
        let root = temp_root("concurrent");
        ensure_sync_dir(&root).unwrap();
        let mut handles = Vec::new();
        for i in 0..16 {
            let root = root.clone();
            let rel = format!("raw/sources/{i}.md");
            fs::write(root.join(&rel), format!("content {i}")).unwrap();
            handles.push(std::thread::spawn(move || {
                enqueue_paths(&root, "p1", BTreeSet::from([rel])).unwrap();
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let queue = with_queue_lock(&root, || read_queue(&root)).unwrap();
        assert_eq!(queue.tasks.len(), 16);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn path_key_is_stable_after_leaf_deletion() {
        let root = temp_root("path-key");
        let path = root.join("raw/sources/a.md");
        fs::write(&path, "content").unwrap();
        let before = path_key(&path);
        fs::remove_file(&path).unwrap();
        let after = path_key(&path);

        assert_eq!(before, after);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn queue_lock_recovers_after_poison() {
        let root = temp_root("poison");
        let lock = queue_lock_for(&root);
        let _ = std::thread::spawn(move || {
            let _guard = lock.lock().unwrap();
            panic!("poison ingest queue lock");
        })
        .join();

        let result = with_queue_lock(&root, || Ok(42));
        assert_eq!(result.unwrap(), 42);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(windows)]
    fn windows_path_key_is_case_insensitive() {
        assert_eq!(
            normalize_path_key(std::path::Path::new(r"C:\Proj\raw\sources\File.md")),
            normalize_path_key(std::path::Path::new(r"c:\proj\RAW\sources\file.md"))
        );
    }
}
