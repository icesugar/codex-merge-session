use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct AppSettings {
    #[serde(default)]
    pub manual_providers: Vec<String>,
    #[serde(default)]
    pub last_selected_provider: Option<String>,
}

pub fn settings_path(codex_root: &Path) -> Result<PathBuf> {
    let parent = codex_root
        .parent()
        .context("codex root has no parent directory")?;
    Ok(parent.join(".codex-merge-session").join("settings.json"))
}

pub fn load_settings(codex_root: &Path) -> Result<AppSettings> {
    let path = settings_path(codex_root)?;
    if !path.exists() {
        return Ok(AppSettings::default());
    }

    let text = fs::read_to_string(&path)
        .with_context(|| format!("failed to read settings {}", path.display()))?;
    let settings: AppSettings = serde_json::from_str(&text)
        .with_context(|| format!("failed to parse settings {}", path.display()))?;
    Ok(settings)
}

pub fn save_settings(codex_root: &Path, settings: &AppSettings) -> Result<()> {
    let path = settings_path(codex_root)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_vec_pretty(settings)?)
        .with_context(|| format!("failed to write settings {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_path_uses_codex_merge_session_directory() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let codex_root = temp_dir.path().join(".codex");
        fs::create_dir_all(&codex_root)?;

        assert_eq!(
            settings_path(&codex_root)?,
            temp_dir
                .path()
                .join(".codex-merge-session")
                .join("settings.json")
        );

        Ok(())
    }
}
