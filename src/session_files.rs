use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const WINDOWS_EXTENDED_PATH_PREFIX: &str = "\\\\?\\";

#[derive(Debug, Clone)]
pub struct RolloutRewrite {
    pub path: PathBuf,
    pub updated_content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupManifest {
    pub created_at_unix_seconds: u64,
    pub target_provider: String,
    pub source_providers: Vec<String>,
    pub affected_thread_ids: Vec<String>,
    pub backed_up_files: Vec<BackupFileRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupFileRecord {
    pub original_path: String,
    pub backup_path: String,
}

pub fn prepare_rollout_rewrite(path: &Path, target_provider: &str) -> Result<RolloutRewrite> {
    rewrite_rollout(path, Some(target_provider)).map(|(rewrite, _)| rewrite)
}

pub fn prepare_rollout_path_rewrite(path: &Path) -> Result<Option<RolloutRewrite>> {
    let (rewrite, changed) = rewrite_rollout(path, None)?;
    Ok(changed.then_some(rewrite))
}

pub fn read_session_meta_cwd(path: &Path) -> Result<Option<String>> {
    let (meta, _rest) = read_rollout_parts(path)?;
    Ok(meta
        .get("payload")
        .and_then(Value::as_object)
        .and_then(|payload| payload.get("cwd"))
        .and_then(Value::as_str)
        .map(ToString::to_string))
}

pub fn ensure_rollout_is_valid(path: &Path) -> Result<()> {
    prepare_rollout_rewrite(path, "__validation__").map(|_| ())
}

pub fn backup_files(
    root: &Path,
    backup_dir: &Path,
    paths: &[PathBuf],
) -> Result<Vec<BackupFileRecord>> {
    let snapshot_dir = backup_dir.join("snapshot");
    let mut records = Vec::new();

    for path in paths {
        if !path.exists() {
            continue;
        }

        let relative = path
            .strip_prefix(root)
            .with_context(|| format!("path {} is outside codex root", path.display()))?;
        let backup_path = snapshot_dir.join(relative);
        if let Some(parent) = backup_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(path, &backup_path).with_context(|| {
            format!(
                "failed to back up {} to {}",
                path.display(),
                backup_path.display()
            )
        })?;

        records.push(BackupFileRecord {
            original_path: path.display().to_string(),
            backup_path: backup_path.display().to_string(),
        });
    }

    Ok(records)
}

pub fn restore_files(records: &[BackupFileRecord]) -> Result<()> {
    for record in records {
        let backup_path = Path::new(&record.backup_path);
        let original_path = Path::new(&record.original_path);
        if let Some(parent) = original_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(backup_path, original_path).with_context(|| {
            format!(
                "failed to restore {} from {}",
                original_path.display(),
                backup_path.display()
            )
        })?;
    }
    Ok(())
}

fn read_rollout_parts(path: &Path) -> Result<(Value, String)> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read rollout file {}", path.display()))?;
    let (first_line, rest) = text
        .split_once('\n')
        .map_or((text.as_str(), ""), |(head, tail)| (head, tail));

    let meta: Value = serde_json::from_str(first_line)
        .with_context(|| format!("invalid session_meta JSON in {}", path.display()))?;

    Ok((meta, rest.to_string()))
}

fn session_meta_payload_mut<'a>(
    meta: &'a mut Value,
    path: &Path,
) -> Result<&'a mut serde_json::Map<String, Value>> {
    meta.get_mut("payload")
        .and_then(Value::as_object_mut)
        .with_context(|| format!("missing session_meta payload in {}", path.display()))
}

fn rewrite_rollout(path: &Path, target_provider: Option<&str>) -> Result<(RolloutRewrite, bool)> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read rollout file {}", path.display()))?;
    let had_trailing_newline = text.ends_with('\n');
    let mut changed = false;
    let mut updated_lines = Vec::new();

    for (index, line) in text.lines().enumerate() {
        let mut json: Value = serde_json::from_str(line)
            .with_context(|| format!("invalid rollout JSON in {}", path.display()))?;
        if index == 0 {
            if let Some(provider) = target_provider {
                let payload = session_meta_payload_mut(&mut json, path)?;
                let needs_provider_update = payload
                    .get("model_provider")
                    .and_then(Value::as_str)
                    .map(|current| current != provider)
                    .unwrap_or(true);
                if needs_provider_update {
                    payload.insert(
                        "model_provider".to_string(),
                        Value::String(provider.to_string()),
                    );
                    changed = true;
                }
            }
        }
        changed |= normalize_rollout_value(&mut json);
        updated_lines.push(serde_json::to_string(&json)?);
    }

    let mut updated_content = updated_lines.join("\n");
    if had_trailing_newline && !updated_content.is_empty() {
        updated_content.push('\n');
    }

    Ok((
        RolloutRewrite {
            path: path.to_path_buf(),
            updated_content,
        },
        changed,
    ))
}

fn normalize_rollout_value(value: &mut Value) -> bool {
    match value {
        Value::Object(object) => object.values_mut().any(normalize_rollout_value),
        Value::Array(array) => array.iter_mut().any(normalize_rollout_value),
        Value::String(text) => {
            if text.contains(WINDOWS_EXTENDED_PATH_PREFIX) {
                *text = text.replace(WINDOWS_EXTENDED_PATH_PREFIX, "");
                true
            } else {
                false
            }
        }
        _ => false,
    }
}
