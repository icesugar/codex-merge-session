use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Clone)]
pub struct SessionIndexThread {
    pub id: String,
    pub title: String,
    pub updated_at_ms: Option<i64>,
    pub updated_at_rfc3339: Option<String>,
}

#[derive(Debug, Serialize)]
struct SessionIndexEntry<'a> {
    id: &'a str,
    thread_name: &'a str,
    updated_at: &'a str,
}

pub fn build_session_index_content(threads: &[SessionIndexThread]) -> Result<String> {
    let mut sorted = threads.to_vec();
    sorted.sort_by(|left, right| {
        right
            .updated_at_ms
            .cmp(&left.updated_at_ms)
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut content = String::new();
    for thread in sorted {
        let updated_at = thread
            .updated_at_rfc3339
            .as_deref()
            .unwrap_or("1970-01-01T00:00:00.000Z");
        let entry = SessionIndexEntry {
            id: &thread.id,
            thread_name: &thread.title,
            updated_at,
        };
        content.push_str(&serde_json::to_string(&entry)?);
        content.push('\n');
    }

    Ok(content)
}

pub fn session_index_path(codex_root: &Path) -> PathBuf {
    codex_root.join("session_index.jsonl")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_session_index_orders_by_updated_at_desc() {
        let content = build_session_index_content(&[
            SessionIndexThread {
                id: "a".to_string(),
                title: "A".to_string(),
                updated_at_ms: Some(100),
                updated_at_rfc3339: Some("2026-04-01T00:00:00.100Z".to_string()),
            },
            SessionIndexThread {
                id: "b".to_string(),
                title: "B".to_string(),
                updated_at_ms: Some(200),
                updated_at_rfc3339: Some("2026-04-01T00:00:00.200Z".to_string()),
            },
        ])
        .expect("session index content");

        let lines: Vec<&str> = content.lines().collect();
        assert!(lines[0].contains("\"id\":\"b\""));
        assert!(lines[1].contains("\"id\":\"a\""));
    }
}
