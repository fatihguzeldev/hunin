use crate::process::{Counter, Error, ErrorKind, MemoryReading, ProcessInstance, Result};
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
    match unsafe { WaitForSingleObject(handle.0, 0) } {
        WAIT_TIMEOUT => Ok(handle),
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
    Ok(ProcessInstance {
        platform: "windows".into(),
        pid,
        parent_pid: None,
        boot_identity: None,
        namespace: None,
        start_identity: format!("creation-filetime:{creation}"),
        executable,
        name,
    })
}
pub(crate) fn pids() -> Result<Vec<u32>> {
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
            result.push(entry.th32ProcessID);
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
