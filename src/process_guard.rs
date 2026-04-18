use std::thread;
use std::time::Duration;

use anyhow::Result;
use sysinfo::{Pid, ProcessesToUpdate, System};

use crate::codex_store::ProcessInspector;

pub struct LiveProcessInspector;

impl ProcessInspector for LiveProcessInspector {
    fn running_codex_pids(&self) -> Result<Vec<u32>> {
        let mut system = System::new_all();
        system.refresh_processes(ProcessesToUpdate::All, true);

        let mut pids: Vec<u32> = system
            .processes()
            .values()
            .filter_map(codex_process_pid)
            .collect();
        pids.sort_unstable();
        pids.dedup();
        Ok(pids)
    }

    fn terminate_codex_pids(&self, pids: &[u32]) -> Result<Vec<u32>> {
        if pids.is_empty() {
            return Ok(Vec::new());
        }

        let mut system = System::new_all();
        system.refresh_processes(ProcessesToUpdate::All, true);

        for pid in pids {
            let sys_pid = Pid::from_u32(*pid);
            let Some(process) = system.process(sys_pid) else {
                continue;
            };
            if is_codex_process(process) {
                process.kill();
            }
        }

        thread::sleep(Duration::from_millis(500));

        let mut verify_system = System::new_all();
        verify_system.refresh_processes(ProcessesToUpdate::All, true);
        let mut still_running = pids
            .iter()
            .copied()
            .filter(|pid| {
                verify_system
                    .process(Pid::from_u32(*pid))
                    .is_some_and(is_codex_process)
            })
            .collect::<Vec<_>>();
        still_running.sort_unstable();
        still_running.dedup();
        Ok(still_running)
    }
}

fn codex_process_pid(process: &sysinfo::Process) -> Option<u32> {
    is_codex_process(process).then(|| process.pid().as_u32())
}

fn is_codex_process(process: &sysinfo::Process) -> bool {
    let name = process.name().to_string_lossy().to_ascii_lowercase();
    matches!(name.as_str(), "codex.exe" | "codex")
}
