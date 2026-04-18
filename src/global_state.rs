use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalStateRewrite {
    pub updated_content: Option<String>,
    pub normalized_path_count: usize,
    pub recovered_workspace_root_count: usize,
    pub updated_thread_workspace_hint_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadWorkspaceState {
    pub thread_id: String,
    pub workspace_root: String,
    pub updated_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct WorkspaceRecovery {
    recovered_workspace_root_count: usize,
    updated_thread_workspace_hint_count: usize,
}

pub fn prepare_global_state_rewrite(
    codex_root: &Path,
    thread_workspaces: &[ThreadWorkspaceState],
) -> Result<GlobalStateRewrite> {
    let path = global_state_path(codex_root);
    let content = if path.exists() {
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?
    } else {
        "{}".to_string()
    };
    let (updated_content, summary) = normalize_global_state_content(&content, thread_workspaces)?;

    if summary.normalized_path_count == 0
        && summary.recovered_workspace_root_count == 0
        && summary.updated_thread_workspace_hint_count == 0
    {
        return Ok(GlobalStateRewrite::default());
    }

    Ok(GlobalStateRewrite {
        updated_content: Some(updated_content),
        normalized_path_count: summary.normalized_path_count,
        recovered_workspace_root_count: summary.recovered_workspace_root_count,
        updated_thread_workspace_hint_count: summary.updated_thread_workspace_hint_count,
    })
}

pub fn global_state_path(codex_root: &Path) -> PathBuf {
    codex_root.join(".codex-global-state.json")
}

fn normalize_global_state_content(
    content: &str,
    thread_workspaces: &[ThreadWorkspaceState],
) -> Result<(String, GlobalStateRewrite)> {
    let mut value: Value =
        serde_json::from_str(content).context("failed to parse .codex-global-state.json")?;
    let normalized_path_count = normalize_value(&mut value, None);
    let recovery = recover_workspace_state(&mut value, thread_workspaces)?;
    Ok((
        serde_json::to_string(&value)?,
        GlobalStateRewrite {
            updated_content: None,
            normalized_path_count,
            recovered_workspace_root_count: recovery.recovered_workspace_root_count,
            updated_thread_workspace_hint_count: recovery.updated_thread_workspace_hint_count,
        },
    ))
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

fn recover_workspace_state(
    value: &mut Value,
    thread_workspaces: &[ThreadWorkspaceState],
) -> Result<WorkspaceRecovery> {
    let root = value
        .as_object_mut()
        .context(".codex-global-state.json root must be an object")?;

    let normalized_threads = thread_workspaces
        .iter()
        .filter(|thread| !thread.workspace_root.is_empty())
        .map(|thread| ThreadWorkspaceState {
            thread_id: thread.thread_id.clone(),
            workspace_root: normalized_workspace_root(&thread.workspace_root),
            updated_at_ms: thread.updated_at_ms,
        })
        .collect::<Vec<_>>();

    let updated_thread_workspace_hint_count =
        recover_thread_workspace_root_hints(root, &normalized_threads);
    let recovered_workspace_root_count = recover_workspace_root_arrays(root, &normalized_threads);

    Ok(WorkspaceRecovery {
        recovered_workspace_root_count,
        updated_thread_workspace_hint_count,
    })
}

fn recover_thread_workspace_root_hints(
    root: &mut Map<String, Value>,
    thread_workspaces: &[ThreadWorkspaceState],
) -> usize {
    let hints = ensure_object_field(root, "thread-workspace-root-hints");
    let mut updated_hint_count = 0;

    for thread in thread_workspaces {
        let needs_update = match hints.get(&thread.thread_id) {
            Some(Value::String(current)) => current != &thread.workspace_root,
            _ => true,
        };
        if needs_update {
            hints.insert(
                thread.thread_id.clone(),
                Value::String(thread.workspace_root.clone()),
            );
            updated_hint_count += 1;
        }
    }

    updated_hint_count
}

fn recover_workspace_root_arrays(
    root: &mut Map<String, Value>,
    thread_workspaces: &[ThreadWorkspaceState],
) -> usize {
    let ordered_roots = workspace_roots_by_recency(thread_workspaces);
    let mut recovered_roots = HashSet::new();

    {
        let saved_roots = ensure_array_field(root, "electron-saved-workspace-roots");
        let mut saved_root_set = string_values(saved_roots);
        for workspace_root in &ordered_roots {
            if saved_root_set.insert(workspace_root.clone()) {
                saved_roots.push(Value::String(workspace_root.clone()));
                recovered_roots.insert(workspace_root.clone());
            }
        }
    }

    {
        let project_order = ensure_array_field(root, "project-order");
        let mut project_order_set = string_values(project_order);
        for workspace_root in &ordered_roots {
            if project_order_set.insert(workspace_root.clone()) {
                project_order.push(Value::String(workspace_root.clone()));
                recovered_roots.insert(workspace_root.clone());
            }
        }
    }

    recovered_roots.len()
}

fn workspace_roots_by_recency(thread_workspaces: &[ThreadWorkspaceState]) -> Vec<String> {
    let mut latest_updated_at_by_root = BTreeMap::new();

    for thread in thread_workspaces {
        latest_updated_at_by_root
            .entry(thread.workspace_root.clone())
            .and_modify(|existing_updated_at| {
                if thread.updated_at_ms > *existing_updated_at {
                    *existing_updated_at = thread.updated_at_ms;
                }
            })
            .or_insert(thread.updated_at_ms);
    }

    let mut ordered_roots = latest_updated_at_by_root.into_iter().collect::<Vec<_>>();
    ordered_roots.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    ordered_roots
        .into_iter()
        .map(|(workspace_root, _)| workspace_root)
        .collect()
}

fn ensure_array_field<'a>(root: &'a mut Map<String, Value>, key: &str) -> &'a mut Vec<Value> {
    if !matches!(root.get(key), Some(Value::Array(_))) {
        root.insert(key.to_string(), Value::Array(Vec::new()));
    }

    root.get_mut(key)
        .and_then(Value::as_array_mut)
        .expect("array field")
}

fn ensure_object_field<'a>(
    root: &'a mut Map<String, Value>,
    key: &str,
) -> &'a mut Map<String, Value> {
    if !matches!(root.get(key), Some(Value::Object(_))) {
        root.insert(key.to_string(), Value::Object(Map::new()));
    }

    root.get_mut(key)
        .and_then(Value::as_object_mut)
        .expect("object field")
}

fn string_values(array: &[Value]) -> HashSet<String> {
    array
        .iter()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn normalized_workspace_root(path: &str) -> String {
    normalize_windows_extended_path(path).unwrap_or_else(|| path.to_string())
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

        let rewrite = prepare_global_state_rewrite(&codex_root, &[])?;

        assert_eq!(rewrite.normalized_path_count, 7);
        assert_eq!(rewrite.recovered_workspace_root_count, 0);
        assert_eq!(rewrite.updated_thread_workspace_hint_count, 0);
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

    #[test]
    fn prepare_global_state_rewrite_recovers_workspace_roots_and_hints() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let codex_root = temp_dir.path().join(".codex");
        fs::create_dir_all(&codex_root)?;
        fs::write(
            global_state_path(&codex_root),
            serde_json::to_vec(&json!({
                "electron-saved-workspace-roots": [r"D:\dev\codex-workspace\codex-rebuild-session"],
                "project-order": [r"D:\dev\codex-workspace\codex-rebuild-session"],
                "thread-workspace-root-hints": {
                    "other-thread": r"D:\somewhere\else"
                }
            }))?,
        )?;

        let rewrite = prepare_global_state_rewrite(
            &codex_root,
            &[
                ThreadWorkspaceState {
                    thread_id: "thread-yonbip".to_string(),
                    workspace_root: r"\\?\D:\gpt\yonbip".to_string(),
                    updated_at_ms: Some(300),
                },
                ThreadWorkspaceState {
                    thread_id: "thread-dict".to_string(),
                    workspace_root: r"C:\Users\home127\Desktop\ncc2207dict".to_string(),
                    updated_at_ms: Some(200),
                },
            ],
        )?;

        assert_eq!(rewrite.normalized_path_count, 0);
        assert_eq!(rewrite.recovered_workspace_root_count, 2);
        assert_eq!(rewrite.updated_thread_workspace_hint_count, 2);

        let updated_json: Value =
            serde_json::from_str(&rewrite.updated_content.expect("updated content"))?;
        assert_eq!(
            updated_json["electron-saved-workspace-roots"],
            json!([
                r"D:\dev\codex-workspace\codex-rebuild-session",
                r"D:\gpt\yonbip",
                r"C:\Users\home127\Desktop\ncc2207dict"
            ])
        );
        assert_eq!(
            updated_json["project-order"],
            json!([
                r"D:\dev\codex-workspace\codex-rebuild-session",
                r"D:\gpt\yonbip",
                r"C:\Users\home127\Desktop\ncc2207dict"
            ])
        );
        assert_eq!(
            updated_json["thread-workspace-root-hints"]["thread-yonbip"],
            json!(r"D:\gpt\yonbip")
        );
        assert_eq!(
            updated_json["thread-workspace-root-hints"]["thread-dict"],
            json!(r"C:\Users\home127\Desktop\ncc2207dict")
        );
        assert_eq!(
            updated_json["thread-workspace-root-hints"]["other-thread"],
            json!(r"D:\somewhere\else")
        );

        Ok(())
    }
}
