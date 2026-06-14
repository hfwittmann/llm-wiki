use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum UserDataError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid user_id: {0}")]
    InvalidUserId(String),
    #[error("invalid project_id: {0}")]
    InvalidProjectId(String),
    #[error("invalid conversation_id: {0}")]
    InvalidConversationId(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMeta {
    pub id: String,
    pub modified_unix: u64,
}

#[derive(Debug, Clone)]
pub struct UserData {
    data_root: PathBuf,
}

impl UserData {
    pub fn new(data_root: PathBuf) -> Self {
        UserData { data_root }
    }

    fn user_dir(&self, user_id: &str) -> Result<PathBuf, UserDataError> {
        safe_segment("user_id", user_id)?;
        Ok(self.data_root.join("users").join(user_id))
    }

    pub fn load_config(&self, user_id: &str) -> Result<serde_json::Value, UserDataError> {
        let path = self.user_dir(user_id)?.join("config.json");
        if !path.exists() {
            return Ok(serde_json::Value::Object(Default::default()));
        }
        let raw = fs::read(&path)?;
        Ok(serde_json::from_slice(&raw)?)
    }

    pub fn save_config(&self, user_id: &str, value: &serde_json::Value) -> Result<(), UserDataError> {
        let path = self.user_dir(user_id)?.join("config.json");
        atomic_write_json(&path, value)
    }

    pub fn recently_opened(&self, user_id: &str) -> Vec<String> {
        let path = match self.user_dir(user_id) {
            Ok(p) => p.join("recently_opened.json"),
            Err(_) => return vec![],
        };
        if !path.exists() {
            return vec![];
        }
        let raw = match fs::read(&path) {
            Ok(b) => b,
            Err(_) => return vec![],
        };
        serde_json::from_slice::<Vec<String>>(&raw).unwrap_or_default()
    }

    pub fn add_recently_opened(&self, user_id: &str, project_id: &str) -> Result<(), UserDataError> {
        let path = self.user_dir(user_id)?.join("recently_opened.json");
        let mut current = self.recently_opened(user_id);
        current.retain(|p| p != project_id);
        current.insert(0, project_id.to_string());
        current.truncate(20);
        atomic_write_json(&path, &serde_json::json!(current))
    }

    fn chat_dir(&self, user_id: &str, project_id: &str) -> Result<PathBuf, UserDataError> {
        safe_segment("user_id", user_id)?;
        safe_segment("project_id", project_id)?;
        Ok(self.data_root.join("users").join(user_id).join("chat").join(project_id))
    }

    pub fn list_conversations(&self, user_id: &str, project_id: &str)
        -> Result<Vec<ConversationMeta>, UserDataError>
    {
        let dir = self.chat_dir(user_id, project_id)?;
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let id = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let modified_unix = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            out.push(ConversationMeta { id, modified_unix });
        }
        out.sort_by(|a, b| b.modified_unix.cmp(&a.modified_unix));
        Ok(out)
    }

    pub fn load_conversation(&self, user_id: &str, project_id: &str, conv_id: &str)
        -> Result<serde_json::Value, UserDataError>
    {
        safe_segment("conversation_id", conv_id)?;
        let path = self.chat_dir(user_id, project_id)?.join(format!("{conv_id}.json"));
        let raw = fs::read(&path)?;
        Ok(serde_json::from_slice(&raw)?)
    }

    pub fn save_conversation(&self, user_id: &str, project_id: &str, conv_id: &str, value: &serde_json::Value)
        -> Result<(), UserDataError>
    {
        safe_segment("conversation_id", conv_id)?;
        let path = self.chat_dir(user_id, project_id)?.join(format!("{conv_id}.json"));
        atomic_write_json(&path, value)
    }
}

// ---- private helpers ----

fn safe_segment(label: &'static str, segment: &str) -> Result<(), UserDataError> {
    // Allow [a-zA-Z0-9._-] only — covers usernames, conv UUIDs, project hashes.
    if segment.is_empty()
        || !segment.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(match label {
            "user_id" => UserDataError::InvalidUserId(segment.into()),
            "project_id" => UserDataError::InvalidProjectId(segment.into()),
            "conversation_id" => UserDataError::InvalidConversationId(segment.into()),
            _ => UserDataError::InvalidUserId(segment.into()),
        });
    }
    Ok(())
}

fn atomic_write_json(path: &Path, value: &serde_json::Value) -> Result<(), UserDataError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(serde_json::to_vec_pretty(value)?.as_slice())?;
        f.sync_all()?;
    }
    fs::rename(tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn ud() -> (TempDir, UserData) {
        let dir = TempDir::new().unwrap();
        let ud = UserData::new(dir.path().to_path_buf());
        (dir, ud)
    }

    #[test]
    fn load_config_returns_empty_object_when_missing() {
        let (_dir, ud) = ud();
        let cfg = ud.load_config("alice").unwrap();
        assert!(cfg.is_object());
        assert_eq!(cfg.as_object().unwrap().len(), 0);
    }

    #[test]
    fn save_then_load_config_roundtrip() {
        let (_dir, ud) = ud();
        let value = json!({"llm": {"endpoint": "https://api.example.com", "model": "x"}});
        ud.save_config("alice", &value).unwrap();
        assert_eq!(ud.load_config("alice").unwrap(), value);
    }

    #[test]
    fn save_config_is_atomic_no_leftover_tmp() {
        let (dir, ud) = ud();
        ud.save_config("alice", &json!({"x": 1})).unwrap();
        let entries: Vec<_> = fs::read_dir(dir.path().join("users/alice")).unwrap().collect();
        assert!(!entries.iter().any(|e| {
            e.as_ref().unwrap().path().extension().map(|x| x == "tmp").unwrap_or(false)
        }));
    }

    #[test]
    fn invalid_user_id_is_rejected() {
        let (_dir, ud) = ud();
        let result = ud.save_config("../etc", &json!({}));
        assert!(matches!(result, Err(UserDataError::InvalidUserId(_))));
        let result = ud.load_config("alice/bob");
        assert!(matches!(result, Err(UserDataError::InvalidUserId(_))));
    }

    #[test]
    fn config_isolated_per_user() {
        let (_dir, ud) = ud();
        ud.save_config("alice", &json!({"who": "alice"})).unwrap();
        ud.save_config("bob", &json!({"who": "bob"})).unwrap();
        assert_eq!(ud.load_config("alice").unwrap(), json!({"who": "alice"}));
        assert_eq!(ud.load_config("bob").unwrap(), json!({"who": "bob"}));
    }

    #[test]
    fn recently_opened_starts_empty() {
        let (_dir, ud) = ud();
        assert_eq!(ud.recently_opened("alice"), Vec::<String>::new());
    }

    #[test]
    fn add_recently_opened_dedupes_and_moves_to_front() {
        let (_dir, ud) = ud();
        ud.add_recently_opened("alice", "proj-a").unwrap();
        ud.add_recently_opened("alice", "proj-b").unwrap();
        ud.add_recently_opened("alice", "proj-a").unwrap();
        assert_eq!(
            ud.recently_opened("alice"),
            vec!["proj-a".to_string(), "proj-b".to_string()]
        );
    }

    #[test]
    fn save_then_list_conversation() {
        let (_dir, ud) = ud();
        ud.save_conversation("alice", "proj1", "abc", &json!({"messages": []})).unwrap();
        let conv = ud.load_conversation("alice", "proj1", "abc").unwrap();
        assert_eq!(conv, json!({"messages": []}));
        let list = ud.list_conversations("alice", "proj1").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "abc");
    }

    #[test]
    fn list_conversations_for_unused_project_is_empty() {
        let (_dir, ud) = ud();
        assert!(ud.list_conversations("alice", "untouched").unwrap().is_empty());
    }
}
