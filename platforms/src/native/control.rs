use crate::process::{
    Error, ErrorKind, ProcessInstance, ResourceSnapshot, Result, inspect_process, read_resources,
};
use std::{
    io::Write,
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

pub struct BorrowedProcess {
    instance: ProcessInstance,
    closed: bool,
}
impl BorrowedProcess {
    pub fn attach(instance: ProcessInstance) -> Result<Self> {
        if !instance.same_instance(&inspect_process(instance.pid)?) {
            return Err(Error::new(
                ErrorKind::InstanceChanged,
                "borrowed target instance changed",
            ));
        }
        Ok(Self {
            instance,
            closed: false,
        })
    }
    pub fn snapshot(&self) -> Result<ResourceSnapshot> {
        if self.closed {
            return Err(Error::invalid("borrowed source is closed"));
        }
        read_resources(&self.instance)
    }
    pub fn stop(&mut self) {
        self.closed = true;
    }
    pub fn instance(&self) -> &ProcessInstance {
        &self.instance
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnedScope {
    Process,
    ProcessGroup,
}

pub struct OwnedProcess {
    child: Child,
    instance: ProcessInstance,
    #[cfg_attr(windows, allow(dead_code))]
    scope: OwnedScope,
    exited: Option<ExitStatus>,
    gate_released: bool,
    #[cfg(windows)]
    job: Option<WindowsJob>,
}
impl OwnedProcess {
    pub fn spawn(command: &mut Command, scope: OwnedScope) -> Result<Self> {
        command.stdin(Stdio::piped());
        #[cfg(unix)]
        if scope == OwnedScope::ProcessGroup {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        if scope == OwnedScope::ProcessGroup {
            use std::os::windows::process::CommandExt;
            command.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
        }
        let mut child = command.spawn()?;
        #[cfg(windows)]
        let job = if scope == OwnedScope::ProcessGroup {
            match WindowsJob::assign_and_resume(&child) {
                Ok(job) => Some(job),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
            }
        } else {
            None
        };
        let instance = match inspect_process(child.id()) {
            Ok(instance) => instance,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        Ok(Self {
            child,
            instance,
            scope,
            exited: None,
            gate_released: false,
            #[cfg(windows)]
            job,
        })
    }
    pub fn instance(&self) -> &ProcessInstance {
        &self.instance
    }
    pub fn take_stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.child.stdout.take()
    }
    pub fn take_stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.child.stderr.take()
    }
    pub fn release(&mut self, bytes: &[u8]) -> Result<()> {
        if self.gate_released || bytes.is_empty() || bytes.len() > 512 {
            return Err(Error::invalid(
                "one gate release of 1..512 bytes is permitted",
            ));
        }
        self.gate_released = true;
        self.child
            .stdin
            .as_mut()
            .ok_or_else(|| Error::invalid("owned target has no input gate"))?
            .write_all(bytes)?;
        self.child.stdin.as_mut().unwrap().flush()?;
        Ok(())
    }
    pub fn poll(&mut self) -> Result<Option<ExitStatus>> {
        if self.exited.is_none() {
            #[cfg(unix)]
            if self.scope == OwnedScope::ProcessGroup {
                let mut information: libc::siginfo_t = unsafe { std::mem::zeroed() };
                if unsafe {
                    libc::waitid(
                        libc::P_PID,
                        self.child.id() as libc::id_t,
                        &mut information,
                        libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                    )
                } != 0
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                if unsafe { information.si_pid() } == 0 {
                    return Ok(None);
                }
                // Keep the unreaped leader's pid reserved until its accepted group has been stopped.
                stop_group(self.child.id())?;
            }
            let status = self.child.try_wait()?;
            #[cfg(windows)]
            if status.is_some()
                && let Some(job) = &self.job
            {
                job.terminate()?;
            }
            self.exited = status;
        }
        Ok(self.exited)
    }
    pub fn wait(&mut self, timeout: Duration) -> Result<ExitStatus> {
        let start = Instant::now();
        loop {
            if let Some(status) = self.poll()? {
                return Ok(status);
            }
            if start.elapsed() >= timeout {
                return Err(Error::new(
                    ErrorKind::Timeout,
                    "owned process wait timed out",
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    pub fn cancel(&mut self, graceful: Duration, forced: Duration) -> Result<ExitStatus> {
        if let Some(status) = self.exited {
            return Ok(status);
        }
        #[cfg(unix)]
        {
            let pid = if self.scope == OwnedScope::ProcessGroup {
                -(self.child.id() as i32)
            } else {
                self.child.id() as i32
            };
            if unsafe { libc::kill(pid, libc::SIGTERM) } != 0
                && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
            {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        #[cfg(windows)]
        {
            if let Some(job) = &self.job {
                job.terminate()?;
            } else {
                self.child.kill()?;
            }
        }
        match self.wait(graceful) {
            Ok(status) => Ok(status),
            Err(error) if error.kind == ErrorKind::Timeout => {
                #[cfg(unix)]
                {
                    let pid = if self.scope == OwnedScope::ProcessGroup {
                        -(self.child.id() as i32)
                    } else {
                        self.child.id() as i32
                    };
                    if unsafe { libc::kill(pid, libc::SIGKILL) } != 0
                        && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
                    {
                        return Err(std::io::Error::last_os_error().into());
                    }
                }
                #[cfg(windows)]
                {
                    if let Some(job) = &self.job {
                        job.terminate()?;
                    } else {
                        self.child.kill()?;
                    }
                }
                self.wait(forced)
            }
            Err(error) => Err(error),
        }
    }
}
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        if self.exited.is_none() {
            #[cfg(unix)]
            if self.scope == OwnedScope::ProcessGroup {
                unsafe {
                    libc::kill(-(self.child.id() as i32), libc::SIGKILL);
                }
            }
            let _ = self.child.kill();
            let _ = self.wait(Duration::from_secs(2));
        }
    }
}

#[cfg(windows)]
pub(crate) struct WindowsJob(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
impl WindowsJob {
    pub(crate) fn assign_and_resume(child: &Child) -> Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
            System::{Diagnostics::ToolHelp::*, JobObjects::*, Threading::*},
        };
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let job = Self(raw);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                raw,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
            || unsafe { AssignProcessToJobObject(raw, child.as_raw_handle()) } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().into());
        }
        let result = (|| {
            let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
            entry.dwSize = std::mem::size_of_val(&entry) as u32;
            let mut available = unsafe { Thread32First(snapshot, &mut entry) };
            while available != 0 {
                if entry.th32OwnerProcessID == child.id() {
                    let thread =
                        unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                    if thread.is_null() {
                        return Err(std::io::Error::last_os_error().into());
                    }
                    let resumed = unsafe { ResumeThread(thread) };
                    unsafe {
                        CloseHandle(thread);
                    }
                    if resumed == u32::MAX {
                        return Err(std::io::Error::last_os_error().into());
                    }
                    return Ok(());
                }
                available = unsafe { Thread32Next(snapshot, &mut entry) };
            }
            Err(Error::new(
                ErrorKind::CollectorFailure,
                "suspended primary thread was not found",
            ))
        })();
        unsafe {
            CloseHandle(snapshot);
        }
        result?;
        Ok(job)
    }
    pub(crate) fn terminate(&self) -> Result<()> {
        if unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
}
#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(unix)]
pub(crate) fn stop_group(leader: u32) -> Result<()> {
    if unsafe { libc::kill(-(leader as i32), libc::SIGKILL) } == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    // Darwin reports EPERM for a group whose sole remaining member is its unreaped leader.
    if error.raw_os_error() == Some(libc::EPERM) {
        for pid in crate::os::pids()? {
            if pid != leader && unsafe { libc::getpgid(pid as i32) } == leader as i32 {
                return Err(error.into());
            }
        }
        return Ok(());
    }
    Err(error.into())
}

impl hunin_core::execution::ExecutionChild for OwnedProcess {
    fn id(&self) -> u32 {
        self.instance.pid
    }
    fn take_stdin(&mut self) -> Option<std::process::ChildStdin> {
        self.child.stdin.take()
    }
    fn take_stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.child.stdout.take()
    }
    fn take_stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.child.stderr.take()
    }
    fn poll(&mut self) -> std::io::Result<Option<ExitStatus>> {
        OwnedProcess::poll(self).map_err(std::io::Error::other)
    }
    fn terminate(&mut self) -> std::io::Result<()> {
        if self.exited.is_some() {
            return Ok(());
        }
        #[cfg(unix)]
        {
            if self.scope == OwnedScope::ProcessGroup {
                stop_group(self.instance.pid).map_err(std::io::Error::other)?;
            } else {
                self.child.kill()?;
            }
        }
        #[cfg(windows)]
        {
            if let Some(job) = &self.job {
                job.terminate().map_err(std::io::Error::other)?;
            } else {
                self.child.kill()?;
            }
        }
        Ok(())
    }
    fn reap(&mut self) -> std::io::Result<ExitStatus> {
        self.wait(Duration::from_secs(5))
            .map_err(std::io::Error::other)
    }
}

pub fn spawn_prepared(
    spec: &hunin_core::execution::PreparedLaunch,
    scope: OwnedScope,
) -> Result<hunin_core::execution::PreparedProcess> {
    if !spec.program.is_absolute()
        || !spec.directory.is_absolute()
        || spec.stdout_limit > 16 * 1024 * 1024
    {
        return Err(Error::invalid(
            "prepared launch requires absolute paths and output bound at most 16 mib",
        ));
    }
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .current_dir(&spec.directory)
        .envs(&spec.environment)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = OwnedProcess::spawn(&mut command, scope)?;
    hunin_core::execution::PreparedProcess::from_child(Box::new(child), spec.stdout_limit)
        .map_err(|e| Error::new(ErrorKind::CollectorFailure, e.to_string()))
}

#[cfg(windows)]
unsafe impl Send for WindowsJob {}
