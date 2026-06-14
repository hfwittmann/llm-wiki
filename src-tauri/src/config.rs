//! Server configuration loaded from environment variables at startup.
//!
//! Env vars are the only configuration source for v1; a startup TOML can
//! be added later if shell-env limits become a problem. Defaults are tuned
//! for local-dev (relative paths, port 8080, legacy listener enabled).

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub projects_root: PathBuf,
    pub data_root: PathBuf,
    pub legacy_19828_enabled: bool,
    pub session_cookie_name: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid LLM_WIKI_PORT (must be 1-65535): {0}")]
    InvalidPort(String),
    #[error("invalid LLM_WIKI_LEGACY_19828_ENABLED (must be true|false): {0}")]
    InvalidBool(String),
}

impl ServerConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let port = match std::env::var("LLM_WIKI_PORT") {
            Ok(s) => s
                .parse::<u16>()
                .map_err(|_| ConfigError::InvalidPort(s))?,
            Err(_) => 8080,
        };

        let projects_root = std::env::var("LLM_WIKI_PROJECTS_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./projects"));

        let data_root = std::env::var("LLM_WIKI_DATA_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./data"));

        let legacy_19828_enabled = match std::env::var("LLM_WIKI_LEGACY_19828_ENABLED") {
            Ok(s) => parse_bool(&s)?,
            Err(_) => true,
        };

        let session_cookie_name = std::env::var("LLM_WIKI_SESSION_COOKIE_NAME")
            .unwrap_or_else(|_| "llm_wiki_session".to_string());

        Ok(ServerConfig {
            port,
            projects_root,
            data_root,
            legacy_19828_enabled,
            session_cookie_name,
        })
    }
}

fn parse_bool(s: &str) -> Result<bool, ConfigError> {
    match s.to_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(ConfigError::InvalidBool(s.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locked global mutex — env vars are process-wide, so tests that mutate
    /// them must serialize.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_clean_env<R>(f: impl FnOnce() -> R) -> R {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        for key in [
            "LLM_WIKI_PORT",
            "LLM_WIKI_PROJECTS_ROOT",
            "LLM_WIKI_DATA_ROOT",
            "LLM_WIKI_LEGACY_19828_ENABLED",
            "LLM_WIKI_SESSION_COOKIE_NAME",
        ] {
            std::env::remove_var(key);
        }
        f()
    }

    #[test]
    fn defaults_when_no_env() {
        let cfg = with_clean_env(|| ServerConfig::from_env().unwrap());
        assert_eq!(cfg.port, 8080);
        assert_eq!(cfg.projects_root, PathBuf::from("./projects"));
        assert_eq!(cfg.data_root, PathBuf::from("./data"));
        assert!(cfg.legacy_19828_enabled);
        assert_eq!(cfg.session_cookie_name, "llm_wiki_session");
    }

    #[test]
    fn port_from_env() {
        let cfg = with_clean_env(|| {
            std::env::set_var("LLM_WIKI_PORT", "9000");
            ServerConfig::from_env().unwrap()
        });
        assert_eq!(cfg.port, 9000);
    }

    #[test]
    fn invalid_port_errors() {
        let result = with_clean_env(|| {
            std::env::set_var("LLM_WIKI_PORT", "not-a-number");
            ServerConfig::from_env()
        });
        assert!(matches!(result, Err(ConfigError::InvalidPort(_))));
    }

    #[test]
    fn legacy_listener_can_be_disabled() {
        let cfg = with_clean_env(|| {
            std::env::set_var("LLM_WIKI_LEGACY_19828_ENABLED", "false");
            ServerConfig::from_env().unwrap()
        });
        assert!(!cfg.legacy_19828_enabled);
    }

    #[test]
    fn invalid_bool_errors() {
        let result = with_clean_env(|| {
            std::env::set_var("LLM_WIKI_LEGACY_19828_ENABLED", "maybe");
            ServerConfig::from_env()
        });
        assert!(matches!(result, Err(ConfigError::InvalidBool(_))));
    }

    #[test]
    fn bool_accepts_aliases() {
        for (v, expected) in [
            ("true", true), ("1", true), ("yes", true), ("TRUE", true),
            ("false", false), ("0", false), ("no", false), ("FALSE", false),
        ] {
            let cfg = with_clean_env(|| {
                std::env::set_var("LLM_WIKI_LEGACY_19828_ENABLED", v);
                ServerConfig::from_env().unwrap()
            });
            assert_eq!(cfg.legacy_19828_enabled, expected, "input: {v}");
        }
    }
}
