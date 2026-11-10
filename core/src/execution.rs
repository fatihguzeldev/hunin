use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct PreparedLaunch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub directory: PathBuf,
    pub environment: BTreeMap<String, String>,
    pub stdout_limit: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionPhase {
    AwaitingReadiness,
    Ready,
    Running,
    Exited,
    Cancelled,
}

#[derive(Debug)]
pub struct ExecutionOutcome {
    pub exit_code: Option<i32>,
    pub success: bool,
    pub cancelled: bool,
    pub elapsed: Duration,
    pub measured_elapsed: Option<Duration>,
    pub stdout: Vec<u8>,
    pub stdout_truncated: bool,
    pub cleanup_pending: bool,
}

#[derive(Debug)]
pub enum ExecutionError {
    Invalid(&'static str),
    Io(io::Error),
    Readiness(String),
    Deadline(&'static str),
}

impl std::fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(s) | Self::Deadline(s) => f.write_str(s),
            Self::Io(e) => e.fmt(f),
            Self::Readiness(e) => write!(f, "source readiness failed: {e}"),
        }
    }
}

impl std::error::Error for ExecutionError {}
impl From<io::Error> for ExecutionError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub struct PreparedProcess {
    child: Option<Child>,
    input: Option<ChildStdin>,
    ready: Receiver<Result<(), String>>,
    output: Receiver<(Vec<u8>, bool)>,
    phase: ExecutionPhase,
    started: Instant,
    released: Option<Instant>,
}

impl PreparedProcess {
    // Ownership is this direct child only; descendants are never selected by pid matching.
    pub fn spawn(spec: &PreparedLaunch) -> Result<Self, ExecutionError> {
        if !spec.program.is_absolute()
            || !spec.directory.is_absolute()
            || spec.stdout_limit > 16 * 1024 * 1024
        {
            return Err(ExecutionError::Invalid(
                "launch requires absolute paths and a stdout bound at most 16 mib",
            ));
        }
        let started = Instant::now();
        let mut child = Command::new(&spec.program)
            .args(&spec.args)
            .current_dir(&spec.directory)
            .envs(&spec.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let input = child.stdin.take();
        let mut stdout = child
            .stdout
            .take()
            .ok_or(ExecutionError::Invalid("missing stdout pipe"))?;
        let (ready_tx, ready) = mpsc::sync_channel(1);
        let (output_tx, output) = mpsc::sync_channel(1);
        let limit = spec.stdout_limit;
        std::thread::spawn(move || {
            let mut first = Vec::with_capacity(32);
            let mut byte = [0];
            let readiness = loop {
                match stdout.read(&mut byte) {
                    Ok(0) => break Err("stdout closed before readiness".to_owned()),
                    Ok(_) if byte[0] == b'\n' => {
                        if first.last() == Some(&b'\r') {
                            first.pop();
                        }
                        break if first == b"HUNIN_READY" {
                            Ok(())
                        } else {
                            Err("invalid readiness line".to_owned())
                        };
                    }
                    Ok(_) if first.len() < 1024 => first.push(byte[0]),
                    Ok(_) => break Err("readiness line exceeds 1024 bytes".to_owned()),
                    Err(e) => break Err(e.to_string()),
                }
            };
            let accepted = readiness.is_ok();
            let _ = ready_tx.send(readiness);
            if !accepted {
                return;
            }
            let mut bytes = Vec::new();
            let mut truncated = false;
            let mut buffer = [0; 8192];
            loop {
                match stdout.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        let remaining = limit.saturating_sub(bytes.len());
                        bytes.extend_from_slice(&buffer[..n.min(remaining)]);
                        truncated |= n > remaining;
                    }
                    Err(_) => {
                        truncated = true;
                        break;
                    }
                }
            }
            let _ = output_tx.send((bytes, truncated));
        });
        Ok(Self {
            child: Some(child),
            input,
            ready,
            output,
            phase: ExecutionPhase::AwaitingReadiness,
            started,
            released: None,
        })
    }

    pub fn id(&self) -> u32 {
        self.child.as_ref().expect("owned child").id()
    }
    pub fn phase(&self) -> ExecutionPhase {
        self.phase
    }

    pub fn await_ready(&mut self, deadline: Duration) -> Result<(), ExecutionError> {
        if self.phase != ExecutionPhase::AwaitingReadiness {
            return Err(ExecutionError::Invalid(
                "readiness is only accepted before execution",
            ));
        }
        match self.ready.recv_timeout(deadline) {
            Ok(Ok(())) => {
                self.phase = ExecutionPhase::Ready;
                Ok(())
            }
            Ok(Err(e)) => Err(ExecutionError::Readiness(e)),
            Err(_) => Err(ExecutionError::Deadline(
                "source readiness deadline exceeded",
            )),
        }
    }

    pub fn release(&mut self) -> Result<(), ExecutionError> {
        if self.phase != ExecutionPhase::Ready {
            return Err(ExecutionError::Invalid(
                "work requires acknowledged source readiness",
            ));
        }
        self.input
            .as_mut()
            .ok_or(ExecutionError::Invalid("closed input"))?
            .write_all(b"HUNIN_GO\n")?;
        self.released = Some(Instant::now());
        self.phase = ExecutionPhase::Running;
        Ok(())
    }

    fn poll_exit(&mut self, deadline: Duration) -> Result<Option<ExitStatus>, ExecutionError> {
        let until = Instant::now()
            .checked_add(deadline)
            .ok_or(ExecutionError::Invalid("invalid deadline"))?;
        loop {
            if let Some(status) = self.child.as_mut().expect("owned child").try_wait()? {
                return Ok(Some(status));
            }
            if Instant::now() >= until {
                return Ok(None);
            }
            std::thread::sleep(
                Duration::from_millis(2).min(until.saturating_duration_since(Instant::now())),
            );
        }
    }

    fn outcome(&mut self, status: Option<ExitStatus>, cancelled: bool) -> ExecutionOutcome {
        let elapsed = self.started.elapsed();
        let measured_elapsed = self.released.map(|t| t.elapsed());
        // A descendant may retain stdout; never wait indefinitely for that borrowed descriptor.
        let output = self.output.recv_timeout(Duration::from_millis(100));
        let output_pending = output.is_err();
        let (stdout, stdout_truncated) = output.unwrap_or((Vec::new(), true));
        self.phase = if cancelled {
            ExecutionPhase::Cancelled
        } else {
            ExecutionPhase::Exited
        };
        ExecutionOutcome {
            exit_code: status.and_then(|s| s.code()),
            success: status.is_some_and(|s| s.success()),
            cancelled,
            elapsed,
            measured_elapsed,
            stdout,
            stdout_truncated,
            cleanup_pending: status.is_none() || output_pending,
        }
    }

    pub fn wait(&mut self, deadline: Duration) -> Result<ExecutionOutcome, ExecutionError> {
        if !matches!(self.phase, ExecutionPhase::Running | ExecutionPhase::Ready) {
            return Err(ExecutionError::Invalid("execution is not active"));
        }
        let status = self.poll_exit(deadline)?.ok_or(ExecutionError::Deadline(
            "application deadline exceeded; owned child remains active",
        ))?;
        Ok(self.outcome(Some(status), false))
    }

    pub fn cancel(
        &mut self,
        grace: Duration,
        forced: Duration,
    ) -> Result<ExecutionOutcome, ExecutionError> {
        if matches!(
            self.phase,
            ExecutionPhase::Cancelled | ExecutionPhase::Exited
        ) {
            return Err(ExecutionError::Invalid("execution is already terminal"));
        }
        if let Some(mut input) = self.input.take() {
            let _ = input.write_all(b"HUNIN_STOP\n");
        }
        let mut status = self.poll_exit(grace)?;
        if status.is_none() {
            self.child.as_mut().expect("owned child").kill()?;
            status = self.poll_exit(forced)?;
        }
        Ok(self.outcome(status, true))
    }
}

impl Drop for PreparedProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn fixture(script: &str, limit: usize) -> PreparedProcess {
        PreparedProcess::spawn(&PreparedLaunch {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            directory: "/".into(),
            environment: BTreeMap::new(),
            stdout_limit: limit,
        })
        .unwrap()
    }

    #[test]
    fn gate_prevents_work_before_source_readiness_and_bounds_output() {
        let mut p = fixture(
            "printf 'HUNIN_READY\\n'; read gate; test \"$gate\" = HUNIN_GO || exit 17; printf '123456789'",
            4,
        );
        assert!(p.release().is_err());
        p.await_ready(Duration::from_secs(2)).unwrap();
        p.release().unwrap();
        let result = p.wait(Duration::from_secs(2)).unwrap();
        assert!(result.success);
        assert_eq!(result.stdout, b"1234");
        assert!(result.stdout_truncated);
        assert!(!result.cleanup_pending);
        assert!(result.measured_elapsed.is_some());
    }

    #[test]
    fn terminal_cancel_fences_release_and_kills_only_owned_child() {
        let mut p = fixture(
            "printf 'HUNIN_READY\\n'; read gate; test \"$gate\" = HUNIN_STOP",
            16,
        );
        p.await_ready(Duration::from_secs(2)).unwrap();
        let outcome = p
            .cancel(Duration::from_secs(1), Duration::from_secs(1))
            .unwrap();
        assert!(outcome.cancelled);
        assert!(!outcome.cleanup_pending);
        assert!(p.release().is_err());
        assert_eq!(p.phase(), ExecutionPhase::Cancelled);
    }

    #[test]
    fn readiness_requires_actual_ack_and_excessive_lines_fail() {
        let mut p = fixture("printf 'other\\n'; read gate", 16);
        assert!(matches!(
            p.await_ready(Duration::from_secs(1)),
            Err(ExecutionError::Readiness(_))
        ));
        p.cancel(Duration::ZERO, Duration::from_secs(1)).unwrap();
        let mut p = fixture("while :; do printf x; done", 16);
        assert!(matches!(
            p.await_ready(Duration::from_secs(1)),
            Err(ExecutionError::Readiness(_))
        ));
        p.cancel(Duration::ZERO, Duration::from_secs(1)).unwrap();
    }
}
