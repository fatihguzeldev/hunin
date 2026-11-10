use crate::process::{
    Counter, Error, ErrorKind, MemoryReading, ProcessInstance, Result, parse_linux_stat,
    parse_smaps_rollup,
};
use std::{fs, io::Read};

fn read(path: impl AsRef<std::path::Path>, limit: u64) -> Result<String> {
    let mut text = String::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > limit {
        return Err(Error::new(
            ErrorKind::LimitExceeded,
            "procfs input exceeds limit",
        ));
    }
    Ok(text)
}
fn identity_scope() -> Result<(String, String)> {
    let boot = read("/proc/sys/kernel/random/boot_id", 128)?
        .trim()
        .to_owned();
    let namespace = fs::read_link("/proc/self/ns/pid")?
        .to_string_lossy()
        .into_owned();
    Ok((boot, namespace))
}
pub(crate) fn inspect(pid: u32) -> Result<ProcessInstance> {
    let stat = parse_linux_stat(&read(format!("/proc/{pid}/stat"), 65_536)?)?;
    if stat.pid != pid {
        return Err(Error::new(
            ErrorKind::InstanceChanged,
            "procfs pid mismatch",
        ));
    }
    let (boot, namespace) = identity_scope()?;
    Ok(ProcessInstance {
        platform: "linux".into(),
        pid,
        parent_pid: Some(stat.parent_pid),
        boot_identity: Some(boot),
        namespace: Some(namespace),
        start_identity: stat.start_ticks.to_string(),
        executable: fs::read_link(format!("/proc/{pid}/exe")).ok(),
        name: stat.name,
    })
}
pub(crate) fn pids() -> Result<Vec<u32>> {
    let mut pids = Vec::new();
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        if let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        {
            pids.push(pid);
            if pids.len() > 1_000_000 {
                return Err(Error::new(
                    ErrorKind::LimitExceeded,
                    "process discovery exceeds one million ids",
                ));
            }
        }
    }
    pids.sort_unstable();
    Ok(pids)
}
pub(crate) fn resources(pid: u32) -> Result<(Counter, Counter, Vec<MemoryReading>)> {
    let stat = parse_linux_stat(&read(format!("/proc/{pid}/stat"), 65_536)?)?;
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks <= 0 {
        return Err(Error::new(
            ErrorKind::CollectorFailure,
            "clock tick rate unavailable",
        ));
    }
    let memory = match read(format!("/proc/{pid}/smaps_rollup"), 262_144) {
        Ok(text) => parse_smaps_rollup(&text)?,
        Err(error) if matches!(error.kind, ErrorKind::AccessDenied | ErrorKind::ProcessGone) => {
            let status = read(format!("/proc/{pid}/status"), 65_536)?;
            let line = status
                .lines()
                .find(|l| l.starts_with("VmRSS:"))
                .ok_or_else(|| {
                    Error::new(
                        ErrorKind::CollectorFailure,
                        "both rollup and resident fallback are unavailable",
                    )
                })?;
            let values: Vec<_> = line.split_whitespace().collect();
            if values.len() != 3 || values[2] != "kB" {
                return Err(Error::invalid("invalid VmRSS"));
            }
            let kb: u64 = values[1]
                .parse()
                .map_err(|_| Error::invalid("invalid VmRSS value"))?;
            vec![MemoryReading {
                definition: "linux.proc.status.VmRSS".into(),
                bytes: kb
                    .checked_mul(1024)
                    .ok_or_else(|| Error::invalid("VmRSS overflow"))?,
                approximate: true,
            }]
        }
        Err(error) => return Err(error),
    };
    Ok((
        Counter::new(stat.user_ticks, 1_000_000_000, ticks as u64, "clock_ticks")?,
        Counter::new(
            stat.system_ticks,
            1_000_000_000,
            ticks as u64,
            "clock_ticks",
        )?,
        memory,
    ))
}
pub(crate) fn monotonic_ns() -> Result<u64> {
    let mut time = std::mem::MaybeUninit::<libc::timespec>::uninit();
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, time.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let time = unsafe { time.assume_init() };
    let seconds =
        u64::try_from(time.tv_sec).map_err(|_| Error::invalid("negative monotonic time"))?;
    seconds
        .checked_mul(1_000_000_000)
        .and_then(|s| s.checked_add(time.tv_nsec as u64))
        .ok_or_else(|| Error::invalid("monotonic clock overflow"))
}
pub(crate) fn clock_identity() -> Result<String> {
    Ok(format!("linux.CLOCK_MONOTONIC:{}", identity_scope()?.0))
}
