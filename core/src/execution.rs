use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
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

pub struct ControlProtocol {
    token: Option<String>,
}

impl ControlProtocol {
    /// Built-in helpers opt in; an unmarked launch explicitly keeps the legacy line protocol.
    pub fn prepare(environment: &mut BTreeMap<String, String>) -> Result<Self, ExecutionError> {
        for key in environment.keys() {
            if key.eq_ignore_ascii_case("HUNIN_CONTROL_TOKEN") {
                return Err(ExecutionError::Invalid(
                    "control token is owned by the launch factory",
                ));
            }
            if key.eq_ignore_ascii_case("HUNIN_CONTROL_PROTOCOL") && key != "HUNIN_CONTROL_PROTOCOL"
            {
                return Err(ExecutionError::Invalid(
                    "control protocol environment aliases are reserved",
                ));
            }
        }
        match environment
            .get("HUNIN_CONTROL_PROTOCOL")
            .map(String::as_str)
        {
            None => Ok(Self { token: None }),
            Some("nonce-lines-v1") => {
                let mut bytes = [0; 16];
                getrandom::fill(&mut bytes)
                    .map_err(|error| ExecutionError::Io(io::Error::other(error.to_string())))?;
                let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
                environment.insert("HUNIN_CONTROL_TOKEN".into(), token.clone());
                Ok(Self { token: Some(token) })
            }
            Some(_) => Err(ExecutionError::Invalid(
                "unsupported prepared control protocol",
            )),
        }
    }

    fn marker(&self, name: &str) -> Vec<u8> {
        match &self.token {
            Some(token) => format!("{name} {token}").into_bytes(),
            None => name.as_bytes().to_vec(),
        }
    }
}

fn retain_output(bytes: &mut Vec<u8>, truncated: &mut bool, input: &[u8], limit: usize) {
    let remaining = limit.saturating_sub(bytes.len());
    bytes.extend_from_slice(&input[..input.len().min(remaining)]);
    *truncated |= input.len() > remaining;
}

fn nonce_stdout(
    mut stdout: ChildStdout,
    limit: usize,
    protocol: ControlProtocol,
    ready: mpsc::SyncSender<Result<(), String>>,
    completion: mpsc::SyncSender<Result<Instant, String>>,
    output: mpsc::SyncSender<(Vec<u8>, bool)>,
) {
    let terminated = |marker: Vec<u8>| {
        let mut lf = marker.clone();
        lf.push(b'\n');
        let mut crlf = marker;
        crlf.extend_from_slice(b"\r\n");
        [lf, crlf]
    };
    let ready_markers = terminated(protocol.marker("HUNIN_READY"));
    let completion_markers = terminated(protocol.marker("HUNIN_WORK_COMPLETE"));
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut pending = Vec::new();
    let mut acknowledged = false;
    let mut completed = false;
    let mut buffer = [0; 8192];
    let failure = loop {
        match stdout.read(&mut buffer) {
            Ok(0) => break "stdout closed".to_owned(),
            Ok(count) => {
                let mut offset = 0;
                while offset < count {
                    if completed {
                        retain_output(&mut bytes, &mut truncated, &buffer[offset..count], limit);
                        break;
                    }
                    let markers = if acknowledged {
                        &completion_markers
                    } else {
                        &ready_markers
                    };
                    if pending.is_empty() {
                        let ordinary = buffer[offset..count]
                            .iter()
                            .position(|byte| *byte == markers[0][0])
                            .unwrap_or(count - offset);
                        if ordinary != 0 {
                            retain_output(
                                &mut bytes,
                                &mut truncated,
                                &buffer[offset..offset + ordinary],
                                limit,
                            );
                            offset += ordinary;
                            continue;
                        }
                    }
                    pending.push(buffer[offset]);
                    offset += 1;
                    // Only a marker prefix stays pending; ordinary output needs no newline.
                    while !markers.iter().any(|marker| marker.starts_with(&pending)) {
                        retain_output(&mut bytes, &mut truncated, &pending[..1], limit);
                        pending.remove(0);
                    }
                    if markers.iter().any(|marker| marker == &pending) {
                        pending.clear();
                        if acknowledged {
                            let _ = completion.send(Ok(Instant::now()));
                            completed = true;
                        } else {
                            let _ = ready.send(Ok(()));
                            acknowledged = true;
                        }
                    }
                }
            }
            Err(error) => {
                truncated = true;
                break error.to_string();
            }
        }
    };
    retain_output(&mut bytes, &mut truncated, &pending, limit);
    if !acknowledged {
        let diagnostic = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
        let _ = ready.send(Err(format!(
            "{failure} before readiness; startup stdout: {diagnostic}"
        )));
    } else if !completed {
        let _ = completion.send(Err(format!(
            "{failure} before work completion acknowledgment"
        )));
    }
    let _ = output.send((bytes, truncated));
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionPhase {
    AwaitingReadiness,
    Ready,
    Running,
    Exited,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionBoundary {
    CompletionAcknowledgment,
    ProcessExit,
}

#[derive(Clone, Debug)]
pub struct ExecutionOutcome {
    pub exit_code: Option<i32>,
    pub success: bool,
    pub cancelled: bool,
    pub elapsed: Duration,
    pub measured_elapsed: Option<Duration>,
    pub measurement_end: Option<ExecutionBoundary>,
    pub stdout: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr: Vec<u8>,
    pub stderr_truncated: bool,
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

pub trait ExecutionChild: Send {
    fn id(&self) -> u32;
    fn take_stdin(&mut self) -> Option<ChildStdin>;
    fn take_stdout(&mut self) -> Option<ChildStdout>;
    fn take_stderr(&mut self) -> Option<ChildStderr>;
    fn poll(&mut self) -> io::Result<Option<ExitStatus>>;
    fn terminate(&mut self) -> io::Result<()>;
    fn reap(&mut self) -> io::Result<ExitStatus>;
}

struct DirectChild(Child);

impl ExecutionChild for DirectChild {
    fn id(&self) -> u32 {
        self.0.id()
    }
    fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.0.stdin.take()
    }
    fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.0.stdout.take()
    }
    fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.0.stderr.take()
    }
    fn poll(&mut self) -> io::Result<Option<ExitStatus>> {
        self.0.try_wait()
    }
    fn terminate(&mut self) -> io::Result<()> {
        self.0.kill()
    }
    fn reap(&mut self) -> io::Result<ExitStatus> {
        self.0.wait()
    }
}

pub struct PreparedProcess {
    child: Option<Box<dyn ExecutionChild>>,
    input: Option<ChildStdin>,
    ready: Receiver<Result<(), String>>,
    output: Receiver<(Vec<u8>, bool)>,
    errors: Receiver<(Vec<u8>, bool)>,
    captured_errors: Option<(Vec<u8>, bool)>,
    terminal_outcome: Option<ExecutionOutcome>,
    output_done: bool,
    errors_done: bool,
    completion: Receiver<Result<Instant, String>>,
    completed_at: Option<Instant>,
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
        let mut environment = spec.environment.clone();
        let protocol = ControlProtocol::prepare(&mut environment)?;
        let child = Command::new(&spec.program)
            .args(&spec.args)
            .current_dir(&spec.directory)
            .env_remove("HUNIN_CONTROL_PROTOCOL")
            .env_remove("HUNIN_CONTROL_TOKEN")
            .envs(&environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut process = Self::from_child_with_protocol(
            Box::new(DirectChild(child)),
            spec.stdout_limit,
            protocol,
        )?;
        process.started = started;
        Ok(process)
    }

    /// Adopts a child using the legacy first-line readiness and plain completion protocol.
    pub fn from_child(
        child: Box<dyn ExecutionChild>,
        limit: usize,
    ) -> Result<Self, ExecutionError> {
        Self::from_child_with_protocol(child, limit, ControlProtocol { token: None })
    }

    /// The protocol must be prepared before starting the child with its resulting environment.
    pub fn from_child_with_protocol(
        mut child: Box<dyn ExecutionChild>,
        limit: usize,
        protocol: ControlProtocol,
    ) -> Result<Self, ExecutionError> {
        let input = child.take_stdin();
        let stdout = child.take_stdout();
        if limit > 16 * 1024 * 1024 || input.is_none() || stdout.is_none() {
            let _ = child.terminate();
            std::thread::spawn(move || {
                let _ = child.reap();
            });
            return Err(ExecutionError::Invalid(
                "prepared child needs bounded stdin/stdout pipes",
            ));
        }
        let mut stdout = stdout.expect("checked stdout");
        let (errors_tx, errors) = mpsc::sync_channel(1);
        if let Some(mut stderr) = child.take_stderr() {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let mut buffer = [0; 8192];
                let mut truncated = false;
                loop {
                    match stderr.read(&mut buffer) {
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
                let _ = errors_tx.send((bytes, truncated));
            });
        } else {
            let _ = errors_tx.send((Vec::new(), false));
        }
        let (ready_tx, ready) = mpsc::sync_channel(1);
        let (output_tx, output) = mpsc::sync_channel(1);
        let (completion_tx, completion) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            if protocol.token.is_some() {
                nonce_stdout(stdout, limit, protocol, ready_tx, completion_tx, output_tx);
                return;
            }
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
            let mut line = Vec::new();
            let mut long_line = false;
            let mut completed = false;
            loop {
                match stdout.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        for byte in &buffer[..n] {
                            if *byte == b'\n' {
                                if line.last() == Some(&b'\r') {
                                    line.pop();
                                }
                                if !long_line && !completed && line == b"HUNIN_WORK_COMPLETE" {
                                    let _ = completion_tx.send(Ok(Instant::now()));
                                    completed = true;
                                }
                                line.clear();
                                long_line = false;
                            } else if line.len() < 1024 {
                                line.push(*byte);
                            } else {
                                long_line = true;
                            }
                        }
                        retain_output(&mut bytes, &mut truncated, &buffer[..n], limit);
                    }
                    Err(_) => {
                        truncated = true;
                        break;
                    }
                }
            }
            if !completed {
                let _ = completion_tx.send(Err(
                    "stdout closed before work completion acknowledgment".into(),
                ));
            }
            let _ = output_tx.send((bytes, truncated));
        });
        Ok(Self {
            child: Some(child),
            input,
            ready,
            output,
            errors,
            captured_errors: None,
            terminal_outcome: None,
            output_done: false,
            errors_done: false,
            completion,
            completed_at: None,
            phase: ExecutionPhase::AwaitingReadiness,
            started: Instant::now(),
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
            Ok(Err(e)) => {
                self.captured_errors = self.errors.recv_timeout(Duration::from_millis(100)).ok();
                let diagnostic = self
                    .captured_errors
                    .as_ref()
                    .map(|(bytes, _)| String::from_utf8_lossy(bytes).into_owned())
                    .unwrap_or_default();
                Err(ExecutionError::Readiness(format!("{e}; {diagnostic}")))
            }
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
        let released = Instant::now();
        self.input
            .as_mut()
            .ok_or(ExecutionError::Invalid("closed input"))?
            .write_all(b"HUNIN_GO\n")?;
        self.released = Some(released);
        self.phase = ExecutionPhase::Running;
        Ok(())
    }

    pub fn work_complete(&mut self) -> Result<bool, ExecutionError> {
        if self.completed_at.is_some() {
            return Ok(true);
        }
        match self.completion.try_recv() {
            Ok(Ok(completed_at)) => {
                if self.released.is_none_or(|released| completed_at < released) {
                    return Err(ExecutionError::Readiness(
                        "work completion preceded work release".into(),
                    ));
                }
                self.completed_at = Some(completed_at);
                Ok(true)
            }
            Ok(Err(error)) => Err(ExecutionError::Readiness(error)),
            Err(mpsc::TryRecvError::Empty) => Ok(false),
            Err(mpsc::TryRecvError::Disconnected) => Err(ExecutionError::Readiness(
                "work completion channel closed".into(),
            )),
        }
    }

    pub fn acknowledged_elapsed(&self) -> Option<Duration> {
        self.completed_at
            .zip(self.released)
            .map(|(end, start)| end.saturating_duration_since(start))
    }

    pub fn finish(&mut self) -> Result<(), ExecutionError> {
        if self.phase != ExecutionPhase::Running || self.completed_at.is_none() {
            return Err(ExecutionError::Invalid(
                "final exit requires work completion acknowledgment",
            ));
        }
        self.input
            .take()
            .ok_or(ExecutionError::Invalid("closed input"))?
            .write_all(b"HUNIN_EXIT\n")?;
        Ok(())
    }

    fn poll_exit(&mut self, deadline: Duration) -> Result<Option<ExitStatus>, ExecutionError> {
        let until = Instant::now()
            .checked_add(deadline)
            .ok_or(ExecutionError::Invalid("invalid deadline"))?;
        loop {
            if let Some(status) = self.child.as_mut().expect("owned child").poll()? {
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
        let _ = self.work_complete();
        let measured_elapsed = self.released.map(|released| {
            self.completed_at
                .unwrap_or_else(Instant::now)
                .saturating_duration_since(released)
        });
        let measurement_end = self.released.map(|_| {
            if self.completed_at.is_some() {
                ExecutionBoundary::CompletionAcknowledgment
            } else {
                ExecutionBoundary::ProcessExit
            }
        });
        // A descendant may retain stdout; never wait indefinitely for that borrowed descriptor.
        let output = self.output.recv_timeout(Duration::from_millis(100));
        let output_pending = matches!(output, Err(mpsc::RecvTimeoutError::Timeout));
        self.output_done = !output_pending;
        let (stdout, stdout_truncated) = output.unwrap_or((Vec::new(), true));
        let errors = self
            .captured_errors
            .take()
            .map(Ok)
            .unwrap_or_else(|| self.errors.recv_timeout(Duration::from_millis(100)));
        let errors_pending = matches!(errors, Err(mpsc::RecvTimeoutError::Timeout));
        self.errors_done = !errors_pending;
        let (stderr, stderr_truncated) = errors.unwrap_or((Vec::new(), true));
        self.phase = if cancelled {
            ExecutionPhase::Cancelled
        } else {
            ExecutionPhase::Exited
        };
        let outcome = ExecutionOutcome {
            exit_code: status.and_then(|s| s.code()),
            success: status.is_some_and(|s| s.success()),
            cancelled,
            elapsed,
            measured_elapsed,
            measurement_end,
            stdout,
            stdout_truncated,
            stderr,
            stderr_truncated,
            cleanup_pending: status.is_none() || output_pending || errors_pending,
        };
        self.terminal_outcome = Some(outcome.clone());
        outcome
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
        if let Some(outcome) = &self.terminal_outcome {
            return Ok(outcome.clone());
        }
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
            if let Err(error) = self.child.as_mut().expect("owned child").terminate()
                && self.child.as_mut().expect("owned child").poll()?.is_none()
            {
                return Err(error.into());
            }
            status = self.poll_exit(forced)?;
        }
        Ok(self.outcome(status, true))
    }

    pub fn confirm_cleanup(&mut self, deadline: Duration) -> Result<bool, ExecutionError> {
        let Some(mut outcome) = self.terminal_outcome.clone() else {
            return Err(ExecutionError::Invalid(
                "cleanup confirmation requires a terminal control outcome",
            ));
        };
        if !outcome.cleanup_pending {
            return Ok(true);
        }
        let until = Instant::now()
            .checked_add(deadline)
            .ok_or(ExecutionError::Invalid("invalid cleanup deadline"))?;
        let mut terminated = false;
        loop {
            let status = self.child.as_mut().expect("owned child").poll()?;
            if status.is_none() && !terminated {
                self.child.as_mut().expect("owned child").terminate()?;
                terminated = true;
            }
            if !self.output_done {
                match self.output.try_recv() {
                    Ok((bytes, truncated)) => {
                        outcome.stdout = bytes;
                        outcome.stdout_truncated = truncated;
                        self.output_done = true;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => self.output_done = true,
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
            if !self.errors_done {
                match self.errors.try_recv() {
                    Ok((bytes, truncated)) => {
                        outcome.stderr = bytes;
                        outcome.stderr_truncated = truncated;
                        self.errors_done = true;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => self.errors_done = true,
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
            if let Some(status) = status {
                outcome.exit_code = status.code();
                outcome.success = status.success();
            }
            outcome.cleanup_pending = status.is_none() || !self.output_done || !self.errors_done;
            self.terminal_outcome = Some(outcome.clone());
            if !outcome.cleanup_pending {
                return Ok(true);
            }
            if Instant::now() >= until {
                return Ok(false);
            }
            std::thread::sleep(
                Duration::from_millis(2).min(until.saturating_duration_since(Instant::now())),
            );
        }
    }
}

impl Drop for PreparedProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take()
            && child.poll().ok().flatten().is_none()
        {
            let _ = child.terminate();
            std::thread::spawn(move || {
                let _ = child.reap();
            });
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

    #[test]
    fn final_gate_keeps_target_alive_until_observer_finishes_snapshot() {
        let mut p = fixture(
            "printf 'HUNIN_READY\\n'; read gate; test \"$gate\" = HUNIN_GO || exit 17; printf 'HUNIN_WORK_COMPLETE\\n'; read gate; test \"$gate\" = HUNIN_EXIT",
            128,
        );
        p.await_ready(Duration::from_secs(2)).unwrap();
        p.release().unwrap();
        assert!(p.finish().is_err());
        let until = Instant::now() + Duration::from_secs(2);
        while !p.work_complete().unwrap() {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(p.child.as_mut().unwrap().poll().unwrap().is_none());
        let frozen = p.acknowledged_elapsed().unwrap();
        std::thread::sleep(Duration::from_millis(120));
        p.finish().unwrap();
        let outcome = p.wait(Duration::from_secs(2)).unwrap();
        assert!(outcome.success);
        assert_eq!(outcome.measured_elapsed, Some(frozen));
        assert_eq!(
            outcome.measurement_end,
            Some(ExecutionBoundary::CompletionAcknowledgment)
        );
        assert!(outcome.elapsed.saturating_sub(frozen) >= Duration::from_millis(100));
    }

    #[test]
    fn prerequisite_diagnostics_survive_readiness_failure_and_stderr_is_bounded() {
        let mut p = fixture("printf 'missing profiler permission' >&2; exit 9", 64);
        assert!(
            p.await_ready(Duration::from_secs(2))
                .unwrap_err()
                .to_string()
                .contains("missing profiler permission")
        );
        let mut p = fixture(
            "printf 'HUNIN_READY\\n'; read gate; printf '123456789' >&2",
            4,
        );
        p.await_ready(Duration::from_secs(2)).unwrap();
        p.release().unwrap();
        let outcome = p.wait(Duration::from_secs(2)).unwrap();
        assert_eq!(outcome.stderr, b"1234");
        assert!(outcome.stderr_truncated);
        assert!(!outcome.cleanup_pending);
    }

    #[test]
    fn late_descriptor_cleanup_is_bounded_and_can_be_confirmed_without_redispatch() {
        let mut p = fixture("printf 'HUNIN_READY\\n'; read gate; sleep 0.6 & exit 0", 64);
        assert!(p.confirm_cleanup(Duration::ZERO).is_err());
        p.await_ready(Duration::from_secs(2)).unwrap();
        p.release().unwrap();
        let outcome = p.wait(Duration::from_secs(1)).unwrap();
        assert!(outcome.success);
        assert!(outcome.cleanup_pending);
        let start = Instant::now();
        assert!(!p.confirm_cleanup(Duration::from_millis(10)).unwrap());
        assert!(start.elapsed() < Duration::from_millis(250));
        assert!(
            p.cancel(Duration::ZERO, Duration::ZERO)
                .unwrap()
                .cleanup_pending
        );
        assert!(p.confirm_cleanup(Duration::from_secs(2)).unwrap());
        assert!(
            !p.cancel(Duration::ZERO, Duration::ZERO)
                .unwrap()
                .cleanup_pending
        );
    }

    #[test]
    fn nonce_control_keeps_startup_logs_and_ignores_plain_workload_completion() {
        let mut p = PreparedProcess::spawn(&PreparedLaunch {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "printf 'preload log\\nHUNIN_READY\\npartial preload'; printf 'HUNIN_READY %s\\n' \"$HUNIN_CONTROL_TOKEN\"; read gate; test \"$gate\" = HUNIN_GO || exit 17; printf 'progressHUNIN_WORK_COMPLETE\\nHUNIN_WORK_COMPLETE wrong-token\\npartial work'; sleep 0.16; printf 'HUNIN_WORK_COMPLETE %s\\r\\n' \"$HUNIN_CONTROL_TOKEN\"; read gate; test \"$gate\" = HUNIN_EXIT".into()],
            directory: "/".into(),
            environment: BTreeMap::from([("HUNIN_CONTROL_PROTOCOL".into(), "nonce-lines-v1".into())]),
            stdout_limit: 1024,
        }).unwrap();
        p.await_ready(Duration::from_secs(2)).unwrap();
        p.release().unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        while !p.work_complete().unwrap() {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(p.acknowledged_elapsed().unwrap() >= Duration::from_millis(130));
        p.finish().unwrap();
        let outcome = p.wait(Duration::from_secs(2)).unwrap();
        assert!(outcome.success);
        assert_eq!(outcome.stdout,b"preload log\nHUNIN_READY\npartial preloadprogressHUNIN_WORK_COMPLETE\nHUNIN_WORK_COMPLETE wrong-token\npartial work");
    }

    #[test]
    fn nonce_readiness_has_bounded_output_diagnostics_and_a_deadline() {
        let make = |script: &str| {
            PreparedProcess::spawn(&PreparedLaunch {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), script.into()],
                directory: "/".into(),
                environment: BTreeMap::from([(
                    "HUNIN_CONTROL_PROTOCOL".into(),
                    "nonce-lines-v1".into(),
                )]),
                stdout_limit: 16,
            })
            .unwrap()
        };
        let mut p = make(
            "printf '%03000d' 0; printf 'HUNIN_READY %s\\r\\n' \"$HUNIN_CONTROL_TOKEN\"; read gate",
        );
        p.await_ready(Duration::from_secs(2)).unwrap();
        p.release().unwrap();
        let outcome = p.wait(Duration::from_secs(2)).unwrap();
        assert_eq!(outcome.stdout, b"0000000000000000");
        assert!(outcome.stdout_truncated);

        let mut p = make("printf 'HUNIN_READY\\nHUNIN_WOR'; exit 9");
        assert!(
            p.await_ready(Duration::from_secs(2))
                .unwrap_err()
                .to_string()
                .contains("startup stdout: HUNIN_READY")
        );
        let outcome = p.cancel(Duration::ZERO, Duration::from_secs(1)).unwrap();
        assert_eq!(outcome.stdout, b"HUNIN_READY\nHUNI");
        assert!(outcome.stdout_truncated);

        let mut p = make("printf 'HUNIN_READY\\n'; read gate");
        assert!(matches!(
            p.await_ready(Duration::from_millis(20)),
            Err(ExecutionError::Deadline(_))
        ));
        p.cancel(Duration::ZERO, Duration::from_secs(1)).unwrap();
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::*;

    #[test]
    fn launch_factories_create_fresh_tokens_and_reject_reserved_environment_aliases() {
        let prepare = || {
            let mut environment =
                BTreeMap::from([("HUNIN_CONTROL_PROTOCOL".into(), "nonce-lines-v1".into())]);
            ControlProtocol::prepare(&mut environment).unwrap();
            environment.remove("HUNIN_CONTROL_TOKEN").unwrap()
        };
        let token = prepare();
        assert_eq!(token.len(), 32);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(token, prepare());
        for key in [
            "HUNIN_CONTROL_TOKEN",
            "hunin_control_token",
            "Hunin_Control_Token",
            "hunin_control_protocol",
        ] {
            assert!(
                ControlProtocol::prepare(&mut BTreeMap::from([(
                    key.into(),
                    "caller value".into()
                )]))
                .is_err()
            );
        }
        assert!(
            ControlProtocol::prepare(&mut BTreeMap::from([(
                "HUNIN_CONTROL_PROTOCOL".into(),
                "unknown-version".into()
            )]))
            .is_err()
        );
        let mut legacy = BTreeMap::new();
        assert!(
            ControlProtocol::prepare(&mut legacy)
                .unwrap()
                .token
                .is_none()
        );
        assert!(!legacy.contains_key("HUNIN_CONTROL_TOKEN"));
    }
}
