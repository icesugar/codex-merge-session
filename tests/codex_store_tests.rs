use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use tempfile::TempDir;

use codex_merge_session::codex_store::{
    CodexStore, FaultInjector, MergeReport, ProcessInspector, RepairReport,
};

#[derive(Default)]
struct StaticProcessInspector {
    running_pids: Vec<u32>,
}

impl ProcessInspector for StaticProcessInspector {
    fn running_codex_pids(&self) -> Result<Vec<u32>> {
        Ok(self.running_pids.clone())
    }

    fn terminate_codex_pids(&self, _pids: &[u32]) -> Result<Vec<u32>> {
        Ok(Vec::new())
    }
}

#[derive(Default)]
struct FailOnWriteInjector {
    fail_on_index: Option<usize>,
}

impl FaultInjector for FailOnWriteInjector {
    fn before_rollout_write(&self, _path: &Path, index: usize) -> Result<()> {
        if self.fail_on_index == Some(index) {
            anyhow::bail!("injected rollout write failure at index {index}");
        }
        Ok(())
    }
}

#[test]
fn scan_loads_current_provider_and_summaries() -> Result<()> {
    let fixture = Fixture::new("OpenAI")?;
    fixture.add_thread("t-openai", "OpenAI", false, 100, "OpenAI")?;
    fixture.add_thread("t-openai-archived", "openai", true, 250, "openai")?;
    fixture.add_thread("t-custom", "custom", false, 180, "custom")?;

    let store = fixture.store();
    let scan = store.scan()?;

    assert_eq!(scan.current_provider, "OpenAI");
    assert_eq!(scan.provider_summaries.len(), 3);

    let openai = scan
        .provider_summaries
        .iter()
        .find(|summary| summary.name == "OpenAI")
        .expect("OpenAI summary");
    assert_eq!(openai.active_count, 1);
    assert_eq!(openai.archived_count, 0);
    assert_eq!(openai.latest_updated_at, Some(100));

    let lower_openai = scan
        .provider_summaries
        .iter()
        .find(|summary| summary.name == "openai")
        .expect("openai summary");
    assert_eq!(lower_openai.active_count, 0);
    assert_eq!(lower_openai.archived_count, 1);
    assert_eq!(lower_openai.latest_updated_at, Some(250));

    Ok(())
}

#[test]
fn preview_rejects_missing_rollout_file() -> Result<()> {
    let fixture = Fixture::new("OpenAI")?;
    fixture.add_thread("t-openai", "OpenAI", false, 100, "OpenAI")?;
    let missing = fixture.rollout_path("missing-thread", false);
    fixture.insert_thread_only("missing-thread", "custom", false, 300, &missing)?;

    let store = fixture.store();
    let error = store.build_preview("OpenAI").unwrap_err();

    assert!(error.to_string().contains("missing rollout file"));
    Ok(())
}

#[test]
fn preview_marks_config_update_when_only_root_provider_differs() -> Result<()> {
    let fixture = Fixture::new("openai")?;

    let store = fixture.store();
    let preview = store.build_preview("custom")?;

    assert!(preview.will_update_config);
    assert!(preview.affected_thread_ids.is_empty());
    assert!(preview.affected_rollout_paths.is_empty());

    Ok(())
}

#[test]
fn merge_updates_threads_session_meta_and_manifest() -> Result<()> {
    let fixture = Fixture::new("OpenAI")?;
    fixture.add_thread("t-openai", "OpenAI", false, 100, "OpenAI")?;
    fixture.add_thread("t-lower", "openai", true, 250, "openai")?;
    fixture.add_thread("t-custom", "custom", false, 180, "custom")?;
    fixture.write_session_index(&[("t-openai", "旧标题", "2026-04-18T00:00:00.000Z")])?;

    let store = fixture.store();
    let preview = store.build_preview("OpenAI")?;
    assert_eq!(
        preview.source_providers,
        vec!["custom".to_string(), "openai".to_string()]
    );
    assert_eq!(preview.affected_thread_ids.len(), 2);

    let report = store.execute_merge("OpenAI")?;

    assert!(!report.rolled_back);
    assert!(report.errors.is_empty());
    assert_eq!(report.migrated_thread_count, 2);
    assert_eq!(report.migrated_file_count, 2);
    assert!(report.backup_dir.exists());
    assert!(report.backup_dir.join("manifest.json").exists());
    let providers = fixture.thread_providers()?;
    assert_eq!(
        providers,
        vec![
            ("t-custom".to_string(), "OpenAI".to_string()),
            ("t-lower".to_string(), "OpenAI".to_string()),
            ("t-openai".to_string(), "OpenAI".to_string()),
        ]
    );

    for thread_id in ["t-openai", "t-lower", "t-custom"] {
        let first_line = fixture.read_session_meta_provider(thread_id)?;
        assert_eq!(first_line, "OpenAI");
    }
    assert_eq!(
        fixture.read_session_index_entries()?,
        vec![
            (
                "t-lower".to_string(),
                "标题-t-lower".to_string(),
                "1970-01-01T00:00:00.250Z".to_string(),
            ),
            (
                "t-custom".to_string(),
                "标题-t-custom".to_string(),
                "1970-01-01T00:00:00.180Z".to_string(),
            ),
            (
                "t-openai".to_string(),
                "标题-t-openai".to_string(),
                "1970-01-01T00:00:00.100Z".to_string(),
            ),
        ]
    );
    assert!(report
        .backup_dir
        .join("snapshot")
        .join("session_index.jsonl")
        .exists());

    let refreshed = store.scan()?;
    assert_eq!(refreshed.provider_summaries.len(), 1);
    assert_eq!(refreshed.provider_summaries[0].name, "OpenAI");
    assert_eq!(refreshed.provider_summaries[0].active_count, 2);
    assert_eq!(refreshed.provider_summaries[0].archived_count, 1);

    Ok(())
}

#[test]
fn merge_replaces_root_provider_with_selected_target() -> Result<()> {
    let fixture = Fixture::new("OpenAI")?;
    fixture.add_thread("t-openai", "OpenAI", false, 100, "OpenAI")?;
    fixture.add_thread("t-custom", "custom", false, 180, "custom")?;

    let store = fixture.store();
    let report = store.execute_merge("custom")?;

    assert!(!report.rolled_back);
    assert!(fixture.read_config_text()?.contains("model_provider = \"custom\""));

    Ok(())
}

#[test]
fn merge_updates_rollout_only_files_even_without_sqlite_thread() -> Result<()> {
    let fixture = Fixture::new("openai")?;
    fixture.write_rollout_without_sqlite("rollout-only", "openai", false)?;

    let store = fixture.store();
    let report = store.execute_merge("custom")?;

    assert!(!report.rolled_back);
    assert_eq!(report.migrated_thread_count, 0);
    assert_eq!(report.migrated_file_count, 1);
    assert_eq!(fixture.read_session_meta_provider("rollout-only")?, "custom");
    assert!(fixture.read_config_text()?.contains("model_provider = \"custom\""));

    Ok(())
}

#[test]
fn scan_includes_config_rollout_sqlite_and_manual_provider_options() -> Result<()> {
    let fixture = Fixture::new("openai")?;
    fixture.write_config_text(
        r#"model_provider = "openai"
sandbox_mode = "danger-full-access"

[model_providers.declared-only]
base_url = "https://example.com"
"#,
    )?;
    fixture.write_manual_settings(&["manual-only"])?;
    fixture.add_thread("t-openai", "openai", false, 100, "openai")?;
    fixture.write_rollout_without_sqlite("rollout-only", "rollout-only", false)?;
    fixture.insert_thread_only(
        "sqlite-only",
        "sqlite-only",
        false,
        180,
        &fixture
            .codex_root()
            .join("sessions")
            .join("2026")
            .join("04")
            .join("18")
            .join("rollout-missing.sqlite-only.jsonl"),
    )?;

    let store = fixture.store();
    let scan = store.scan()?;

    assert_eq!(scan.current_provider, "openai");
    assert!(scan.provider_options.iter().any(|option| {
        option.id == "openai"
            && option.from_config
            && option.from_rollout
            && option.from_sqlite
            && option.is_current
            && option.rollout_active_count == 1
            && option.sqlite_active_count == 1
    }));
    assert!(scan.provider_options.iter().any(|option| {
        option.id == "declared-only"
            && option.from_config
            && !option.from_rollout
            && !option.from_sqlite
            && !option.from_manual
    }));
    assert!(scan.provider_options.iter().any(|option| {
        option.id == "rollout-only"
            && !option.from_config
            && option.from_rollout
            && !option.from_sqlite
            && option.rollout_active_count == 1
    }));
    assert!(scan.provider_options.iter().any(|option| {
        option.id == "sqlite-only"
            && !option.from_config
            && !option.from_rollout
            && option.from_sqlite
            && option.sqlite_active_count == 1
    }));
    assert!(scan.provider_options.iter().any(|option| {
        option.id == "manual-only"
            && !option.from_config
            && !option.from_rollout
            && !option.from_sqlite
            && option.from_manual
    }));

    Ok(())
}

#[test]
fn merge_inserts_root_provider_when_config_missing_root_model_provider() -> Result<()> {
    let fixture = Fixture::new("OpenAI")?;
    fixture.write_config_text(
        r#"
sandbox_mode = "danger-full-access"

[model_providers.openai]
name = "OpenAI"
"#,
    )?;
    fixture.add_thread("t-custom", "custom", false, 180, "custom")?;

    let store = fixture.store();
    let report = store.execute_merge("custom")?;

    assert!(!report.rolled_back);
    let config_text = fixture.read_config_text()?;
    assert!(config_text.contains("model_provider = \"custom\""));
    assert!(config_text.contains("[model_providers.openai]"));

    Ok(())
}

#[test]
fn merge_normalizes_windows_extended_cwds() -> Result<()> {
    let fixture = Fixture::new("OpenAI")?;
    fixture.add_thread("t-openai", "OpenAI", false, 100, "OpenAI")?;
    fixture.add_thread("t-custom", "custom", false, 180, "custom")?;
    fixture.set_thread_cwd("t-openai", r"\\?\D:\dev\idea-workspace\yonyou-mcp")?;
    fixture.set_thread_cwd("t-custom", r"\\?\D:\gpt\yonbip")?;

    let store = fixture.store();
    let report = store.execute_merge("OpenAI")?;

    assert!(!report.rolled_back);
    assert_eq!(
        fixture.thread_cwds()?,
        vec![
            ("t-custom".to_string(), r"D:\gpt\yonbip".to_string()),
            (
                "t-openai".to_string(),
                r"D:\dev\idea-workspace\yonyou-mcp".to_string(),
            ),
        ]
    );

    Ok(())
}

#[test]
fn repair_rebuilds_session_index_and_normalizes_cwds_without_merge_targets() -> Result<()> {
    let fixture = Fixture::new("OpenAI")?;
    fixture.add_thread("t-openai", "OpenAI", false, 100, "OpenAI")?;
    fixture.set_thread_cwd("t-openai", r"\\?\D:\dev\idea-workspace\yonyou-mcp")?;
    fixture.write_session_index(&[])?;

    let store = fixture.store();
    let report: RepairReport = store.execute_repair()?;

    assert!(!report.rolled_back);
    assert_eq!(report.normalized_cwd_count, 1);
    assert_eq!(report.normalized_rollout_file_count, 0);
    assert!(report.rebuilt_session_index);
    assert_eq!(
        fixture.thread_cwds()?,
        vec![(
            "t-openai".to_string(),
            r"D:\dev\idea-workspace\yonyou-mcp".to_string(),
        )]
    );
    assert_eq!(
        fixture.read_session_index_entries()?,
        vec![(
            "t-openai".to_string(),
            "标题-t-openai".to_string(),
            "1970-01-01T00:00:00.100Z".to_string(),
        )]
    );

    Ok(())
}

#[test]
fn repair_syncs_thread_cwd_from_rollout_session_meta() -> Result<()> {
    let fixture = Fixture::new("OpenAI")?;
    fixture.add_thread("t-openai", "OpenAI", false, 100, "OpenAI")?;
    fixture.set_thread_cwd("t-openai", r"D:\wrong\yonbip")?;
    fixture.set_rollout_session_meta_cwd("t-openai", false, r"D:\gpt\yonbip")?;

    let store = fixture.store();
    let report: RepairReport = store.execute_repair()?;

    assert!(!report.rolled_back);
    assert_eq!(report.normalized_cwd_count, 1);
    assert_eq!(report.normalized_rollout_file_count, 0);
    assert_eq!(
        fixture.thread_cwds()?,
        vec![("t-openai".to_string(), r"D:\gpt\yonbip".to_string())]
    );

    Ok(())
}

#[test]
fn repair_normalizes_rollout_embedded_extended_paths() -> Result<()> {
    let fixture = Fixture::new("OpenAI")?;
    fixture.add_thread("t-openai", "OpenAI", false, 100, "OpenAI")?;
    fixture.write_session_index(&[("t-openai", "标题-t-openai", "1970-01-01T00:00:00.100Z")])?;
    fixture.append_rollout_json_line(
        "t-openai",
        false,
        &json!({
            "type": "response_item",
            "payload": {
                "arguments": "{\"workdir\":\"\\\\?\\D:\\gpt\\yonbip\"}",
                "output": "cwd: \\\\?\\D:\\gpt\\yonbip"
            }
        }),
    )?;

    let store = fixture.store();
    let report: RepairReport = store.execute_repair()?;

    assert!(!report.rolled_back);
    assert_eq!(report.normalized_cwd_count, 0);
    assert_eq!(report.normalized_rollout_file_count, 1);
    let rollout_text = fixture.read_rollout_text("t-openai", false)?;
    assert!(!rollout_text.contains(r"\\?\D:\gpt\yonbip"));
    assert!(rollout_text.contains(r#"D:\\gpt\\yonbip"#));

    Ok(())
}

#[test]
fn repair_normalizes_global_state_workspace_paths_without_thread_updates() -> Result<()> {
    let fixture = Fixture::new("OpenAI")?;
    fixture.add_thread("t-openai", "OpenAI", false, 100, "OpenAI")?;
    fixture.set_thread_cwd("t-openai", r"D:\gpt\yonbip")?;
    fixture.write_session_index(&[("t-openai", "标题-t-openai", "1970-01-01T00:00:00.100Z")])?;
    fixture.write_global_state(&json!({
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
        "active-workspace-roots": [
            r"\\?\D:\gpt\yonbip"
        ],
        "project-order": [
            r"D:\gpt\yonbip",
            r"\\?\D:\dev\idea-workspace\yonyou-mcp"
        ],
        "thread-workspace-root-hints": {
            "t-openai": r"\\?\D:\gpt\yonbip"
        },
        "queued-follow-ups": {
            "thread-2": [{
                "cwd": r"\\?\D:\gpt\yonbip",
                "context": {
                    "workspaceRoots": [r"\\?\D:\dev\idea-workspace\yonyou-mcp"]
                }
            }]
        }
    }))?;

    let store = fixture.store();
    let report: RepairReport = store.execute_repair()?;

    assert!(!report.rolled_back);
    assert_eq!(report.normalized_cwd_count, 0);
    assert_eq!(report.normalized_rollout_file_count, 0);
    assert_eq!(report.normalized_workspace_path_count, 9);
    assert_eq!(report.recovered_workspace_root_count, 0);
    assert_eq!(report.updated_thread_workspace_hint_count, 0);
    assert!(!report.rebuilt_session_index);

    let global_state = fixture.read_global_state_json()?;
    assert_eq!(
        global_state["electron-saved-workspace-roots"],
        json!([r"D:\gpt\yonbip", r"D:\dev\idea-workspace\yonyou-mcp"])
    );
    assert_eq!(
        global_state["active-workspace-roots"],
        json!([r"D:\gpt\yonbip"])
    );
    assert_eq!(
        global_state["project-order"],
        json!([r"D:\gpt\yonbip", r"D:\dev\idea-workspace\yonyou-mcp"])
    );
    assert_eq!(
        global_state["thread-workspace-root-hints"]["t-openai"],
        json!(r"D:\gpt\yonbip")
    );
    assert_eq!(
        global_state["queued-follow-ups"]["thread-2"][0]["cwd"],
        json!(r"D:\gpt\yonbip")
    );
    assert_eq!(
        global_state["queued-follow-ups"]["thread-2"][0]["context"]["workspaceRoots"],
        json!([r"D:\dev\idea-workspace\yonyou-mcp"])
    );
    assert_eq!(
        global_state["electron-persisted-atom-state"]["sidebar-collapsed-groups"],
        json!({
            r"D:\gpt\yonbip": true
        })
    );
    assert_eq!(
        global_state["electron-persisted-atom-state"]["open-in-target-preferences"]["perPath"],
        json!({
            r"D:\dev\idea-workspace\yonyou-mcp": "fileManager"
        })
    );

    Ok(())
}

#[test]
fn repair_recovers_missing_workspace_roots_and_thread_hints_from_threads() -> Result<()> {
    let fixture = Fixture::new("OpenAI")?;
    fixture.add_thread("t-yonbip", "OpenAI", false, 300, "OpenAI")?;
    fixture.add_thread("t-dict", "OpenAI", false, 200, "OpenAI")?;
    fixture.set_thread_cwd("t-yonbip", r"\\?\D:\gpt\yonbip")?;
    fixture.set_thread_cwd("t-dict", r"\\?\C:\Users\home127\Desktop\ncc2207dict")?;
    fixture.write_session_index(&[
        ("t-yonbip", "标题-t-yonbip", "1970-01-01T00:00:00.300Z"),
        ("t-dict", "标题-t-dict", "1970-01-01T00:00:00.200Z"),
    ])?;
    fixture.write_global_state(&json!({
        "active-workspace-roots": [r"D:\dev\codex-workspace\codex-rebuild-session"],
        "electron-saved-workspace-roots": [r"D:\dev\codex-workspace\codex-rebuild-session"],
        "project-order": [r"D:\dev\codex-workspace\codex-rebuild-session"],
        "thread-workspace-root-hints": {
            "other-thread": r"D:\somewhere\else"
        },
        "electron-persisted-atom-state": {
            "sidebar-collapsed-groups": {
                r"D:\dev\codex-workspace\codex-rebuild-session": true
            }
        }
    }))?;

    let store = fixture.store();
    let report: RepairReport = store.execute_repair()?;

    assert!(!report.rolled_back);
    assert_eq!(report.normalized_cwd_count, 2);
    assert_eq!(report.normalized_rollout_file_count, 0);
    assert_eq!(report.normalized_workspace_path_count, 0);
    assert_eq!(report.recovered_workspace_root_count, 2);
    assert_eq!(report.updated_thread_workspace_hint_count, 2);
    assert!(!report.rebuilt_session_index);

    let global_state = fixture.read_global_state_json()?;
    assert_eq!(
        global_state["electron-saved-workspace-roots"],
        json!([
            r"D:\dev\codex-workspace\codex-rebuild-session",
            r"D:\gpt\yonbip",
            r"C:\Users\home127\Desktop\ncc2207dict"
        ])
    );
    assert_eq!(
        global_state["project-order"],
        json!([
            r"D:\dev\codex-workspace\codex-rebuild-session",
            r"D:\gpt\yonbip",
            r"C:\Users\home127\Desktop\ncc2207dict"
        ])
    );
    assert_eq!(
        global_state["active-workspace-roots"],
        json!([r"D:\dev\codex-workspace\codex-rebuild-session"])
    );
    assert_eq!(
        global_state["thread-workspace-root-hints"]["t-yonbip"],
        json!(r"D:\gpt\yonbip")
    );
    assert_eq!(
        global_state["thread-workspace-root-hints"]["t-dict"],
        json!(r"C:\Users\home127\Desktop\ncc2207dict")
    );
    assert_eq!(
        global_state["thread-workspace-root-hints"]["other-thread"],
        json!(r"D:\somewhere\else")
    );

    let manifest = fixture.read_repair_manifest_json(&report.backup_dir)?;
    assert_eq!(manifest["normalized_workspace_path_count"], json!(0));
    assert_eq!(manifest["recovered_workspace_root_count"], json!(2));
    assert_eq!(manifest["updated_thread_workspace_hint_count"], json!(2));

    Ok(())
}

#[test]
fn repair_workspace_recovery_is_idempotent() -> Result<()> {
    let fixture = Fixture::new("OpenAI")?;
    fixture.add_thread("t-yonbip", "OpenAI", false, 300, "OpenAI")?;
    fixture.set_thread_cwd("t-yonbip", r"\\?\D:\gpt\yonbip")?;
    fixture.write_session_index(&[("t-yonbip", "标题-t-yonbip", "1970-01-01T00:00:00.300Z")])?;
    fixture.write_global_state(&json!({
        "electron-saved-workspace-roots": [],
        "project-order": [],
        "thread-workspace-root-hints": {}
    }))?;

    let store = fixture.store();
    let first_report: RepairReport = store.execute_repair()?;
    assert_eq!(first_report.recovered_workspace_root_count, 1);
    assert_eq!(first_report.updated_thread_workspace_hint_count, 1);

    let second_report: RepairReport = store.execute_repair()?;
    assert!(!second_report.rolled_back);
    assert_eq!(second_report.normalized_cwd_count, 0);
    assert_eq!(second_report.normalized_rollout_file_count, 0);
    assert_eq!(second_report.normalized_workspace_path_count, 0);
    assert_eq!(second_report.recovered_workspace_root_count, 0);
    assert_eq!(second_report.updated_thread_workspace_hint_count, 0);
    assert!(!second_report.rebuilt_session_index);

    Ok(())
}

#[test]
fn repair_rebuilds_dirty_session_index_and_backfills_project_hints_for_yonbip_threads() -> Result<()>
{
    let fixture = Fixture::new("OpenAI")?;
    fixture.add_thread("t-old", "OpenAI", false, 100, "OpenAI")?;
    fixture.add_thread("t-new", "OpenAI", false, 200, "OpenAI")?;
    fixture.set_thread_cwd("t-old", r"\\?\D:\gpt\yonbip")?;
    fixture.set_thread_cwd("t-new", r"D:\gpt\yonbip")?;
    fixture.set_thread_title("t-old", "旧 yonbip 会话")?;
    fixture.set_thread_title("t-new", "123")?;
    fixture.write_session_index(&[
        ("t-old", "旧 yonbip 会话", "1970-01-01T00:00:00.100Z"),
        ("t-new", "Count numbers 1 2 3", "1970-01-01T00:00:00.150Z"),
        ("t-new", "123", "1970-01-01T00:00:00.200Z"),
    ])?;
    fixture.write_global_state(&json!({
        "electron-saved-workspace-roots": [r"D:\gpt\yonbip"],
        "project-order": [r"D:\gpt\yonbip"],
        "thread-workspace-root-hints": {}
    }))?;

    let store = fixture.store();
    let report: RepairReport = store.execute_repair()?;

    assert!(!report.rolled_back);
    assert_eq!(report.normalized_cwd_count, 1);
    assert_eq!(report.normalized_rollout_file_count, 0);
    assert_eq!(report.recovered_workspace_root_count, 0);
    assert_eq!(report.updated_thread_workspace_hint_count, 2);
    assert!(report.rebuilt_session_index);
    assert_eq!(
        fixture.thread_cwds()?,
        vec![
            ("t-new".to_string(), r"D:\gpt\yonbip".to_string()),
            ("t-old".to_string(), r"D:\gpt\yonbip".to_string()),
        ]
    );
    assert_eq!(
        fixture.read_session_index_entries()?,
        vec![
            (
                "t-new".to_string(),
                "123".to_string(),
                "1970-01-01T00:00:00.200Z".to_string(),
            ),
            (
                "t-old".to_string(),
                "旧 yonbip 会话".to_string(),
                "1970-01-01T00:00:00.100Z".to_string(),
            ),
        ]
    );

    let global_state = fixture.read_global_state_json()?;
    assert_eq!(
        global_state["thread-workspace-root-hints"]["t-old"],
        json!(r"D:\gpt\yonbip")
    );
    assert_eq!(
        global_state["thread-workspace-root-hints"]["t-new"],
        json!(r"D:\gpt\yonbip")
    );

    Ok(())
}

#[test]
fn merge_rolls_back_when_rollout_write_fails() -> Result<()> {
    let fixture = Fixture::new("OpenAI")?;
    fixture.add_thread("t-openai", "OpenAI", false, 100, "OpenAI")?;
    fixture.add_thread("t-custom", "custom", false, 180, "custom")?;
    fixture.write_session_index(&[
        ("t-openai", "标题-t-openai", "2026-04-18T00:00:00.100Z"),
        ("t-custom", "标题-t-custom", "2026-04-18T00:00:00.180Z"),
    ])?;

    let store = fixture.store_with_injector(FailOnWriteInjector {
        fail_on_index: Some(0),
    });
    let report: MergeReport = store.execute_merge("OpenAI")?;

    assert!(report.rolled_back);
    assert_eq!(report.migrated_thread_count, 0);
    assert_eq!(report.migrated_file_count, 0);
    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0].contains("injected rollout write failure"));

    let providers = fixture.thread_providers()?;
    assert_eq!(
        providers,
        vec![
            ("t-custom".to_string(), "custom".to_string()),
            ("t-openai".to_string(), "OpenAI".to_string()),
        ]
    );
    assert_eq!(fixture.read_session_meta_provider("t-custom")?, "custom");
    assert_eq!(
        fixture.read_session_index_entries()?,
        vec![
            (
                "t-openai".to_string(),
                "标题-t-openai".to_string(),
                "2026-04-18T00:00:00.100Z".to_string(),
            ),
            (
                "t-custom".to_string(),
                "标题-t-custom".to_string(),
                "2026-04-18T00:00:00.180Z".to_string(),
            ),
        ]
    );

    Ok(())
}

#[test]
fn backup_list_reads_entries_from_codex_merge_session_directory() -> Result<()> {
    let fixture = Fixture::new("OpenAI")?;
    fixture.add_thread("t-openai", "OpenAI", false, 100, "OpenAI")?;
    fixture.add_thread("t-custom", "custom", false, 180, "custom")?;

    let store = fixture.store();
    let merge_report = store.execute_merge("custom")?;
    assert!(!merge_report.rolled_back);

    let extra_backup_dir = fixture.write_backup_manifest("1776494198-extra-merge")?;

    let backups = store.list_backups()?;

    assert!(backups.iter().any(|entry| {
        entry.path == merge_report.backup_dir
            && entry.kind == "merge"
            && !entry.is_legacy_location
    }));
    assert!(backups.iter().any(|entry| {
        entry.path == extra_backup_dir
            && entry.kind == "merge"
            && !entry.is_legacy_location
    }));

    Ok(())
}

#[test]
fn manual_provider_changes_persist_to_settings() -> Result<()> {
    let fixture = Fixture::new("openai")?;
    let store = fixture.store();

    store.add_manual_provider("manual-added")?;
    let added_scan = store.scan()?;
    assert!(added_scan.provider_options.iter().any(|option| {
        option.id == "manual-added" && option.from_manual
    }));

    store.remove_manual_provider("manual-added")?;
    let removed_scan = store.scan()?;
    assert!(!removed_scan
        .provider_options
        .iter()
        .any(|option| option.id == "manual-added"));

    Ok(())
}

#[test]
fn restore_backup_restores_config_sqlite_and_rollout() -> Result<()> {
    let fixture = Fixture::new("OpenAI")?;
    fixture.add_thread("t-openai", "OpenAI", false, 100, "OpenAI")?;
    fixture.add_thread("t-custom", "custom", false, 180, "custom")?;

    let store = fixture.store();
    let merge_report = store.execute_merge("custom")?;
    assert!(!merge_report.rolled_back);
    assert!(fixture.read_config_text()?.contains("model_provider = \"custom\""));

    store.restore_backup(&merge_report.backup_dir)?;

    assert!(fixture.read_config_text()?.contains("model_provider = \"OpenAI\""));
    assert_eq!(
        fixture.thread_providers()?,
        vec![
            ("t-custom".to_string(), "custom".to_string()),
            ("t-openai".to_string(), "OpenAI".to_string()),
        ]
    );
    assert_eq!(fixture.read_session_meta_provider("t-openai")?, "OpenAI");

    Ok(())
}

#[test]
fn delete_backup_removes_target_directory_only() -> Result<()> {
    let fixture = Fixture::new("OpenAI")?;
    fixture.add_thread("t-openai", "OpenAI", false, 100, "OpenAI")?;
    fixture.add_thread("t-custom", "custom", false, 180, "custom")?;

    let store = fixture.store();
    let first_report = store.execute_merge("custom")?;
    assert!(!first_report.rolled_back);
    let sibling_backup_dir = fixture.write_backup_manifest("1776494198-extra-merge")?;

    store.delete_backup(&first_report.backup_dir)?;

    assert!(!first_report.backup_dir.exists());
    assert!(sibling_backup_dir.exists());

    Ok(())
}

#[test]
fn merge_blocks_when_codex_process_is_running() -> Result<()> {
    let fixture = Fixture::new("OpenAI")?;
    fixture.add_thread("t-openai", "OpenAI", false, 100, "OpenAI")?;
    fixture.add_thread("t-custom", "custom", false, 180, "custom")?;

    let store = CodexStore::new_with_hooks(
        fixture.codex_root().to_path_buf(),
        Arc::new(StaticProcessInspector {
            running_pids: vec![1234, 5678],
        }),
        Arc::new(FailOnWriteInjector::default()),
    );

    let error = store.execute_merge("OpenAI").unwrap_err();
    assert!(error.to_string().contains("1234, 5678"));
    Ok(())
}

struct Fixture {
    _temp_dir: TempDir,
    codex_root: PathBuf,
}

impl Fixture {
    fn new(current_provider: &str) -> Result<Self> {
        let temp_dir = tempfile::tempdir()?;
        let codex_root = temp_dir.path().join(".codex");
        fs::create_dir_all(
            codex_root
                .join("sessions")
                .join("2026")
                .join("04")
                .join("18"),
        )?;
        fs::create_dir_all(codex_root.join("archived_sessions"))?;
        fs::write(
            codex_root.join("config.toml"),
            format!("model_provider = \"{current_provider}\"\n"),
        )?;

        let connection = Connection::open(codex_root.join("state_5.sqlite"))?;
        connection.execute(
            "create table threads(
                id text primary key,
                rollout_path text not null,
                cwd text not null,
                title text not null,
                updated_at_ms integer,
                model_provider text not null,
                archived integer not null default 0
            )",
            [],
        )?;
        drop(connection);

        Ok(Self {
            _temp_dir: temp_dir,
            codex_root,
        })
    }

    fn codex_root(&self) -> &Path {
        &self.codex_root
    }

    fn store(&self) -> CodexStore {
        CodexStore::new_with_hooks(
            self.codex_root.clone(),
            Arc::new(StaticProcessInspector::default()),
            Arc::new(FailOnWriteInjector::default()),
        )
    }

    fn store_with_injector<T>(&self, injector: T) -> CodexStore
    where
        T: FaultInjector + 'static,
    {
        CodexStore::new_with_hooks(
            self.codex_root.clone(),
            Arc::new(StaticProcessInspector::default()),
            Arc::new(injector),
        )
    }

    fn add_thread(
        &self,
        id: &str,
        provider: &str,
        archived: bool,
        updated_at_ms: i64,
        meta_provider: &str,
    ) -> Result<()> {
        let rollout_path = self.rollout_path(id, archived);
        fs::create_dir_all(rollout_path.parent().expect("rollout parent"))?;
        fs::write(
            &rollout_path,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"model_provider\":\"{meta_provider}\"}}}}\n{{\"type\":\"noop\"}}\n"
            ),
        )?;
        self.insert_thread_only(id, provider, archived, updated_at_ms, &rollout_path)
    }

    fn insert_thread_only(
        &self,
        id: &str,
        provider: &str,
        archived: bool,
        updated_at_ms: i64,
        rollout_path: &Path,
    ) -> Result<()> {
        let connection = Connection::open(self.codex_root.join("state_5.sqlite"))?;
        connection.execute(
            "insert into threads(id, rollout_path, cwd, title, updated_at_ms, model_provider, archived)
             values(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                rollout_path.display().to_string(),
                self.codex_root.display().to_string(),
                format!("标题-{id}"),
                updated_at_ms,
                provider,
                i64::from(archived as i32)
            ],
        )?;
        Ok(())
    }

    fn rollout_path(&self, id: &str, archived: bool) -> PathBuf {
        if archived {
            self.codex_root
                .join("archived_sessions")
                .join(format!("rollout-{id}.jsonl"))
        } else {
            self.codex_root
                .join("sessions")
                .join("2026")
                .join("04")
                .join("18")
                .join(format!("rollout-{id}.jsonl"))
        }
    }

    fn thread_providers(&self) -> Result<Vec<(String, String)>> {
        let connection = Connection::open(self.codex_root.join("state_5.sqlite"))?;
        let mut statement =
            connection.prepare("select id, model_provider from threads order by id asc")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let items = rows.collect::<rusqlite::Result<Vec<(String, String)>>>()?;
        Ok(items)
    }

    fn thread_cwds(&self) -> Result<Vec<(String, String)>> {
        let connection = Connection::open(self.codex_root.join("state_5.sqlite"))?;
        let mut statement = connection.prepare("select id, cwd from threads order by id asc")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let items = rows.collect::<rusqlite::Result<Vec<(String, String)>>>()?;
        Ok(items)
    }

    fn read_session_meta_provider(&self, id: &str) -> Result<String> {
        let active_path = self.rollout_path(id, false);
        let archived_path = self.rollout_path(id, true);
        let target_path = if active_path.exists() {
            active_path
        } else {
            archived_path
        };
        let text = fs::read_to_string(target_path)?;
        let first_line = text.lines().next().expect("session meta line");
        let json: Value = serde_json::from_str(first_line)?;
        Ok(json["payload"]["model_provider"]
            .as_str()
            .expect("provider string")
            .to_string())
    }

    fn write_session_index(&self, entries: &[(&str, &str, &str)]) -> Result<()> {
        let mut content = String::new();
        for (id, thread_name, updated_at) in entries {
            content.push_str(&serde_json::to_string(&serde_json::json!({
                "id": id,
                "thread_name": thread_name,
                "updated_at": updated_at,
            }))?);
            content.push('\n');
        }
        fs::write(self.codex_root.join("session_index.jsonl"), content)?;
        Ok(())
    }

    fn set_thread_cwd(&self, id: &str, cwd: &str) -> Result<()> {
        let connection = Connection::open(self.codex_root.join("state_5.sqlite"))?;
        connection.execute(
            "update threads set cwd = ?1 where id = ?2",
            params![cwd, id],
        )?;
        Ok(())
    }

    fn set_thread_title(&self, id: &str, title: &str) -> Result<()> {
        let connection = Connection::open(self.codex_root.join("state_5.sqlite"))?;
        connection.execute(
            "update threads set title = ?1 where id = ?2",
            params![title, id],
        )?;
        Ok(())
    }

    fn set_rollout_session_meta_cwd(&self, id: &str, archived: bool, cwd: &str) -> Result<()> {
        let rollout_path = self.rollout_path(id, archived);
        let text = fs::read_to_string(&rollout_path)?;
        let (first_line, rest) = text
            .split_once('\n')
            .map_or((text.as_str(), ""), |(head, tail)| (head, tail));
        let mut json: Value = serde_json::from_str(first_line)?;
        json["payload"]["cwd"] = json!(cwd);

        let mut updated = serde_json::to_string(&json)?;
        if !rest.is_empty() {
            updated.push('\n');
            updated.push_str(rest);
        }
        fs::write(rollout_path, updated)?;
        Ok(())
    }

    fn append_rollout_json_line(&self, id: &str, archived: bool, value: &Value) -> Result<()> {
        let rollout_path = self.rollout_path(id, archived);
        let mut text = fs::read_to_string(&rollout_path)?;
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&serde_json::to_string(value)?);
        text.push('\n');
        fs::write(rollout_path, text)?;
        Ok(())
    }

    fn read_rollout_text(&self, id: &str, archived: bool) -> Result<String> {
        Ok(fs::read_to_string(self.rollout_path(id, archived))?)
    }

    fn write_global_state(&self, value: &Value) -> Result<()> {
        fs::write(
            self.codex_root.join(".codex-global-state.json"),
            serde_json::to_vec(value)?,
        )?;
        Ok(())
    }

    fn read_global_state_json(&self) -> Result<Value> {
        Ok(serde_json::from_slice(&fs::read(
            self.codex_root.join(".codex-global-state.json"),
        )?)?)
    }

    fn read_repair_manifest_json(&self, backup_dir: &Path) -> Result<Value> {
        Ok(serde_json::from_slice(&fs::read(
            backup_dir.join("manifest.json"),
        )?)?)
    }

    fn read_session_index_entries(&self) -> Result<Vec<(String, String, String)>> {
        let text = fs::read_to_string(self.codex_root.join("session_index.jsonl"))?;
        let mut entries = Vec::new();
        for line in text.lines() {
            let json: Value = serde_json::from_str(line)?;
            entries.push((
                json["id"].as_str().expect("id").to_string(),
                json["thread_name"]
                    .as_str()
                    .expect("thread_name")
                    .to_string(),
                json["updated_at"].as_str().expect("updated_at").to_string(),
            ));
        }
        Ok(entries)
    }

    fn write_config_text(&self, content: &str) -> Result<()> {
        fs::write(self.codex_root.join("config.toml"), content)?;
        Ok(())
    }

    fn read_config_text(&self) -> Result<String> {
        Ok(fs::read_to_string(self.codex_root.join("config.toml"))?)
    }

    fn write_rollout_without_sqlite(&self, id: &str, provider: &str, archived: bool) -> Result<()> {
        let rollout_path = self.rollout_path(id, archived);
        fs::create_dir_all(rollout_path.parent().expect("rollout parent"))?;
        fs::write(
            rollout_path,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"model_provider\":\"{provider}\"}}}}\n{{\"type\":\"noop\"}}\n"
            ),
        )?;
        Ok(())
    }

    fn write_manual_settings(&self, manual_providers: &[&str]) -> Result<()> {
        let settings_dir = self
            .codex_root
            .parent()
            .expect("settings dir parent")
            .join(".codex-merge-session");
        fs::create_dir_all(&settings_dir)?;
        fs::write(
            settings_dir.join("settings.json"),
            serde_json::to_vec(&json!({
                "manual_providers": manual_providers,
                "last_selected_provider": null,
            }))?,
        )?;
        Ok(())
    }

    fn backup_root(&self) -> PathBuf {
        self.codex_root
            .parent()
            .expect("backup parent")
            .join(".codex-merge-session")
            .join("backups")
    }

    fn write_backup_manifest(&self, name: &str) -> Result<PathBuf> {
        let backup_dir = self.backup_root().join(name);
        fs::create_dir_all(&backup_dir)?;
        fs::write(
            backup_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&json!({
                "created_at_unix_seconds": 1_776_494_198_u64,
                "target_provider": "custom",
                "source_providers": ["OpenAI"],
                "affected_thread_ids": ["thread-1"],
                "backed_up_files": [],
            }))?,
        )?;
        Ok(backup_dir)
    }
}
