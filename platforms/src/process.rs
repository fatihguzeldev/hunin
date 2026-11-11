use serde::{Deserialize, Serialize};
use std::{fmt, io, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    AccessDenied,
    ProcessGone,
    InstanceChanged,
    InvalidInput,
    MissingTool,
    UnsupportedFormat,
    SymbolMismatch,
    CollectorFailure,
    Timeout,
    LimitExceeded,
    Io,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidInput, message)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        let kind = match error.kind() {
            io::ErrorKind::PermissionDenied => ErrorKind::AccessDenied,
            io::ErrorKind::NotFound => ErrorKind::ProcessGone,
            _ => ErrorKind::Io,
        };
        Self::new(kind, error.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessInstance {
    pub platform: String,
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub boot_identity: Option<String>,
    pub namespace: Option<String>,
    pub start_identity: String,
    pub executable: Option<PathBuf>,
    pub name: String,
}

impl ProcessInstance {
    pub fn same_instance(&self, other: &Self) -> bool {
        self.platform == other.platform
            && self.pid == other.pid
            && self.boot_identity == other.boot_identity
            && self.namespace == other.namespace
            && self.start_identity == other.start_identity
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counter {
    pub raw: u64,
    pub nanoseconds_numerator: u64,
    pub denominator: u64,
    pub source_unit: String,
}

impl Counter {
    pub fn new(raw: u64, numerator: u64, denominator: u64, unit: &str) -> Result<Self> {
        if numerator == 0 || denominator == 0 {
            return Err(Error::invalid("counter scale must be positive"));
        }
        Ok(Self {
            raw,
            nanoseconds_numerator: numerator,
            denominator,
            source_unit: unit.into(),
        })
    }
    pub fn nanoseconds(&self) -> Result<u64> {
        if self.denominator == 0 || self.nanoseconds_numerator == 0 {
            return Err(Error::invalid("counter denominator is zero"));
        }
        let value = u128::from(self.raw) * u128::from(self.nanoseconds_numerator)
            / u128::from(self.denominator);
        u64::try_from(value).map_err(|_| Error::invalid("counter duration exceeds u64 nanoseconds"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryReading {
    pub definition: String,
    pub bytes: u64,
    pub approximate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcquisitionWindow {
    pub clock: String,
    pub start_ns: u64,
    pub end_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSnapshot {
    pub instance: ProcessInstance,
    pub acquired: AcquisitionWindow,
    pub user_cpu: Counter,
    pub system_cpu: Counter,
    pub memory: Vec<MemoryReading>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnavailableTarget {
    pub pid: u32,
    pub error: Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Discovery {
    pub processes: Vec<ProcessInstance>,
    pub unavailable: Vec<UnavailableTarget>,
}

pub(crate) fn validate_pid(pid: u32) -> Result<()> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Err(Error::invalid(
            "pid is outside the supported process-id range",
        ));
    }
    Ok(())
}
pub fn inspect_process(pid: u32) -> Result<ProcessInstance> {
    validate_pid(pid)?;
    crate::os::inspect(pid)
}

#[cfg(windows)]
pub fn discover_processes() -> Result<Discovery> {
    crate::os::discover()
}
#[cfg(not(windows))]
pub fn discover_processes() -> Result<Discovery> {
    let mut result = Discovery {
        processes: Vec::new(),
        unavailable: Vec::new(),
    };
    for pid in crate::os::pids()? {
        match inspect_process(pid) {
            Ok(process) => result.processes.push(process),
            Err(error) => result.unavailable.push(UnavailableTarget { pid, error }),
        }
    }
    Ok(result)
}

pub fn read_resources(expected: &ProcessInstance) -> Result<ResourceSnapshot> {
    validate_pid(expected.pid)?;
    let start_ns = crate::os::monotonic_ns()?;
    let before = crate::os::inspect_for_resources(expected.pid)?;
    if !expected.same_instance(&before) {
        return Err(Error::new(
            ErrorKind::InstanceChanged,
            "process instance changed before resource read",
        ));
    }
    let (user_cpu, system_cpu, memory) = crate::os::resources(expected.pid)?;
    let after = crate::os::inspect_for_resources(expected.pid)?;
    if !expected.same_instance(&after) {
        return Err(Error::new(
            ErrorKind::InstanceChanged,
            "process instance changed during resource read",
        ));
    }
    let end_ns = crate::os::monotonic_ns()?;
    if end_ns < start_ns {
        return Err(Error::new(
            ErrorKind::CollectorFailure,
            "monotonic clock moved backwards",
        ));
    }
    #[cfg(windows)]
    let after = ProcessInstance {
        parent_pid: expected.parent_pid,
        ..after
    };
    Ok(ResourceSnapshot {
        instance: after,
        acquired: AcquisitionWindow {
            clock: crate::os::clock_identity()?,
            start_ns,
            end_ns,
        },
        user_cpu,
        system_cpu,
        memory,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxStat {
    pub pid: u32,
    pub name: String,
    pub parent_pid: u32,
    pub user_ticks: u64,
    pub system_ticks: u64,
    pub start_ticks: u64,
}

pub fn parse_linux_stat(input: &str) -> Result<LinuxStat> {
    if input.len() > 64 * 1024 {
        return Err(Error::new(
            ErrorKind::LimitExceeded,
            "proc stat exceeds 64 kib",
        ));
    }
    let open = input
        .find('(')
        .ok_or_else(|| Error::invalid("missing proc comm opening parenthesis"))?;
    let close = input
        .rfind(')')
        .ok_or_else(|| Error::invalid("missing proc comm closing parenthesis"))?;
    if close <= open {
        return Err(Error::invalid("invalid proc comm"));
    }
    let pid = input[..open]
        .trim()
        .parse()
        .map_err(|_| Error::invalid("invalid proc pid"))?;
    let fields: Vec<_> = input[close + 1..].split_whitespace().collect();
    if fields.len() < 20 || fields[0].len() != 1 {
        return Err(Error::invalid("short or invalid proc stat"));
    }
    let number = |index: usize| -> Result<u64> {
        fields[index]
            .parse()
            .map_err(|_| Error::invalid(format!("invalid proc stat field {}", index + 3)))
    };
    Ok(LinuxStat {
        pid,
        name: input[open + 1..close].into(),
        parent_pid: u32::try_from(number(1)?).map_err(|_| Error::invalid("parent pid overflow"))?,
        user_ticks: number(11)?,
        system_ticks: number(12)?,
        start_ticks: number(19)?,
    })
}

pub fn parse_smaps_rollup(input: &str) -> Result<Vec<MemoryReading>> {
    if input.len() > 256 * 1024 {
        return Err(Error::new(
            ErrorKind::LimitExceeded,
            "smaps rollup exceeds 256 kib",
        ));
    }
    let mut result = Vec::new();
    let mut names = std::collections::HashSet::new();
    for line in input.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        if let Some((left, right)) = fields.next().and_then(|first| first.split_once('-'))
            && u64::from_str_radix(left, 16).is_ok()
            && u64::from_str_radix(right, 16).is_ok()
            && fields.next().is_some_and(|permissions| {
                permissions.len() == 4 && permissions.ends_with(['p', 's'])
            })
        {
            continue;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| Error::invalid("unrecognized smaps record"))?;
        if name.len() > 128 || !names.insert(name) {
            return Err(Error::invalid("duplicate or oversized smaps field"));
        }
        let parts: Vec<_> = value.split_whitespace().collect();
        if parts.len() != 2 || parts[1] != "kB" {
            return Err(Error::invalid(format!(
                "unsupported smaps units for {name}"
            )));
        }
        let kb = parts[0]
            .parse::<u64>()
            .map_err(|_| Error::invalid("invalid smaps value"))?;
        let bytes = kb
            .checked_mul(1024)
            .ok_or_else(|| Error::invalid("smaps byte conversion overflow"))?;
        result.push(MemoryReading {
            definition: format!("linux.proc.smaps_rollup.{name}"),
            bytes,
            approximate: false,
        });
        if result.len() > 256 {
            return Err(Error::new(
                ErrorKind::LimitExceeded,
                "too many smaps fields",
            ));
        }
    }
    if !names.contains("Rss") {
        return Err(Error::invalid("smaps rollup has no Rss"));
    }
    Ok(result)
}
