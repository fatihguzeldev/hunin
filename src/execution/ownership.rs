use crate::{model::now_us, storage::Store};
use anyhow::Result;
use serde::Serialize;
use serde_json::{Value, json};
use sysinfo::{Pid, ProcessesToUpdate, System};

#[derive(Clone, Serialize)]
pub struct Owner {
    pub pid: u32,
    pub process_start_time_seconds: Option<u64>,
    pub instance_id: String,
}

impl Owner {
    pub fn current() -> Self {
        let pid = std::process::id();
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
        Self {
            pid,
            process_start_time_seconds: system
                .process(Pid::from_u32(pid))
                .map(|p| p.start_time())
                .filter(|t| *t > 0),
            instance_id: uuid::Uuid::new_v4().to_string(),
        }
    }
}

pub fn recover_abandoned(store: &Store) -> Result<()> {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    let mut offset = 0;
    loop {
        let runs = store.runs(250, offset)?;
        let count = runs.len();
        for mut run in runs {
            if run.status != "running" || !owner_absent(&run.source["owner"], &system) {
                continue;
            }
            run.status = "failed".into();
            run.finished_at_us = Some(now_us());
            run.diagnostics.push("Recording interrupted: its owning Hunin process is absent or has a different OS start identity. Preserved artifacts remain available; no stale PID was signaled.".into());
            run.source["collection_status"] = json!("interrupted");
            store.save_run(&run)?;
        }
        if count < 250 {
            break;
        }
        offset += count;
    }
    Ok(())
}

pub fn is_proven_absent(value: &Value) -> bool {
    let mut system = System::new();
    if let Some(pid) = value["pid"].as_u64().and_then(|v| u32::try_from(v).ok()) {
        system.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
    }
    owner_absent(value, &system)
}

fn owner_absent(value: &Value, system: &System) -> bool {
    let Some(pid) = value["pid"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
    else {
        return false;
    };
    if let Some(process) = system.process(Pid::from_u32(pid)) {
        return value["process_start_time_seconds"]
            .as_u64()
            .filter(|t| *t > 0)
            .is_some_and(|saved| process.start_time() > 0 && saved != process.start_time());
    }
    // A missing sysinfo row alone can be a permission/query failure. Require an
    // OS liveness result that proves absence before recovering another owner.
    #[cfg(unix)]
    unsafe {
        return libc::kill(pid as i32, 0) == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, GetLastError},
            System::Threading::{
                GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        };
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return GetLastError() == ERROR_INVALID_PARAMETER;
        }
        let mut code = 259;
        let queried = GetExitCodeProcess(handle, &mut code) != 0;
        CloseHandle(handle);
        return queried && code != 259;
    }
    #[cfg(not(any(unix, windows)))]
    false
}
