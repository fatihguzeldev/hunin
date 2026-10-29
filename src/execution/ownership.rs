use crate::model::runs::RunStatus;
use crate::{model::now_us, storage::Store};
use anyhow::Result;
use serde::Serialize;
use serde_json::{Value, json};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

#[derive(Clone, Serialize)]
pub struct Owner {
    pub pid: u32,
    pub process_start_time_seconds: Option<u64>,
    pub process_birth_identity: Option<String>,
    pub instance_id: String,
}

impl Owner {
    pub fn current() -> Self {
        let pid = std::process::id();
        let mut system = System::new();
        refresh(&mut system, &[Pid::from_u32(pid)]);
        Self {
            pid,
            process_birth_identity: super::process_identity::birth(pid),
            process_start_time_seconds: system
                .process(Pid::from_u32(pid))
                .map(|p| p.start_time())
                .filter(|t| *t > 0),
            instance_id: uuid::Uuid::new_v4().to_string(),
        }
    }
}

pub fn recover_abandoned(store: &Store) -> Result<()> {
    let mut offset = 0;
    let mut running = Vec::new();
    loop {
        let runs = store.runs(250, offset)?;
        let count = runs.len();
        running.extend(runs.into_iter().filter(|run| run.status == RunStatus::Running));
        if count < 250 {
            break;
        }
        offset += count;
    }
    if running.is_empty() {
        return Ok(());
    }
    let pids = running
        .iter()
        .filter_map(|run| run.source["owner"]["pid"].as_u64())
        .filter_map(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 0)
        .map(Pid::from_u32)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut system = System::new();
    refresh(&mut system, &pids);
    for mut run in running {
        if !owner_absent(&run.source["owner"], &system) {
            continue;
        }
        run.status = RunStatus::Failed;
        run.finished_at_us = Some(now_us());
        run.diagnostics.push("Recording interrupted: its owning Hunin process is absent or has a different OS start identity. Preserved artifacts remain available; no stale PID was signaled.".into());
        run.source["collection_status"] = json!("interrupted");
        store.save_run(&run)?;
    }
    Ok(())
}

pub fn is_proven_absent(value: &Value) -> bool {
    let mut system = System::new();
    if let Some(pid) = value["pid"].as_u64().and_then(|v| u32::try_from(v).ok()) {
        refresh(&mut system, &[Pid::from_u32(pid)]);
    }
    owner_absent(value, &system)
}

fn refresh(system: &mut System, pids: &[Pid]) {
    if !pids.is_empty() {
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(pids),
            true,
            ProcessRefreshKind::nothing().without_tasks(),
        );
    }
}

fn owner_absent(value: &Value, system: &System) -> bool {
    let Some(pid) = value["pid"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
    else {
        return false;
    };
    // A terminated Windows process remains queryable while any handle is
    // retained. Matching creation time proves identity, not that it is alive.
    #[cfg(windows)]
    if windows_proven_absent(pid) {
        return true;
    }
    if let Some(saved) = value["process_birth_identity"].as_str() {
        if let Some(current) = super::process_identity::birth(pid) {
            return saved != current;
        }
    }
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
    #[cfg(not(unix))]
    false
}

#[cfg(windows)]
fn windows_proven_absent(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, WAIT_OBJECT_0},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return GetLastError() == ERROR_INVALID_PARAMETER;
        }
        let state = WaitForSingleObject(handle, 0);
        CloseHandle(handle);
        // WAIT_FAILED (including denied access) remains unknown. The signaled
        // process object also handles a legitimate exit code259 correctly.
        state == WAIT_OBJECT_0
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::process::Command;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_OBJECT_0},
        System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE, WaitForSingleObject,
        },
    };

    #[test]
    fn windows_owner_exit259_fixture() {
        let Some(release) = std::env::var_os("HUNIN_OWNER_EXIT259_RELEASE") else {
            return;
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !std::path::Path::new(&release).exists() {
            if std::time::Instant::now() >= deadline {
                std::process::exit(77);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        std::process::exit(259);
    }

    #[test]
    fn terminated_owner_with_retained_handle_and_exit259_is_proven_absent() {
        let directory = tempfile::tempdir().unwrap();
        let release = directory.path().join("release");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "execution::ownership::tests::windows_owner_exit259_fixture",
            ])
            .env("HUNIN_OWNER_EXIT259_RELEASE", &release)
            .spawn()
            .unwrap();
        let pid = child.id();
        let birth = super::super::process_identity::birth(pid).unwrap();
        let owner = json!({"pid":pid,"process_birth_identity":birth});
        assert!(!is_proven_absent(&owner));
        struct Held(windows_sys::Win32::Foundation::HANDLE);
        impl Drop for Held {
            fn drop(&mut self) {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
        let held = Held(unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        });
        assert!(!held.0.is_null());
        std::fs::write(release, b"exit").unwrap();
        assert_eq!(child.wait().unwrap().code(), Some(259));
        assert_eq!(unsafe { WaitForSingleObject(held.0, 0) }, WAIT_OBJECT_0);
        let mut code = 0;
        assert_ne!(unsafe { GetExitCodeProcess(held.0, &mut code) }, 0);
        assert_eq!(code, 259);
        assert_eq!(
            super::super::process_identity::birth(pid).as_deref(),
            owner["process_birth_identity"].as_str()
        );
        assert!(
            is_proven_absent(&owner),
            "signaled owner must recover even though creation time is still queryable"
        );
    }
}
