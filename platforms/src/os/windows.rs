use crate::process::{
    Counter, Discovery, Error, ErrorKind, MemoryReading, ProcessInstance, Result, UnavailableTarget,
};
use std::{mem, path::PathBuf};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    },
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
        Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
        ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        },
        Threading::{
            GetCurrentProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW, WaitForSingleObject,
        },
    },
};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn open(pid: u32) -> Result<Handle> {
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if handle.is_null() {
        return Err(std::io::Error::last_os_error().into());
    }
    let handle = Handle(handle);
    ensure_live(handle.0)?;
    Ok(handle)
}
fn ensure_live(handle: HANDLE) -> Result<()> {
    match unsafe { WaitForSingleObject(handle, 0) } {
        WAIT_TIMEOUT => Ok(()),
        WAIT_OBJECT_0 => Err(Error::new(ErrorKind::ProcessGone, "process has exited")),
        WAIT_FAILED => Err(std::io::Error::last_os_error().into()),
        _ => Err(Error::new(
            ErrorKind::CollectorFailure,
            "unexpected process wait state",
        )),
    }
}
fn integer(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}
fn times(handle: HANDLE) -> Result<(u64, u64, u64)> {
    let mut creation: FILETIME = unsafe { mem::zeroed() };
    let mut exit: FILETIME = unsafe { mem::zeroed() };
    let mut kernel: FILETIME = unsafe { mem::zeroed() };
    let mut user: FILETIME = unsafe { mem::zeroed() };
    if unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok((integer(creation), integer(user), integer(kernel)))
}
pub(crate) fn inspect(pid: u32) -> Result<ProcessInstance> {
    let handle = open(pid)?;
    let entries = process_entries()?;
    let parent = entries
        .iter()
        .find_map(|(child, parent)| (*child == pid).then_some(*parent));
    inspect_handle(pid, &handle, parent)
}
pub(crate) fn inspect_for_resources(pid: u32) -> Result<ProcessInstance> {
    let handle = open(pid)?;
    inspect_handle(pid, &handle, None)
}
fn verified_parent(
    child_creation: u64,
    parent_creation: Option<u64>,
    parent_pid: u32,
) -> Option<u32> {
    parent_creation
        .filter(|created| *created <= child_creation)
        .map(|_| parent_pid)
}
fn inspect_handle(pid: u32, handle: &Handle, parent: Option<u32>) -> Result<ProcessInstance> {
    let (creation, _, _) = times(handle.0)?;
    let mut path = vec![0_u16; 32768];
    let mut length = path.len() as u32;
    let executable = if unsafe {
        QueryFullProcessImageNameW(handle.0, 0, path.as_mut_ptr(), &mut length)
    } != 0
    {
        Some(PathBuf::from(String::from_utf16_lossy(
            &path[..length as usize],
        )))
    } else {
        None
    };
    let name = executable
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parent_pid = parent.filter(|p| *p != 0).and_then(|parent| {
        let handle = open(parent).ok()?;
        let (parent_creation, _, _) = times(handle.0).ok()?;
        verified_parent(creation, Some(parent_creation), parent)
    });
    ensure_live(handle.0)?;
    Ok(ProcessInstance {
        platform: "windows".into(),
        pid,
        parent_pid,
        boot_identity: None,
        namespace: None,
        start_identity: format!("creation-filetime:{creation}"),
        executable,
        name,
    })
}
fn process_entries() -> Result<Vec<(u32, u32)>> {
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error().into());
    }
    let snapshot = Handle(raw);
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut result = Vec::new();
    let mut available = unsafe { Process32FirstW(snapshot.0, &mut entry) };
    while available != 0 {
        if entry.th32ProcessID > 0 {
            result.push((entry.th32ProcessID, entry.th32ParentProcessID));
        }
        if result.len() > 1_000_000 {
            return Err(Error::new(
                ErrorKind::LimitExceeded,
                "process discovery exceeds one million ids",
            ));
        }
        available = unsafe { Process32NextW(snapshot.0, &mut entry) };
    }
    let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
    if code != windows_sys::Win32::Foundation::ERROR_NO_MORE_FILES {
        return Err(std::io::Error::from_raw_os_error(code as i32).into());
    }
    Ok(result)
}

pub(crate) fn discover() -> Result<Discovery> {
    let mut result = Discovery {
        processes: Vec::new(),
        unavailable: Vec::new(),
    };
    let mut held = Vec::new();
    for (pid, _) in process_entries()? {
        match crate::process::validate_pid(pid).and_then(|()| open(pid)) {
            Ok(handle) => held.push((pid, handle)),
            Err(error) => result.unavailable.push(UnavailableTarget { pid, error }),
        }
    }
    // Hold process objects while taking parent entries so their ids cannot identify replacements.
    let parents: std::collections::BTreeMap<_, _> = process_entries()?.into_iter().collect();
    for (pid, handle) in held {
        match inspect_handle(pid, &handle, parents.get(&pid).copied()) {
            Ok(process) => result.processes.push(process),
            Err(error) => result.unavailable.push(UnavailableTarget { pid, error }),
        }
    }
    Ok(result)
}

pub(crate) fn resources(pid: u32) -> Result<(Counter, Counter, Vec<MemoryReading>)> {
    let handle = open(pid)?;
    let (_, user, kernel) = times(handle.0)?;
    let mut memory: PROCESS_MEMORY_COUNTERS_EX = unsafe { mem::zeroed() };
    memory.cb = mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    if unsafe {
        GetProcessMemoryInfo(
            handle.0,
            (&mut memory as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
            memory.cb,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok((
        Counter::new(user, 100, 1, "100ns")?,
        Counter::new(kernel, 100, 1, "100ns")?,
        vec![
            MemoryReading {
                definition: "windows.PROCESS_MEMORY_COUNTERS_EX.WorkingSetSize".into(),
                bytes: memory.WorkingSetSize as u64,
                approximate: false,
            },
            MemoryReading {
                definition: "windows.PROCESS_MEMORY_COUNTERS_EX.PrivateUsage".into(),
                bytes: memory.PrivateUsage as u64,
                approximate: false,
            },
        ],
    ))
}
pub(crate) fn monotonic_ns() -> Result<u64> {
    let mut value = 0_i64;
    let mut frequency = 0_i64;
    if unsafe { QueryPerformanceCounter(&mut value) } == 0
        || unsafe { QueryPerformanceFrequency(&mut frequency) } == 0
        || value < 0
        || frequency <= 0
    {
        return Err(Error::new(
            ErrorKind::CollectorFailure,
            "performance counter unavailable",
        ));
    }
    Counter::new(value as u64, 1_000_000_000, frequency as u64, "qpc_ticks")?.nanoseconds()
}
pub(crate) fn clock_identity() -> Result<String> {
    let (creation, _, _) = times(unsafe { GetCurrentProcess() })?;
    Ok(format!(
        "windows.QPC:observer:{}:{creation}",
        std::process::id()
    ))
}

#[cfg(test)]
mod parent_tests {
    use super::verified_parent;
    #[test]
    fn reused_or_unavailable_parent_ids_do_not_become_parentage() {
        assert_eq!(verified_parent(100, Some(101), 42), None);
        assert_eq!(verified_parent(100, None, 42), None);
        assert_eq!(verified_parent(100, Some(99), 42), Some(42));
    }
}
