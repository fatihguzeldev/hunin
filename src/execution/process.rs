use anyhow::{Context, Result};
use std::{process::ExitStatus, time::Duration};
use tokio::process::{Child, Command};

/// Owns only the process group/job created for this recording.
pub struct ProcessTree {
    pid: u32,
    active: bool,
    #[cfg(windows)]
    job: std::os::windows::io::OwnedHandle,
}

pub async fn spawn(command: &mut Command) -> Result<(Child, ProcessTree)> {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(
        windows_sys::Win32::System::Threading::CREATE_SUSPENDED
            | windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP,
    );
    let child = command.spawn().context("launch measured command")?;
    let pid = child.id().context("launched process has no PID")?;
    #[cfg(windows)]
    {
        let mut child = child;
        let attached = {
            let handle = child
                .raw_handle()
                .context("suspended child process handle missing")?;
            windows::attach(handle, pid)
        };
        match attached {
            Ok(job) => {
                return Ok((
                    child,
                    ProcessTree {
                        pid,
                        job,
                        active: true,
                    },
                ));
            }
            Err(error) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(error.context("assign suspended recording process to its Windows Job"));
            }
        }
    }
    #[cfg(not(windows))]
    Ok((child, ProcessTree { pid, active: true }))
}

pub(crate) fn spawn_std(
    command: &mut std::process::Command,
) -> Result<(std::process::Child, ProcessTree)> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(
            windows_sys::Win32::System::Threading::CREATE_SUSPENDED
                | windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP,
        );
    }
    let child = command.spawn().context("launch bounded collector probe")?;
    let pid = child.id();
    #[cfg(windows)]
    {
        let mut child = child;
        use std::os::windows::io::AsRawHandle;
        match windows::attach(child.as_raw_handle(), pid) {
            Ok(job) => {
                return Ok((
                    child,
                    ProcessTree {
                        pid,
                        job,
                        active: true,
                    },
                ));
            }
            Err(error) => {
                let _ = child.kill();
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Err(error.context("assign suspended collector probe to Windows Job"));
            }
        }
    }
    #[cfg(not(windows))]
    Ok((child, ProcessTree { pid, active: true }))
}

impl ProcessTree {
    pub async fn stop(&mut self, child: &mut Child, collector: bool) -> Result<(ExitStatus, bool)> {
        self.interrupt(collector);
        if let Ok(status) = tokio::time::timeout(Duration::from_millis(1500), child.wait()).await {
            let status = status?;
            self.force();
            return Ok((status, false));
        }
        self.force();
        let _ = child.start_kill();
        Ok((child.wait().await?, true))
    }

    fn interrupt(&self, collector: bool) {
        #[cfg(unix)]
        unsafe {
            libc::kill(
                -(self.pid as i32),
                if collector {
                    libc::SIGINT
                } else {
                    libc::SIGTERM
                },
            );
        }
        #[cfg(windows)]
        unsafe {
            let _ = collector;
            windows_sys::Win32::System::Console::GenerateConsoleCtrlEvent(
                windows_sys::Win32::System::Console::CTRL_BREAK_EVENT,
                self.pid,
            );
        }
    }

    pub fn force(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.pid as i32), libc::SIGKILL);
        }
        #[cfg(windows)]
        unsafe {
            use std::os::windows::io::AsRawHandle;
            windows_sys::Win32::System::JobObjects::TerminateJobObject(
                self.job.as_raw_handle(),
                137,
            );
        }
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        self.force();
    }
}

#[cfg(windows)]
mod windows {
    use anyhow::{Result, bail};
    use std::{
        io,
        mem::{size_of, zeroed},
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{HANDLE, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
            Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
        },
    };

    unsafe fn owned(handle: HANDLE) -> Result<OwnedHandle> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error().into());
        }
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }

    pub fn attach(process: HANDLE, pid: u32) -> Result<OwnedHandle> {
        unsafe {
            let job = owned(CreateJobObjectW(ptr::null(), ptr::null()))?;
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                return Err(io::Error::last_os_error().into());
            }
            if AssignProcessToJobObject(job.as_raw_handle(), process) == 0 {
                return Err(io::Error::last_os_error().into());
            }
            // CREATE_SUSPENDED prevents the target spawning unowned children before
            // Job assignment. Enumerate only its still-suspended primary thread.
            let snapshot = owned(CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0))?;
            let mut entry: THREADENTRY32 = zeroed();
            entry.dwSize = size_of::<THREADENTRY32>() as u32;
            let mut found = Thread32First(snapshot.as_raw_handle(), &mut entry);
            while found != 0 {
                if entry.th32OwnerProcessID == pid {
                    let thread = owned(OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID))?;
                    if ResumeThread(thread.as_raw_handle()) == u32::MAX {
                        return Err(io::Error::last_os_error().into());
                    }
                    return Ok(job);
                }
                found = Thread32Next(snapshot.as_raw_handle(), &mut entry);
            }
            bail!("suspended child primary thread not found")
        }
    }
}
