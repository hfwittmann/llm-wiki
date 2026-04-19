//! Panic-to-error boundary for Tauri commands.
//!
//! Third-party parsers (pdf-extract / lopdf, docx-rs, calamine, …) are
//! known to panic on malformed input instead of returning Err. Under
//! `panic = "abort"` that kills the whole app; even with `panic =
//! "unwind"`, letting a panic propagate through the `extern "C"` Tauri
//! command boundary is UB. These helpers catch panics at the command
//! boundary and convert them into a Tauri Err the frontend can display.

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Run a synchronous command body, converting any panic into an Err.
pub fn run_guarded<T, F>(label: &str, f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String>,
{
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(payload) => Err(report(label, payload)),
    }
}

/// Run an async command body, converting any panic into an Err.
pub async fn run_guarded_async<T, Fut>(label: &str, fut: Fut) -> Result<T, String>
where
    Fut: std::future::Future<Output = Result<T, String>>,
{
    use futures::FutureExt;
    match AssertUnwindSafe(fut).catch_unwind().await {
        Ok(r) => r,
        Err(payload) => Err(report(label, payload)),
    }
}

fn report(label: &str, payload: Box<dyn Any + Send>) -> String {
    let msg = if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else {
        "(non-string panic payload)".to_string()
    };
    eprintln!("[panic_guard] command '{label}' panicked: {msg}");
    format!("Internal error in {label}: {msg}")
}
