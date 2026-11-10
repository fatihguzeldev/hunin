use crate::process::{Error, ErrorKind, Result};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

static NEXT_OUTPUT: AtomicU64 = AtomicU64::new(0);

struct ChildGuard {
    child: std::process::Child,
    exited: bool,
    #[cfg(windows)]
    job: crate::native::control::WindowsJob,
}
impl ChildGuard {
    fn poll(&mut self) -> std::io::Result<Option<ExitStatus>> {
        #[cfg(unix)]
        {
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
                return Err(std::io::Error::last_os_error());
            }
            if unsafe { information.si_pid() } == 0 {
                return Ok(None);
            }
            crate::native::control::stop_group(self.child.id()).map_err(std::io::Error::other)?;
        }
        let status = self.child.try_wait()?;
        #[cfg(windows)]
        if status.is_some() {
            self.job.terminate().map_err(std::io::Error::other)?;
        }
        self.exited |= status.is_some();
        Ok(status)
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !self.exited {
            #[cfg(windows)]
            let _ = self.job.terminate();
            #[cfg(unix)]
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
            let _ = self.child.kill();
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(2) {
                if self.poll().ok().flatten().is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

fn capture_file(directory: &Path, suffix: &str) -> Result<(PathBuf, File)> {
    fs::create_dir_all(directory)?;
    let sequence = NEXT_OUTPUT.fetch_add(1, Ordering::Relaxed);
    let path = directory.join(format!("tool-{}-{sequence}.{suffix}", std::process::id()));
    let mut options = OpenOptions::new();
    options.create_new(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path)?;
    Ok((path, file))
}

pub fn run(
    command: &mut Command,
    directory: &Path,
    timeout: Duration,
    output_limit: u64,
) -> Result<Output> {
    if timeout.is_zero() || output_limit == 0 {
        return Err(Error::invalid(
            "tool timeout and output limit must be positive",
        ));
    }
    let (out_path, mut out) = capture_file(directory, "stdout")?;
    let (err_path, mut err) = match capture_file(directory, "stderr") {
        Ok(value) => value,
        Err(error) => {
            let _ = fs::remove_file(&out_path);
            return Err(error);
        }
    };
    let result = (|| {
        command
            .stdin(Stdio::null())
            .stdout(out.try_clone()?)
            .stderr(err.try_clone()?);
        command
            .env("TMPDIR", directory)
            .env("TEMP", directory)
            .env("TMP", directory);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
        }
        #[allow(unused_mut)]
        let mut child = command.spawn().map_err(|error| {
            Error::new(
                if error.kind() == std::io::ErrorKind::NotFound {
                    ErrorKind::MissingTool
                } else {
                    ErrorKind::Io
                },
                error.to_string(),
            )
        })?;
        #[cfg(windows)]
        let job = match crate::native::control::WindowsJob::assign_and_resume(&child) {
            Ok(job) => job,
            Err(error) => {
                let _ = child.kill();
                return Err(error);
            }
        };
        let mut child = ChildGuard {
            child,
            exited: false,
            #[cfg(windows)]
            job,
        };
        let started = Instant::now();
        let status = loop {
            let too_large =
                out.metadata()?.len().saturating_add(err.metadata()?.len()) > output_limit;
            if too_large || started.elapsed() >= timeout {
                return Err(Error::new(
                    if too_large {
                        ErrorKind::LimitExceeded
                    } else {
                        ErrorKind::Timeout
                    },
                    "tool exceeded its output or time budget",
                ));
            }
            if let Some(status) = child.poll()? {
                break status;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        if out.metadata()?.len().saturating_add(err.metadata()?.len()) > output_limit {
            return Err(Error::new(
                ErrorKind::LimitExceeded,
                "tool output exceeds budget",
            ));
        }
        out.seek(SeekFrom::Start(0))?;
        err.seek(SeekFrom::Start(0))?;
        let mut stdout = String::new();
        let mut stderr = String::new();
        out.take(output_limit + 1).read_to_string(&mut stdout)?;
        err.take(output_limit + 1).read_to_string(&mut stderr)?;
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    })();
    let _ = fs::remove_file(out_path);
    let _ = fs::remove_file(err_path);
    result
}

pub fn require_success(output: Output) -> Result<Output> {
    if output.status.success() {
        Ok(output)
    } else {
        Err(Error::new(
            ErrorKind::CollectorFailure,
            format!(
                "tool exited {}\nstdout: {}\nstderr: {}",
                output.status, output.stdout, output.stderr
            ),
        ))
    }
}
