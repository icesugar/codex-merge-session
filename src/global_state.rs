use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalStateRewrite {
    pub updated_content: Option<String>,
    pub normalized_path_count: usize,
}

pub fn prepare_global_state_rewrite(codex_root: &Path) -> Result<GlobalStateRewrite> {
    let path = global_state_path(codex_root);
    if !path.exists() {
        return Ok(GlobalStateRewrite::default());
    }

    let content =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let (updated_content, normalized_path_count) = normalize_global_state_content(&content)?;

    if normalized_path_count == 0 {
        return Ok(GlobalStateRewrite::default());
    }

    Ok(GlobalStateRewrite {
        updated_content: Some(updated_content),
        normalized_path_count,
    })
}

pub fn global_state_path(codex_root: &Path) -> PathBuf {
    codex_root.join(".codex-global-state.json")
}

fn normalize_global_state_content(content: &str) -> Result<(String, usize)> {
    let mut value: Value =
        serde_json::from_str(content).context("failed to parse .codex-global-state.json")?;
    let normalized_path_count = normalize_value(&mut value, None);
    Ok((serde_json::to_string(&value)?, normalized_path_count))
}

fn normalize_value(value: &mut Value, key: Option<&str>) -> usize {
    match value {
        Value::Object(object) => normalize_object(object, key),
        Value::Array(array) => normalize_array(array, key),
        Value::String(text) if matches!(key, Some("cwd")) => {
            if let Some(normalized) = normalize_windows_extended_path(text) {
                *text = normalized;
                1
            } else {
                0
            }
        }
        _ => 0,
    }
}

fn normalize_object(object: &mut Map<String, Value>, key: Option<&str>) -> usize {
    let mut normalized_path_count = 0;

    if matches!(key, Some("sidebar-collapsed-groups" | "perPath")) {
        normalized_path_count += normalize_object_keys(object);
    }

    if matches!(key, Some("thread-workspace-root-hints")) {
        normalized_path_count += normalize_object_string_values(object);
    }

    let keys: Vec<String> = object.keys().cloned().collect();
    for child_key in keys {
        if let Some(child_value) = object.get_mut(&child_key) {
            normalized_path_count += normalize_value(child_value, Some(child_key.as_str()));
        }
    }

    normalized_path_count
}

fn normalize_array(array: &mut Vec<Value>, key: Option<&str>) -> usize {
    if matches!(
        key,
        Some(
            "electron-saved-workspace-roots"
                | "active-workspace-roots"
                | "project-order"
                | "workspaceRoots"
        )
    ) {
        return normalize_string_array(array);
    }

    array
        .iter_mut()
        .map(|value| normalize_value(value, None))
        .sum()
}

fn normalize_object_keys(object: &mut Map<String, Value>) -> usize {
    let entries: Vec<(String, Value)> = std::mem::take(object).into_iter().collect();
    let mut normalized_object = Map::with_capacity(entries.len());
    let mut normalized_path_count = 0;

    for (key, value) in entries {
        let normalized_key = if let Some(normalized) = normalize_windows_extended_path(&key) {
            normalized_path_count += 1;
            normalized
        } else {
            key
        };
        normalized_object.insert(normalized_key, value);
    }

    *object = normalized_object;
    normalized_path_count
}

fn normalize_object_string_values(object: &mut Map<String, Value>) -> usize {
    let mut normalized_path_count = 0;

    for value in object.values_mut() {
        if let Value::String(text) = value {
            if let Some(normalized) = normalize_windows_extended_path(text) {
                *text = normalized;
                normalized_path_count += 1;
            }
        }
    }

    normalized_path_count
}

fn normalize_string_array(array: &mut Vec<Value>) -> usize {
    let mut normalized_values = Vec::with_capacity(array.len());
    let mut seen = HashSet::new();
    let mut normalized_path_count = 0;

    for value in array.drain(..) {
        if let Value::String(text) = value {
            let normalized = if let Some(path) = normalize_windows_extended_path(&text) {
                normalized_path_count += 1;
                path
            } else {
                text
            };

            if seen.insert(normalized.clone()) {
                normalized_values.push(Value::String(normalized));
            }
        } else {
            normalized_values.push(value);
        }
    }

    *array = normalized_values;
    normalized_path_count
}

fn normalize_windows_extended_path(path: &str) -> Option<String> {
    path.strip_prefix(r"\\?\").map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prepare_global_state_rewrite_normalizes_known_workspace_locations() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let codex_root = temp_dir.path().join(".codex");
        fs::create_dir_all(&codex_root)?;
        fs::write(
            global_state_path(&codex_root),
            serde_json::to_vec(&json!({
                "electron-persisted-atom-state": {
                    "sidebar-collapsed-groups": {
                        r"\\?\D:\gpt\yonbip": true
                    },
                    "open-in-target-preferences": {
                        "global": "fileManager",
                        "perPath": {
                            r"\\?\D:\dev\idea-workspace\yonyou-mcp": "fileManager"
                        }
                    }
                },
                "electron-saved-workspace-roots": [
                    r"D:\gpt\yonbip",
                    r"\\?\D:\gpt\yonbip",
                    r"\\?\D:\dev\idea-workspace\yonyou-mcp"
                ],
                "thread-workspace-root-hints": {
                    "thread-1": r"\\?\D:\gpt\yonbip"
                },
                "queued-follow-ups": {
                    "thread-2": [{
                        "cwd": r"\\?\D:\gpt\yonbip",
                        "context": {
                            "workspaceRoots": [r"\\?\D:\dev\idea-workspace\yonyou-mcp"]
                        }
                    }]
                }
            }))?,
        )?;

        let rewrite = prepare_global_state_rewrite(&codex_root)?;

        assert_eq!(rewrite.normalized_path_count, 7);
        let updated_content = rewrite.updated_content.expect("updated content");
        let updated_json: Value = serde_json::from_str(&updated_content)?;
        assert_eq!(
            updated_json["electron-saved-workspace-roots"],
            json!([r"D:\gpt\yonbip", r"D:\dev\idea-workspace\yonyou-mcp"])
        );
        assert_eq!(
            updated_json["thread-workspace-root-hints"]["thread-1"],
            json!(r"D:\gpt\yonbip")
        );

        Ok(())
    }
}
