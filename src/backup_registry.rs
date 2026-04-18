use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::session_files::{restore_files, BackupFileRecord};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupEntry {
    pub path: PathBuf,
    pub kind: String,
    pub created_at_unix_seconds: u64,
    pub is_legacy_location: bool,
}

#[derive(Debug, Deserialize)]
struct BackupManifest {
    #[serde(default)]
    created_at_unix_seconds: u64,
    #[serde(default)]
    target_provider: Option<String>,
    #[serde(default)]
    backed_up_files: Vec<BackupFileRecord>,
}

pub fn list_backups(codex_root: &Path) -> Result<Vec<BackupEntry>> {
    let mut entries = Vec::new();

    let root = primary_backup_root(codex_root)?;
    if !root.exists() {
        return Ok(entries);
    }

    for child in fs::read_dir(&root)
        .with_context(|| format!("failed to read backup root {}", root.display()))?
    {
        let child = child?;
        let path = child.path();
        if !child.file_type()?.is_dir() {
            continue;
        }

        let manifest = read_manifest(&path)?;
        entries.push(BackupEntry {
            path,
            kind: if manifest.target_provider.is_some() {
                "merge".to_string()
            } else {
                "repair".to_string()
            },
            created_at_unix_seconds: manifest.created_at_unix_seconds,
            is_legacy_location: false,
        });
    }

    entries.sort_by(|left, right| {
        right
            .created_at_unix_seconds
            .cmp(&left.created_at_unix_seconds)
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(entries)
}

pub fn restore_backup(backup_dir: &Path) -> Result<()> {
    let manifest = read_manifest(backup_dir)?;
    restore_files(&manifest.backed_up_files)
}

pub fn delete_backup(backup_dir: &Path) -> Result<()> {
    fs::remove_dir_all(backup_dir)
        .with_context(|| format!("failed to delete backup {}", backup_dir.display()))
}

pub fn primary_backup_root(codex_root: &Path) -> Result<PathBuf> {
    let parent = codex_root
        .parent()
        .context("codex root has no parent directory")?;
    Ok(parent.join(".codex-merge-session").join("backups"))
}

fn read_manifest(backup_dir: &Path) -> Result<BackupManifest> {
    let manifest_path = backup_dir.join("manifest.json");
    let text = fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let manifest: BackupManifest = serde_json::from_str(&text)
        .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn primary_backup_root_uses_codex_merge_session_directory() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let codex_root = temp_dir.path().join(".codex");
        fs::create_dir_all(&codex_root)?;

        assert_eq!(
            primary_backup_root(&codex_root)?,
            temp_dir.path().join(".codex-merge-session").join("backups")
        );

        Ok(())
    }

    #[test]
    fn list_backups_ignores_other_sibling_directories() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let codex_root = temp_dir.path().join(".codex");
        fs::create_dir_all(&codex_root)?;

        let merge_backup_dir = primary_backup_root(&codex_root)?.join("177");
        fs::create_dir_all(&merge_backup_dir)?;
        fs::write(
            merge_backup_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&json!({
                "created_at_unix_seconds": 177_u64,
                "target_provider": "custom",
                "backed_up_files": [],
            }))?,
        )?;

        let unrelated_backup_dir = temp_dir
            .path()
            .join(".ignored-backups")
            .join("backups")
            .join("188");
        fs::create_dir_all(&unrelated_backup_dir)?;
        fs::write(
            unrelated_backup_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&json!({
                "created_at_unix_seconds": 188_u64,
                "target_provider": "legacy",
                "backed_up_files": [],
            }))?,
        )?;

        let backups = list_backups(&codex_root)?;

        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].path, merge_backup_dir);
        assert!(!backups[0].is_legacy_location);

        Ok(())
    }
}
