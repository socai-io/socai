//! Bounded local diagnostics. Never record CDP parameters, JS, URLs or page text.
use serde_json::{json, Value};
use std::{io::Write, sync::Mutex};
static LOCK: Mutex<()> = Mutex::new(());

pub fn record(event: &str, fields: Value) {
    let Ok(_guard) = LOCK.lock() else {
        return;
    };
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let dir = home.join(".socai").join("logs");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("cdp-diagnostics.jsonl");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 5 * 1024 * 1024) {
        let _ = std::fs::rename(&path, dir.join("cdp-diagnostics.previous.jsonl"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(
            f,
            "{}",
            json!({"at":chrono::Utc::now().to_rfc3339(),"event":event,"fields":fields})
        );
    }
}
