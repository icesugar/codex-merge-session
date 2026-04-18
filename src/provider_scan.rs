use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigProviderState {
    pub current_provider: String,
    pub declared_providers: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RolloutProviderState {
    pub active_counts: BTreeMap<String, usize>,
    pub archived_counts: BTreeMap<String, usize>,
    pub records: Vec<RolloutRecord>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SqliteProviderState {
    pub active_counts: BTreeMap<String, usize>,
    pub archived_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolloutRecord {
    pub path: PathBuf,
    pub provider: String,
    pub archived: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOption {
    pub id: String,
    pub from_config: bool,
    pub from_rollout: bool,
    pub from_sqlite: bool,
    pub from_manual: bool,
    pub is_current: bool,
    pub rollout_active_count: usize,
    pub rollout_archived_count: usize,
    pub sqlite_active_count: usize,
    pub sqlite_archived_count: usize,
}

pub fn scan_config_providers(codex_root: &Path) -> Result<ConfigProviderState> {
    #[derive(Debug, Deserialize)]
    struct ConfigToml {
        model_provider: Option<String>,
        model_providers: Option<BTreeMap<String, toml::Value>>,
    }

    let path = codex_root.join("config.toml");
    let text =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let parsed: ConfigToml =
        toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))?;

    let current_provider = parsed
        .model_provider
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "openai".to_string());
    let mut declared = parsed
        .model_providers
        .unwrap_or_default()
        .into_keys()
        .collect::<Vec<_>>();
    if !declared.contains(&current_provider) {
        declared.push(current_provider.clone());
    }
    declared.sort();
    declared.dedup();

    Ok(ConfigProviderState {
        current_provider,
        declared_providers: declared,
    })
}

pub fn scan_rollout_providers(codex_root: &Path) -> Result<RolloutProviderState> {
    let mut state = RolloutProviderState::default();
    scan_rollout_dir(
        &codex_root.join("sessions"),
        &mut state.active_counts,
        false,
        &mut state.records,
    )?;
    scan_rollout_dir(
        &codex_root.join("archived_sessions"),
        &mut state.archived_counts,
        true,
        &mut state.records,
    )?;
    Ok(state)
}

pub fn scan_sqlite_providers(codex_root: &Path) -> Result<SqliteProviderState> {
    let path = codex_root.join("state_5.sqlite");
    if !path.exists() {
        return Ok(SqliteProviderState::default());
    }

    let connection = Connection::open(path)?;
    let mut statement = connection.prepare(
        "select model_provider, archived, count(*)
         from threads
         group by model_provider, archived",
    )?;

    let rows = statement.query_map([], |row| {
        let provider = row
            .get::<_, Option<String>>(0)?
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "(missing)".to_string());
        let archived = row.get::<_, i64>(1)? != 0;
        let count = row.get::<_, i64>(2)? as usize;
        Ok((provider, archived, count))
    })?;

    let mut state = SqliteProviderState::default();
    for row in rows {
        let (provider, archived, count) = row?;
        let bucket = if archived {
            &mut state.archived_counts
        } else {
            &mut state.active_counts
        };
        bucket.insert(provider, count);
    }

    Ok(state)
}

pub fn build_provider_options(
    config: &ConfigProviderState,
    rollout: &RolloutProviderState,
    sqlite: &SqliteProviderState,
    manual_providers: &[String],
) -> Vec<ProviderOption> {
    let mut all_ids = BTreeSet::new();
    all_ids.extend(config.declared_providers.iter().cloned());
    all_ids.extend(rollout.active_counts.keys().cloned());
    all_ids.extend(rollout.archived_counts.keys().cloned());
    all_ids.extend(sqlite.active_counts.keys().cloned());
    all_ids.extend(sqlite.archived_counts.keys().cloned());
    all_ids.extend(
        manual_providers
            .iter()
            .filter(|value| !value.trim().is_empty())
            .cloned(),
    );
    all_ids.insert(config.current_provider.clone());

    all_ids
        .into_iter()
        .map(|id| ProviderOption {
            from_config: config.declared_providers.contains(&id) || config.current_provider == id,
            from_rollout: rollout.active_counts.contains_key(&id)
                || rollout.archived_counts.contains_key(&id),
            from_sqlite: sqlite.active_counts.contains_key(&id)
                || sqlite.archived_counts.contains_key(&id),
            from_manual: manual_providers.iter().any(|value| value == &id),
            is_current: config.current_provider == id,
            rollout_active_count: rollout.active_counts.get(&id).copied().unwrap_or_default(),
            rollout_archived_count: rollout.archived_counts.get(&id).copied().unwrap_or_default(),
            sqlite_active_count: sqlite.active_counts.get(&id).copied().unwrap_or_default(),
            sqlite_archived_count: sqlite.archived_counts.get(&id).copied().unwrap_or_default(),
            id,
        })
        .collect()
}

fn scan_rollout_dir(
    root: &Path,
    counts: &mut BTreeMap<String, usize>,
    archived: bool,
    records: &mut Vec<RolloutRecord>,
) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }

    for path in rollout_paths(root)? {
        if let Some(provider) = read_rollout_provider(&path)? {
            *counts.entry(provider.clone()).or_insert(0) += 1;
            records.push(RolloutRecord {
                path,
                provider,
                archived,
            });
        }
    }
    Ok(())
}

fn rollout_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)
            .with_context(|| format!("failed to read rollout directory {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if file_type.is_file() && name.starts_with("rollout-") && name.ends_with(".jsonl") {
                paths.push(path);
            }
        }
    }

    Ok(paths)
}

fn read_rollout_provider(path: &Path) -> Result<Option<String>> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read rollout file {}", path.display()))?;
    let Some(first_line) = text.lines().next() else {
        return Ok(None);
    };
    let json: Value = serde_json::from_str(first_line)
        .with_context(|| format!("invalid session_meta JSON in {}", path.display()))?;
    Ok(json
        .get("payload")
        .and_then(Value::as_object)
        .and_then(|payload| payload.get("model_provider"))
        .and_then(Value::as_str)
        .map(ToString::to_string))
}
