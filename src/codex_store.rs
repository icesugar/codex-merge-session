use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::backup_registry::{
    delete_backup as delete_backup_dir, list_backups as list_backup_entries,
    primary_backup_root, restore_backup as restore_backup_dir, BackupEntry,
};
use crate::provider_scan::{
    build_provider_options, scan_config_providers, scan_rollout_providers, scan_sqlite_providers,
    ProviderOption,
};
use crate::settings::{load_settings, save_settings};
use crate::global_state::{
    global_state_path, prepare_global_state_rewrite, GlobalStateRewrite, ThreadWorkspaceState,
};
use crate::process_guard::LiveProcessInspector;
use crate::session_files::{
    backup_files, ensure_rollout_is_valid, prepare_rollout_path_rewrite, prepare_rollout_rewrite,
    read_session_meta_cwd, restore_files, BackupManifest, RolloutRewrite,
};
use crate::session_index::{build_session_index_content, session_index_path, SessionIndexThread};

pub trait ProcessInspector: Send + Sync {
    fn running_codex_pids(&self) -> Result<Vec<u32>>;
    fn terminate_codex_pids(&self, pids: &[u32]) -> Result<Vec<u32>>;
}

pub trait FaultInjector: Send + Sync {
    fn before_rollout_write(&self, _path: &Path, _index: usize) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSummary {
    pub name: String,
    pub active_count: usize,
    pub archived_count: usize,
    pub latest_updated_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ScanResult {
    pub current_provider: String,
    pub provider_summaries: Vec<ProviderSummary>,
    pub provider_options: Vec<ProviderOption>,
}

#[derive(Debug, Clone)]
pub struct MergePreview {
    pub target_provider: String,
    pub source_providers: Vec<String>,
    pub affected_thread_ids: Vec<String>,
    pub affected_rollout_paths: Vec<PathBuf>,
    pub will_update_config: bool,
    pub backup_dir: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct MergeReport {
    pub migrated_thread_count: usize,
    pub migrated_file_count: usize,
    pub backup_dir: PathBuf,
    pub rolled_back: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RepairReport {
    pub normalized_cwd_count: usize,
    pub normalized_rollout_file_count: usize,
    pub normalized_workspace_path_count: usize,
    pub recovered_workspace_root_count: usize,
    pub updated_thread_workspace_hint_count: usize,
    pub rebuilt_session_index: bool,
    pub backup_dir: PathBuf,
    pub rolled_back: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
struct ThreadRecord {
    id: String,
    rollout_path: PathBuf,
    model_provider: String,
    archived: bool,
    cwd: String,
    title: String,
    updated_at_ms: Option<i64>,
    updated_at_rfc3339: Option<String>,
}

#[derive(Debug, Clone)]
struct CwdUpdate {
    thread_id: String,
    normalized_cwd: String,
}

#[derive(Debug, Serialize)]
struct RepairManifest {
    created_at_unix_seconds: u64,
    normalized_cwd_thread_ids: Vec<String>,
    normalized_rollout_paths: Vec<String>,
    normalized_workspace_path_count: usize,
    recovered_workspace_root_count: usize,
    updated_thread_workspace_hint_count: usize,
    rebuilt_session_index: bool,
    backed_up_files: Vec<crate::session_files::BackupFileRecord>,
}

pub struct CodexStore {
    codex_root: PathBuf,
    process_inspector: Arc<dyn ProcessInspector>,
    fault_injector: Arc<dyn FaultInjector>,
}

impl CodexStore {
    pub fn new(codex_root: PathBuf) -> Self {
        Self::new_with_hooks(
            codex_root,
            Arc::new(LiveProcessInspector),
            Arc::new(NoopFaultInjector),
        )
    }

    pub fn new_with_hooks(
        codex_root: PathBuf,
        process_inspector: Arc<dyn ProcessInspector>,
        fault_injector: Arc<dyn FaultInjector>,
    ) -> Self {
        Self {
            codex_root,
            process_inspector,
            fault_injector,
        }
    }

    pub fn scan(&self) -> Result<ScanResult> {
        let current_provider = self.read_current_provider()?;
        let mut grouped: BTreeMap<String, ProviderSummary> = BTreeMap::new();
        let settings = load_settings(&self.codex_root)?;
        let config_state = scan_config_providers(&self.codex_root)?;
        let rollout_state = scan_rollout_providers(&self.codex_root)?;
        let sqlite_state = scan_sqlite_providers(&self.codex_root)?;

        for thread in self.load_threads()? {
            let entry = grouped
                .entry(thread.model_provider.clone())
                .or_insert(ProviderSummary {
                    name: thread.model_provider.clone(),
                    active_count: 0,
                    archived_count: 0,
                    latest_updated_at: None,
                });

            if thread.archived {
                entry.archived_count += 1;
            } else {
                entry.active_count += 1;
            }
            entry.latest_updated_at = match (entry.latest_updated_at, thread.updated_at_ms) {
                (Some(left), Some(right)) => Some(left.max(right)),
                (None, right) => right,
                (left, None) => left,
            };
        }

        Ok(ScanResult {
            current_provider,
            provider_summaries: grouped.into_values().collect(),
            provider_options: build_provider_options(
                &config_state,
                &rollout_state,
                &sqlite_state,
                &settings.manual_providers,
            ),
        })
    }

    pub fn build_preview(&self, target_provider: &str) -> Result<MergePreview> {
        let current_provider = self.read_current_provider()?;
        let mut threads: Vec<ThreadRecord> = self
            .load_threads()?
            .into_iter()
            .filter(|thread| thread.model_provider != target_provider)
            .collect();
        threads.sort_by(|left, right| left.id.cmp(&right.id));

        for thread in &threads {
            if !thread.rollout_path.exists() {
                anyhow::bail!("missing rollout file: {}", thread.rollout_path.display());
            }
            ensure_rollout_is_valid(&thread.rollout_path)?;
        }

        let rollout_state = scan_rollout_providers(&self.codex_root)?;
        let mut affected_rollout_paths = rollout_state
            .records
            .iter()
            .filter(|record| record.provider != target_provider)
            .map(|record| record.path.clone())
            .collect::<Vec<_>>();
        affected_rollout_paths.sort();
        affected_rollout_paths.dedup();

        let mut source_providers: Vec<String> = threads
            .iter()
            .map(|thread| thread.model_provider.clone())
            .collect();
        source_providers.extend(
            rollout_state
                .records
                .iter()
                .filter(|record| record.provider != target_provider)
                .map(|record| record.provider.clone()),
        );
        source_providers.sort();
        source_providers.dedup();

        Ok(MergePreview {
            target_provider: target_provider.to_string(),
            source_providers,
            affected_thread_ids: threads.iter().map(|thread| thread.id.clone()).collect(),
            affected_rollout_paths,
            will_update_config: current_provider != target_provider,
            backup_dir: self.backup_root()?.join(self.timestamp_tag()),
        })
    }

    pub fn execute_merge(&self, target_provider: &str) -> Result<MergeReport> {
        let running_pids = self.process_inspector.running_codex_pids()?;
        if !running_pids.is_empty() {
            let pid_text = running_pids
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!("检测到 codex.exe 正在运行，请先关闭。PID: {pid_text}");
        }

        let current_provider = self.read_current_provider()?;
        let preview = self.build_preview(target_provider)?;
        let cwd_updates = self.repaired_thread_cwds()?;
        if preview.affected_thread_ids.is_empty() {
            if cwd_updates.is_empty() && current_provider == target_provider {
                return Ok(MergeReport {
                    backup_dir: preview.backup_dir,
                    ..MergeReport::default()
                });
            }
        }
        let session_index_content = build_session_index_content(&self.session_index_threads()?)?;

        fs::create_dir_all(&preview.backup_dir)?;
        let backup_paths = self.backup_target_paths(&preview.affected_rollout_paths);
        let backup_records = backup_files(&self.codex_root, &preview.backup_dir, &backup_paths)?;
        self.write_manifest(&preview, &backup_records)?;

        let rewrites = preview
            .affected_rollout_paths
            .iter()
            .map(|path| prepare_rollout_rewrite(path, target_provider))
            .collect::<Result<Vec<_>>>()?;

        let merge_result = self.commit_merge(
            target_provider,
            &preview,
            &rewrites,
            &session_index_content,
            &cwd_updates,
        );
        match merge_result {
            Ok(()) => Ok(MergeReport {
                migrated_thread_count: preview.affected_thread_ids.len(),
                migrated_file_count: rewrites.len(),
                backup_dir: preview.backup_dir,
                rolled_back: false,
                errors: Vec::new(),
            }),
            Err(error) => {
                restore_files(&backup_records)?;
                Ok(MergeReport {
                    migrated_thread_count: 0,
                    migrated_file_count: 0,
                    backup_dir: preview.backup_dir,
                    rolled_back: true,
                    errors: vec![format!("{error:#}")],
                })
            }
        }
    }

    pub fn execute_repair(&self) -> Result<RepairReport> {
        let running_pids = self.process_inspector.running_codex_pids()?;
        if !running_pids.is_empty() {
            let pid_text = running_pids
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!("检测到 codex.exe 正在运行，请先关闭。PID: {pid_text}");
        }

        let cwd_updates = self.repaired_thread_cwds()?;
        let rollout_rewrites = self.repaired_rollouts()?;
        let session_index_content = build_session_index_content(&self.session_index_threads()?)?;
        let needs_index_rebuild = self.current_session_index_content()? != session_index_content;
        let global_state_thread_workspaces = self.global_state_thread_workspaces()?;
        let global_state_rewrite = self.global_state_rewrite(&global_state_thread_workspaces)?;
        let backup_dir = self.backup_root()?.join(self.timestamp_tag());

        if cwd_updates.is_empty()
            && rollout_rewrites.is_empty()
            && !needs_index_rebuild
            && global_state_rewrite.normalized_path_count == 0
            && global_state_rewrite.recovered_workspace_root_count == 0
            && global_state_rewrite.updated_thread_workspace_hint_count == 0
        {
            return Ok(RepairReport {
                backup_dir,
                ..RepairReport::default()
            });
        }

        fs::create_dir_all(&backup_dir)?;
        let backup_records = backup_files(
            &self.codex_root,
            &backup_dir,
            &self.repair_backup_target_paths(&rollout_rewrites),
        )?;
        self.write_repair_manifest(
            &backup_dir,
            &cwd_updates,
            &rollout_rewrites,
            global_state_rewrite.normalized_path_count,
            global_state_rewrite.recovered_workspace_root_count,
            global_state_rewrite.updated_thread_workspace_hint_count,
            needs_index_rebuild,
            &backup_records,
        )?;

        let repair_result = self.commit_repair(
            &session_index_content,
            &cwd_updates,
            &rollout_rewrites,
            &global_state_rewrite,
        );
        match repair_result {
            Ok(()) => Ok(RepairReport {
                normalized_cwd_count: cwd_updates.len(),
                normalized_rollout_file_count: rollout_rewrites.len(),
                normalized_workspace_path_count: global_state_rewrite.normalized_path_count,
                recovered_workspace_root_count: global_state_rewrite.recovered_workspace_root_count,
                updated_thread_workspace_hint_count: global_state_rewrite
                    .updated_thread_workspace_hint_count,
                rebuilt_session_index: needs_index_rebuild,
                backup_dir,
                rolled_back: false,
                errors: Vec::new(),
            }),
            Err(error) => {
                restore_files(&backup_records)?;
                Ok(RepairReport {
                    normalized_cwd_count: 0,
                    normalized_rollout_file_count: 0,
                    normalized_workspace_path_count: 0,
                    recovered_workspace_root_count: 0,
                    updated_thread_workspace_hint_count: 0,
                    rebuilt_session_index: false,
                    backup_dir,
                    rolled_back: true,
                    errors: vec![format!("{error:#}")],
                })
            }
        }
    }

    pub fn codex_root(&self) -> &Path {
        &self.codex_root
    }

    pub fn running_codex_pids(&self) -> Result<Vec<u32>> {
        self.process_inspector.running_codex_pids()
    }

    pub fn terminate_codex_pids(&self, pids: &[u32]) -> Result<Vec<u32>> {
        self.process_inspector.terminate_codex_pids(pids)
    }

    pub fn list_backups(&self) -> Result<Vec<BackupEntry>> {
        list_backup_entries(&self.codex_root)
    }

    pub fn restore_backup(&self, backup_dir: &Path) -> Result<()> {
        let running_pids = self.process_inspector.running_codex_pids()?;
        if !running_pids.is_empty() {
            let pid_text = running_pids
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!("检测到 codex.exe 正在运行，请先关闭。PID: {pid_text}");
        }
        restore_backup_dir(backup_dir)
    }

    pub fn delete_backup(&self, backup_dir: &Path) -> Result<()> {
        delete_backup_dir(backup_dir)
    }

    pub fn add_manual_provider(&self, provider: &str) -> Result<()> {
        let trimmed = provider.trim();
        if trimmed.is_empty() {
            return Ok(());
        }

        let mut settings = load_settings(&self.codex_root)?;
        if !settings.manual_providers.iter().any(|value| value == trimmed) {
            settings.manual_providers.push(trimmed.to_string());
            settings.manual_providers.sort();
            settings.manual_providers.dedup();
        }
        settings.last_selected_provider = Some(trimmed.to_string());
        save_settings(&self.codex_root, &settings)
    }

    pub fn remove_manual_provider(&self, provider: &str) -> Result<()> {
        let mut settings = load_settings(&self.codex_root)?;
        settings.manual_providers.retain(|value| value != provider);
        if settings
            .last_selected_provider
            .as_deref()
            .is_some_and(|value| value == provider)
        {
            settings.last_selected_provider = None;
        }
        save_settings(&self.codex_root, &settings)
    }

    fn commit_merge(
        &self,
        target_provider: &str,
        preview: &MergePreview,
        rewrites: &[crate::session_files::RolloutRewrite],
        session_index_content: &str,
        cwd_updates: &[CwdUpdate],
    ) -> Result<()> {
        let mut connection = Connection::open(self.state_db_path())?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

        for thread_id in &preview.affected_thread_ids {
            transaction.execute(
                "update threads set model_provider = ?1 where id = ?2",
                params![target_provider, thread_id],
            )?;
        }
        for update in cwd_updates {
            transaction.execute(
                "update threads set cwd = ?1 where id = ?2",
                params![update.normalized_cwd, update.thread_id],
            )?;
        }

        for (index, rewrite) in rewrites.iter().enumerate() {
            self.fault_injector
                .before_rollout_write(&rewrite.path, index)?;
            fs::write(&rewrite.path, &rewrite.updated_content).with_context(|| {
                format!("failed to update rollout file {}", rewrite.path.display())
            })?;
        }

        self.write_root_provider_to_config(target_provider)?;
        fs::write(self.session_index_path(), session_index_content)
            .context("failed to rebuild session_index.jsonl")?;
        transaction.commit()?;
        Ok(())
    }

    fn commit_repair(
        &self,
        session_index_content: &str,
        cwd_updates: &[CwdUpdate],
        rollout_rewrites: &[RolloutRewrite],
        global_state_rewrite: &GlobalStateRewrite,
    ) -> Result<()> {
        let mut connection = Connection::open(self.state_db_path())?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

        for update in cwd_updates {
            transaction.execute(
                "update threads set cwd = ?1 where id = ?2",
                params![update.normalized_cwd, update.thread_id],
            )?;
        }

        for (index, rewrite) in rollout_rewrites.iter().enumerate() {
            self.fault_injector
                .before_rollout_write(&rewrite.path, index)?;
            fs::write(&rewrite.path, &rewrite.updated_content).with_context(|| {
                format!("failed to update rollout file {}", rewrite.path.display())
            })?;
        }

        fs::write(self.session_index_path(), session_index_content)
            .context("failed to rebuild session_index.jsonl")?;
        if let Some(updated_content) = &global_state_rewrite.updated_content {
            fs::write(self.global_state_path(), updated_content)
                .context("failed to rewrite .codex-global-state.json")?;
        }
        transaction.commit()?;
        Ok(())
    }

    fn read_current_provider(&self) -> Result<String> {
        #[derive(Deserialize)]
        struct Config {
            model_provider: Option<String>,
        }

        let config_text = fs::read_to_string(self.config_path())
            .context("failed to read config.toml")?;
        let config: Config = toml::from_str(&config_text).context("failed to parse config.toml")?;
        Ok(config
            .model_provider
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "openai".to_string()))
    }

    fn write_root_provider_to_config(&self, provider: &str) -> Result<()> {
        let path = self.config_path();
        let config_text = fs::read_to_string(&path).context("failed to read config.toml")?;
        let updated_content = set_root_provider_in_config(&config_text, provider);
        fs::write(path, updated_content).context("failed to update config.toml")
    }

    fn load_threads(&self) -> Result<Vec<ThreadRecord>> {
        let connection = Connection::open(self.state_db_path())?;
        let mut statement = connection.prepare(
            "select id, rollout_path, model_provider, archived, cwd, title, updated_at_ms,
                    strftime('%Y-%m-%dT%H:%M:%fZ', updated_at_ms / 1000.0, 'unixepoch')
             from threads",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(ThreadRecord {
                id: row.get(0)?,
                rollout_path: PathBuf::from(row.get::<_, String>(1)?),
                model_provider: row.get(2)?,
                archived: row.get::<_, i64>(3)? != 0,
                cwd: row.get(4)?,
                title: row.get(5)?,
                updated_at_ms: row.get(6).ok(),
                updated_at_rfc3339: row.get(7).ok(),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn session_index_threads(&self) -> Result<Vec<SessionIndexThread>> {
        Ok(self
            .load_threads()?
            .into_iter()
            .map(|thread| SessionIndexThread {
                id: thread.id,
                title: thread.title,
                updated_at_ms: thread.updated_at_ms,
                updated_at_rfc3339: thread.updated_at_rfc3339,
            })
            .collect())
    }

    fn repaired_thread_cwds(&self) -> Result<Vec<CwdUpdate>> {
        Ok(self
            .load_threads()?
            .into_iter()
            .filter_map(|thread| {
                self.desired_thread_cwd(&thread)
                    .filter(|desired_cwd| desired_cwd != &thread.cwd)
                    .map(|normalized_cwd| CwdUpdate {
                        thread_id: thread.id,
                        normalized_cwd,
                    })
            })
            .collect())
    }

    fn repaired_rollouts(&self) -> Result<Vec<RolloutRewrite>> {
        let mut rewrites = Vec::new();
        for thread in self.load_threads()? {
            if let Some(rewrite) = prepare_rollout_path_rewrite(&thread.rollout_path)? {
                rewrites.push(rewrite);
            }
        }
        Ok(rewrites)
    }

    fn desired_thread_cwd(&self, thread: &ThreadRecord) -> Option<String> {
        read_session_meta_cwd(&thread.rollout_path)
            .ok()
            .flatten()
            .map(|cwd| normalize_windows_extended_path(&cwd).unwrap_or(cwd))
            .or_else(|| normalize_windows_extended_path(&thread.cwd))
            .or_else(|| (!thread.cwd.is_empty()).then(|| thread.cwd.clone()))
    }

    fn write_manifest(
        &self,
        preview: &MergePreview,
        backup_records: &[crate::session_files::BackupFileRecord],
    ) -> Result<()> {
        let created_at_unix_seconds = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let manifest = BackupManifest {
            created_at_unix_seconds,
            target_provider: preview.target_provider.clone(),
            source_providers: preview.source_providers.clone(),
            affected_thread_ids: preview.affected_thread_ids.clone(),
            backed_up_files: backup_records.to_vec(),
        };
        fs::write(
            preview.backup_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        Ok(())
    }

    fn write_repair_manifest(
        &self,
        backup_dir: &Path,
        cwd_updates: &[CwdUpdate],
        rollout_rewrites: &[RolloutRewrite],
        normalized_workspace_path_count: usize,
        recovered_workspace_root_count: usize,
        updated_thread_workspace_hint_count: usize,
        rebuilt_session_index: bool,
        backup_records: &[crate::session_files::BackupFileRecord],
    ) -> Result<()> {
        let created_at_unix_seconds = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let manifest = RepairManifest {
            created_at_unix_seconds,
            normalized_cwd_thread_ids: cwd_updates
                .iter()
                .map(|update| update.thread_id.clone())
                .collect(),
            normalized_rollout_paths: rollout_rewrites
                .iter()
                .map(|rewrite| rewrite.path.display().to_string())
                .collect(),
            normalized_workspace_path_count,
            recovered_workspace_root_count,
            updated_thread_workspace_hint_count,
            rebuilt_session_index,
            backed_up_files: backup_records.to_vec(),
        };
        fs::write(
            backup_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        Ok(())
    }

    fn backup_root(&self) -> Result<PathBuf> {
        primary_backup_root(&self.codex_root)
    }

    fn backup_target_paths(&self, rollout_paths: &[PathBuf]) -> Vec<PathBuf> {
        let mut paths = vec![self.state_db_path(), self.config_path()];
        for suffix in ["state_5.sqlite-wal", "state_5.sqlite-shm"] {
            let path = self.codex_root.join(suffix);
            if path.exists() {
                paths.push(path);
            }
        }
        let session_index = self.session_index_path();
        if session_index.exists() {
            paths.push(session_index);
        }
        paths.extend(rollout_paths.iter().cloned());
        paths
    }

    fn repair_backup_target_paths(&self, rollout_rewrites: &[RolloutRewrite]) -> Vec<PathBuf> {
        let rollout_paths = rollout_rewrites
            .iter()
            .map(|rewrite| rewrite.path.clone())
            .collect::<Vec<_>>();
        let mut paths = self.backup_target_paths(&rollout_paths);
        let global_state = self.global_state_path();
        if global_state.exists() {
            paths.push(global_state);
        }
        paths
    }

    fn state_db_path(&self) -> PathBuf {
        self.codex_root.join("state_5.sqlite")
    }

    fn config_path(&self) -> PathBuf {
        self.codex_root.join("config.toml")
    }

    fn current_session_index_content(&self) -> Result<String> {
        let path = self.session_index_path();
        if !path.exists() {
            return Ok(String::new());
        }
        fs::read_to_string(path).context("failed to read session_index.jsonl")
    }

    fn session_index_path(&self) -> PathBuf {
        session_index_path(&self.codex_root)
    }

    fn global_state_path(&self) -> PathBuf {
        global_state_path(&self.codex_root)
    }

    fn global_state_rewrite(
        &self,
        thread_workspaces: &[ThreadWorkspaceState],
    ) -> Result<GlobalStateRewrite> {
        prepare_global_state_rewrite(&self.codex_root, thread_workspaces)
    }

    fn global_state_thread_workspaces(&self) -> Result<Vec<ThreadWorkspaceState>> {
        Ok(self
            .load_threads()?
            .into_iter()
            .filter_map(|thread| {
                self.desired_thread_cwd(&thread)
                    .map(|workspace_root| ThreadWorkspaceState {
                        thread_id: thread.id,
                        workspace_root,
                        updated_at_ms: thread.updated_at_ms,
                    })
            })
            .collect())
    }

    fn timestamp_tag(&self) -> String {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs().to_string())
            .unwrap_or_else(|_| "0".to_string())
    }
}

struct NoopFaultInjector;

impl FaultInjector for NoopFaultInjector {}

fn normalize_windows_extended_path(path: &str) -> Option<String> {
    path.strip_prefix(r"\\?\").map(ToString::to_string)
}

fn set_root_provider_in_config(config_text: &str, provider: &str) -> String {
    let mut lines: Vec<String> = config_text.lines().map(ToString::to_string).collect();
    let mut insert_index = lines.len();

    for (index, line) in lines.iter_mut().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            insert_index = index + 1;
            continue;
        }
        if trimmed.starts_with('[') {
            insert_index = index;
            break;
        }
        if trimmed.starts_with("model_provider") {
            *line = format!("model_provider = \"{provider}\"");
            let mut output = lines.join("\n");
            if config_text.ends_with('\n') {
                output.push('\n');
            }
            return output;
        }
        insert_index = index + 1;
    }

    lines.insert(insert_index, format!("model_provider = \"{provider}\""));
    let mut output = lines.join("\n");
    if config_text.ends_with('\n') {
        output.push('\n');
    }
    output
}
