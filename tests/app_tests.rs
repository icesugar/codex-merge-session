use anyhow::Result;
use tempfile::tempdir;

use codex_merge_session::app::CodexMergeApp;
use codex_merge_session::codex_store::CodexStore;

#[test]
fn app_can_be_created_without_status_banner() -> Result<()> {
    let temp_dir = tempdir()?;
    let store = CodexStore::new(temp_dir.path().join(".codex"));

    let _app = CodexMergeApp::new(store);

    Ok(())
}
