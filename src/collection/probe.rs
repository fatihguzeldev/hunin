use anyhow::{Context, Result, bail};
use std::{
    io::{Read, Write},
    process::{Command, Output, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

/// Bound tool startup, inherited pipe writers, output volume and process ownership.
pub(crate) fn output(
    command: &mut Command,
    input: Option<Vec<u8>>,
    timeout: Duration,
    budget: usize,
) -> Result<Output> {
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (mut child, mut tree) = crate::execution::process::spawn_std(command)?;
    let started = Instant::now();
    let (sender, receiver) = mpsc::channel();
    for (kind, mut pipe) in [
        (
            0,
            Box::new(child.stdout.take().context("probe stdout missing")?) as Box<dyn Read + Send>,
        ),
        (
            1,
            Box::new(child.stderr.take().context("probe stderr missing")?) as Box<dyn Read + Send>,
        ),
    ] {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = pipe
                .by_ref()
                .take(budget as u64 + 1)
                .read_to_end(&mut bytes);
            let _ = sender.send((kind, result, bytes));
        });
    }
    drop(sender);
    if let Some(input) = input {
        let mut stdin = child.stdin.take().context("probe stdin missing")?;
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let mut streams = [None, None];
    let mut status = None;
    loop {
        while let Ok((kind, result, bytes)) = receiver.try_recv() {
            if let Err(error) = result {
                tree.force();
                let _ = child.kill();
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Err(error).context("read collector probe output");
            }
            if bytes.len() > budget {
                tree.force();
                let _ = child.kill();
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                bail!("collector probe output exceeds {budget} bytes per stream");
            }
            streams[kind] = Some(bytes);
        }
        if status.is_none() {
            status = child.try_wait()?;
            if status.is_some() {
                tree.force();
            }
        }
        if status.is_some() && streams.iter().all(Option::is_some) {
            break;
        }
        if started.elapsed() >= timeout {
            tree.force();
            let _ = child.kill();
            // Reaping runs independently: a stuck OS wait cannot stall discovery.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            bail!(
                "collector probe exceeded {:.3}s deadline",
                timeout.as_secs_f64()
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(Output {
        status: status.unwrap(),
        stdout: streams[0].take().unwrap(),
        stderr: streams[1].take().unwrap(),
    })
}

pub(crate) fn standard(command: &mut Command) -> Result<Output> {
    output(command, None, Duration::from_secs(10), 128 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_probe_stops_hanging_and_overproducing_owned_commands() {
        #[cfg(unix)]
        let mut hanging = {
            let mut c = Command::new("/bin/sh");
            c.args(["-c", "sleep 30"]);
            c
        };
        #[cfg(windows)]
        let mut hanging = {
            let mut c = Command::new("cmd.exe");
            c.args(["/C", "ping -n 31 127.0.0.1 >NUL"]);
            c
        };
        let began = Instant::now();
        assert!(
            output(&mut hanging, None, Duration::from_millis(50), 1024)
                .unwrap_err()
                .to_string()
                .contains("deadline")
        );
        assert!(began.elapsed() < Duration::from_secs(2));
        let mut noisy = Command::new("node");
        noisy.args(["-e", "process.stdout.write('x'.repeat(1000000))"]);
        assert!(
            output(&mut noisy, None, Duration::from_secs(2), 1024)
                .unwrap_err()
                .to_string()
                .contains("exceeds")
        );
    }
}
