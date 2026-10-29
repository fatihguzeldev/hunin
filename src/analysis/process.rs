use anyhow::{Context, Result, bail};
use std::{
    path::Path,
    process::{ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{mpsc, watch},
    time::Instant,
};

const OUTPUT_LIMIT: u64 = 16 * 1024 * 1024;

pub(super) use crate::host::tool_path;

#[derive(Clone)]
pub struct Control {
    pub cancellation: watch::Receiver<bool>,
    pub deadline: Instant,
    bytes: Arc<AtomicU64>,
}
impl Control {
    pub fn new(cancellation: watch::Receiver<bool>, deadline: Instant) -> Self {
        Self {
            cancellation,
            deadline,
            bytes: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn check(&self) -> Result<()> {
        if *self.cancellation.borrow() {
            bail!("analysis cancelled");
        }
        check_deadline(self.deadline)
    }
}

pub(super) fn check_deadline(deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline {
        bail!("analysis timed out");
    }
    Ok(())
}
async fn copy_bounded<R: AsyncRead + Unpin>(
    mut input: R,
    destination: std::path::PathBuf,
    bytes: Arc<AtomicU64>,
    signal: mpsc::Sender<String>,
) -> Result<()> {
    let result = async {
        let mut output = tokio::fs::File::create(destination).await?;
        let mut buffer = [0u8; 8192];
        loop {
            let count = input.read(&mut buffer).await?;
            if count == 0 {
                output.flush().await?;
                return Ok(());
            }
            let previous = bytes.fetch_add(count as u64, Ordering::Relaxed);
            let allowed = OUTPUT_LIMIT.saturating_sub(previous).min(count as u64) as usize;
            output.write_all(&buffer[..allowed]).await?;
            if allowed < count {
                output.flush().await?;
                bail!("analyzer stdout/stderr exceeded the combined 16 MiB output limit");
            }
        }
    }
    .await;
    if let Err(error) = &result {
        let _ = signal.send(format!("{error:#}")).await;
    }
    result
}
pub async fn execute(
    arguments: &[String],
    cwd: &Path,
    input: Option<&Path>,
    stdout: &Path,
    stderr: &Path,
    mut control: Control,
) -> Result<ExitStatus> {
    control.check()?;
    let mut command = Command::new(tool_path(Path::new(&arguments[0])));
    command
        .args(&arguments[1..])
        .current_dir(tool_path(cwd))
        .kill_on_drop(true)
        .stdin(match input {
            Some(path) => Stdio::from(std::fs::File::open(path)?),
            None => Stdio::null(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.env_remove("HUNIN_ANALYSIS_INPUT");
    if let Some(input) = input {
        command.env("HUNIN_ANALYSIS_INPUT", input);
    }
    command
        .env_remove("HUNIN_RUN_ID")
        .env_remove("HUNIN_RECORD_DIR");
    let (mut child, mut tree) = crate::execution::process::spawn(&mut command).await?;
    let total = control.bytes.clone();
    let (sender, mut receiver) = mpsc::channel(2);
    let _signal_guard = sender.clone();
    let stdout_reader = tokio::spawn(copy_bounded(
        child.stdout.take().context("analyzer stdout missing")?,
        stdout.to_path_buf(),
        total.clone(),
        sender.clone(),
    ));
    let stderr_reader = tokio::spawn(copy_bounded(
        child.stderr.take().context("analyzer stderr missing")?,
        stderr.to_path_buf(),
        total,
        sender,
    ));
    let outcome: Result<ExitStatus> = tokio::select! {
        status = child.wait() => Ok(status?),
        _ = control.cancellation.changed() => { let _ = tree.stop(&mut child, false).await; Err(anyhow::anyhow!("analysis cancelled")) },
        _ = tokio::time::sleep_until(control.deadline) => { let _ = tree.stop(&mut child, false).await; Err(anyhow::anyhow!("analysis timed out")) },
        message = receiver.recv() => { let _ = tree.stop(&mut child, false).await; Err(anyhow::anyhow!(message.unwrap_or_else(|| "analyzer output capture failed".into()))) },
    };
    tree.force();
    let a = stdout_reader.await?;
    let b = stderr_reader.await?;
    let status = outcome?;
    a?;
    b?;
    control.check()?;
    Ok(status)
}
