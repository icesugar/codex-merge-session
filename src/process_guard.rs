use anyhow::Result;
use sysinfo::{ProcessesToUpdate, System};

use crate::codex_store::ProcessInspector;

pub struct LiveProcessInspector;

impl ProcessInspector for LiveProcessInspector {
    fn running_codex_pids(&self) -> Result<Vec<u32>> {
        let mut system = System::new_all();
        system.refresh_processes(ProcessesToUpdate::All, true);

        let mut pids: Vec<u32> = system
            .processes()
            .values()
            .filter_map(|process| {
                let name = process.name().to_string_lossy().to_ascii_lowercase();
                if matches!(name.as_str(), "codex.exe" | "codex") {
                    Some(process.pid().as_u32())
                } else {
                    None
                }
            })
            .collect();
        pids.sort_unstable();
        pids.dedup();
        Ok(pids)
    }
}
