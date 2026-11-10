use super::{Frame, Image, Language, Profile, Sample};
use crate::{
    process::{Error, ErrorKind, Result},
    tools,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

#[derive(Debug, Clone)]
struct Node {
    name: String,
    attributes: BTreeMap<String, String>,
    text: String,
    children: Vec<Node>,
}
fn document(text: &str) -> Result<Node> {
    use quick_xml::{Reader, events::Event};
    if text.len() > 64 * 1024 * 1024 {
        return Err(Error::new(
            ErrorKind::LimitExceeded,
            "xctrace export exceeds 64 mib",
        ));
    }
    let mut reader = Reader::from_str(text);
    let mut stack = vec![Node {
        name: "document".into(),
        attributes: BTreeMap::new(),
        text: String::new(),
        children: Vec::new(),
    }];
    let mut nodes = 0;
    loop {
        match reader
            .read_event()
            .map_err(|e| Error::new(ErrorKind::UnsupportedFormat, e.to_string()))?
        {
            Event::Start(start) | Event::Empty(start) => {
                let empty = text
                    .as_bytes()
                    .get(reader.buffer_position().saturating_sub(2) as usize)
                    == Some(&b'/');
                let mut attributes = BTreeMap::new();
                for attr in start.attributes() {
                    let attr = attr.map_err(|e| Error::invalid(e.to_string()))?;
                    let key = attr.key.as_ref().to_string();
                    let value = attr
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|e| Error::invalid(e.to_string()))?
                        .to_string();
                    if attributes.insert(key, value).is_some() {
                        return Err(Error::invalid("duplicate xml attribute"));
                    }
                }
                let node = Node {
                    name: start.name().as_ref().to_string(),
                    attributes,
                    text: String::new(),
                    children: Vec::new(),
                };
                nodes += 1;
                if nodes > 1_000_000 || stack.len() > 1024 {
                    return Err(Error::new(
                        ErrorKind::LimitExceeded,
                        "xctrace xml node/depth budget exceeded",
                    ));
                }
                if empty {
                    stack.last_mut().unwrap().children.push(node);
                } else {
                    stack.push(node);
                }
            }
            Event::End(_) => {
                if stack.len() < 2 {
                    return Err(Error::invalid("xml closing tag has no opener"));
                }
                let node = stack.pop().unwrap();
                stack.last_mut().unwrap().children.push(node);
            }
            Event::Text(value) => {
                let value = value.xml_content(quick_xml::XmlVersion::Implicit1_0);
                stack.last_mut().unwrap().text.push_str(&value);
            }
            Event::Eof => break,
            Event::Decl(_) | Event::Comment(_) => {}
            _ => {
                return Err(Error::new(
                    ErrorKind::UnsupportedFormat,
                    "unsupported xctrace xml construct",
                ));
            }
        }
    }
    if stack.len() != 1 {
        return Err(Error::invalid("incomplete xctrace xml"));
    }
    Ok(stack.pop().unwrap())
}
fn visit<'a>(node: &'a Node, name: &str, out: &mut Vec<&'a Node>) {
    if node.name == name {
        out.push(node);
    }
    for child in &node.children {
        visit(child, name, out);
    }
}
fn ids<'a>(node: &'a Node, result: &mut BTreeMap<String, &'a Node>) -> Result<()> {
    if let Some(id) = node.attributes.get("id")
        && result.insert(id.clone(), node).is_some()
    {
        return Err(Error::invalid("duplicate xctrace xml id"));
    }
    for child in &node.children {
        ids(child, result)?;
    }
    Ok(())
}
fn resolved<'a>(node: &'a Node, index: &'a BTreeMap<String, &'a Node>) -> Result<&'a Node> {
    match node.attributes.get("ref") {
        Some(id) => index
            .get(id)
            .copied()
            .ok_or_else(|| Error::invalid("dangling xctrace xml reference")),
        None => Ok(node),
    }
}
fn number(value: &str) -> Result<u64> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix("0x") {
        u64::from_str_radix(hex, 16)
    } else {
        value.parse()
    }
    .map_err(|_| Error::invalid("invalid xctrace numeric value"))
}
fn child<'a>(
    node: &'a Node,
    name: &str,
    index: &'a BTreeMap<String, &'a Node>,
) -> Result<&'a Node> {
    resolved(
        node.children
            .iter()
            .find(|n| n.name == name)
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::UnsupportedFormat,
                    format!("xctrace row lacks {name}"),
                )
            })?,
        index,
    )
}
fn descendant_number(node: &Node, name: &str, index: &BTreeMap<String, &Node>) -> Result<u64> {
    if node.name == name {
        return number(&resolved(node, index)?.text);
    }
    for child in &node.children {
        let child = resolved(child, index)?;
        if let Ok(value) = descendant_number(child, name, index) {
            return Ok(value);
        }
    }
    Err(Error::new(
        ErrorKind::UnsupportedFormat,
        format!("xctrace data lacks {name}"),
    ))
}

pub fn parse_time_samples(xml: &str, language: Language, clock: &str) -> Result<Profile> {
    let document = document(xml)?;
    let mut index = BTreeMap::new();
    ids(&document, &mut index)?;
    let mut rows = Vec::new();
    visit(&document, "row", &mut rows);
    if rows.is_empty() {
        return Err(Error::new(
            ErrorKind::UnsupportedFormat,
            "xctrace export has no time-sample rows",
        ));
    }
    let mut images = Vec::new();
    let mut samples = Vec::new();
    let mut issues = Vec::new();
    for row in rows {
        let timestamp = number(&child(row, "sample-time", &index)?.text)?;
        let thread = child(row, "thread", &index)?;
        let thread_id = descendant_number(thread, "tid", &index)?;
        let process_id = u32::try_from(descendant_number(thread, "pid", &index)?)
            .map_err(|_| Error::invalid("xctrace pid overflow"))?;
        let backtrace = child(row, "backtrace", &index)?;
        let mut frames = Vec::new();
        for frame in &backtrace.children {
            let frame = resolved(frame, &index)?;
            if frame.name != "frame" {
                return Err(Error::new(
                    ErrorKind::UnsupportedFormat,
                    "unexpected xctrace backtrace child",
                ));
            }
            let address = number(frame.attributes.get("addr").ok_or_else(|| {
                Error::new(
                    ErrorKind::UnsupportedFormat,
                    "xctrace frame lacks raw address",
                )
            })?)?;
            let binary = child(frame, "binary", &index)?;
            let attribute = |name: &str| {
                binary.attributes.get(name).ok_or_else(|| {
                    Error::new(
                        ErrorKind::UnsupportedFormat,
                        format!("xctrace binary lacks {name}"),
                    )
                })
            };
            let image = Image {
                path: PathBuf::from(attribute("path")?),
                identity_kind: "mach_uuid".into(),
                identity: attribute("UUID")?.to_ascii_uppercase(),
                architecture: attribute("arch")?.clone(),
                load_address: number(attribute("load-addr")?)?,
                file_offset: 0,
                mapped_virtual_address: None,
                end_address: None,
            };
            let image_index = images
                .iter()
                .position(|existing| existing == &image)
                .unwrap_or_else(|| {
                    images.push(image);
                    images.len() - 1
                });
            frames.push(Frame {
                address,
                alternate_addresses: Vec::new(),
                addresses_truncated: false,
                image: Some(image_index),
                function: frame.attributes.get("name").cloned(),
                reported_function: frame.attributes.get("name").cloned(),
                inline_sites: Vec::new(),
                source: None,
                resolution: "producer_reported; source lookup requires matched dSYM".into(),
            });
        }
        if frames.len() > 1024 {
            return Err(Error::new(
                ErrorKind::LimitExceeded,
                "xctrace stack exceeds 1024 frames",
            ));
        }
        let (weight, weight_unit) = match row.children.iter().find(|n| n.name == "weight") {
            Some(node) => (
                number(&resolved(node, &index)?.text)?,
                "time_profiler_weight_ns",
            ),
            None => (1, "sample"),
        };
        for field in &row.children {
            if !["sample-time", "thread", "backtrace", "weight", "process"]
                .contains(&field.name.as_str())
            {
                let issue = format!(
                    "unsupported optional time-sample column:{}; excluded from interpreted records",
                    field.name
                );
                if !issues.contains(&issue) {
                    issues.push(issue);
                }
            }
        }
        samples.push(Sample {
            instruction_pointer: frames.first().map_or(0, |f| f.address),
            producer_fields: serde_json::json!({"table_schema":"time-sample"}),
            process_id,
            timestamp,
            thread_id,
            cpu: None,
            weight,
            weight_unit: weight_unit.into(),
            frames,
        });
    }
    let start = samples.iter().map(|s| s.timestamp).min();
    let end = samples.iter().map(|s| s.timestamp).max();
    Ok(Profile {
        schema: "macos.xctrace.time-sample.xml.v1".into(),
        producer: "xctrace".into(),
        language,
        images,
        samples,
        aggregates: Vec::new(),
        clock: Some(clock.into()),
        ticks_per_second: 1_000_000_000,
        period: None,
        start,
        end,
        issues,
        metadata: serde_json::json!({"time_meaning":"time-profiler export sample-time in trace-relative nanoseconds","coverage_bounds":"first and last extracted samples; not collector notification time","supported_fields":["sample-time","thread","backtrace","weight","process"]}),
    })
}

pub fn executable(temporary: &Path) -> Result<(PathBuf, String)> {
    let output = tools::require_success(tools::run(
        Command::new("xcrun").args(["--find", "xctrace"]),
        temporary,
        Duration::from_secs(10),
        65536,
    )?)?;
    let path = PathBuf::from(output.stdout.trim());
    if !path.is_file() {
        return Err(Error::new(
            ErrorKind::MissingTool,
            "full Xcode with usable xctrace is required",
        ));
    }
    let version = tools::require_success(tools::run(
        Command::new(&path).arg("version"),
        temporary,
        Duration::from_secs(10),
        65536,
    )?)?;
    Ok((path, version.stdout.trim().into()))
}

#[cfg(target_os = "macos")]
mod live {
    use super::*;
    use crate::native::control::{OwnedProcess, OwnedScope};
    use crate::process::{ProcessInstance, inspect_process};
    use std::{
        ffi::CString,
        fs::OpenOptions,
        os::fd::{AsRawFd, FromRawFd, OwnedFd},
        process::Stdio,
        time::Instant,
    };
    unsafe extern "C" {
        fn notify_register_file_descriptor(
            name: *const libc::c_char,
            fd: *mut i32,
            flags: i32,
            token: *mut i32,
        ) -> u32;
        fn notify_cancel(token: i32) -> u32;
    }
    struct Notification {
        fd: OwnedFd,
        token: i32,
    }
    impl Notification {
        fn register(name: &str) -> Result<Self> {
            let name =
                CString::new(name).map_err(|_| Error::invalid("invalid notification name"))?;
            let mut fd = -1;
            let mut token = -1;
            let status =
                unsafe { notify_register_file_descriptor(name.as_ptr(), &mut fd, 0, &mut token) };
            if status != 0 || fd < 0 {
                return Err(Error::new(
                    ErrorKind::CollectorFailure,
                    format!("notify registration failed:{status}"),
                ));
            }
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };
            unsafe {
                libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
            }
            Ok(Self { fd, token })
        }
        fn observed(&self) -> Result<bool> {
            let mut fd = libc::pollfd {
                fd: self.fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let n = unsafe { libc::poll(&mut fd, 1, 0) };
            if n < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            if n == 0 {
                return Ok(false);
            }
            if fd.revents & libc::POLLIN == 0 {
                return Err(Error::new(
                    ErrorKind::CollectorFailure,
                    "xctrace notify descriptor closed",
                ));
            }
            let mut token = [0u8; 4];
            if unsafe { libc::read(fd.fd, token.as_mut_ptr().cast(), 4) } != 4 {
                return Err(Error::new(
                    ErrorKind::CollectorFailure,
                    "short readiness notification",
                ));
            }
            Ok(true)
        }
    }
    impl Drop for Notification {
        fn drop(&mut self) {
            unsafe {
                notify_cancel(self.token);
            }
        }
    }
    pub struct XctraceRecording {
        child: OwnedProcess,
        pub path: PathBuf,
        pub version: String,
        temporary: PathBuf,
        clock: String,
    }
    impl XctraceRecording {
        pub fn start(
            target: &ProcessInstance,
            temporary: &Path,
            ready_timeout: Duration,
        ) -> Result<Self> {
            if !target.same_instance(&inspect_process(target.pid)?) {
                return Err(Error::new(
                    ErrorKind::InstanceChanged,
                    "xctrace target changed",
                ));
            }
            let (executable, version) = executable(temporary)?;
            let id = hunin_core::project::Project::new_id()
                .map_err(|e| Error::invalid(e.to_string()))?;
            let notify_name = format!("org.hunin.xctrace.{id}");
            let notification = Notification::register(&notify_name)?;
            let path = temporary.join(format!("native-{id}.trace"));
            let log = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(temporary.join(format!("native-{id}.log")))?;
            let mut command = Command::new(executable);
            command
                .args(["record", "--template", "Time Profiler", "--attach"])
                .arg(target.pid.to_string())
                .arg("--output")
                .arg(&path)
                .args([
                    "--notify-tracing-started",
                    &notify_name,
                    "--time-limit",
                    "60s",
                    "--no-prompt",
                ])
                .env("TMPDIR", temporary)
                .stdout(Stdio::from(log.try_clone()?))
                .stderr(Stdio::from(log));
            let mut child = OwnedProcess::spawn(&mut command, OwnedScope::ProcessGroup)?;
            let start = Instant::now();
            loop {
                if notification.observed()? {
                    break;
                }
                if let Some(status) = child.poll()? {
                    return Err(Error::new(
                        ErrorKind::CollectorFailure,
                        format!("xctrace exited before recording-ready:{status}"),
                    ));
                }
                if start.elapsed() >= ready_timeout {
                    return Err(Error::new(
                        ErrorKind::Timeout,
                        "xctrace recording-ready notification timed out",
                    ));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            if !target.same_instance(&inspect_process(target.pid)?) {
                return Err(Error::new(
                    ErrorKind::InstanceChanged,
                    "xctrace target changed while arming",
                ));
            }
            Ok(Self {
                child,
                path,
                version,
                temporary: temporary.into(),
                clock: format!("macos.xctrace.relative-ns:{id}"),
            })
        }
        pub fn stop(mut self, language: Language) -> Result<Profile> {
            let pid = self.child.instance().pid;
            if unsafe { libc::kill(pid as i32, libc::SIGINT) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            if !self.child.wait(Duration::from_secs(5))?.success() {
                return Err(Error::new(
                    ErrorKind::CollectorFailure,
                    "xctrace recording finalization failed",
                ));
            }
            let (executable, _) = executable(&self.temporary)?;
            let export = tools::require_success(tools::run(
                Command::new(&executable)
                    .args(["export", "--input"])
                    .arg(&self.path)
                    .args([
                        "--xpath",
                        "/trace-toc/run[@number=\"1\"]/data/table[@schema=\"time-sample\"]",
                    ]),
                &self.temporary,
                Duration::from_secs(30),
                64 * 1024 * 1024,
            )?)?;
            let mut profile = parse_time_samples(&export.stdout, language, &self.clock)?;
            profile.metadata["xctrace_version"] = self.version.into();
            Ok(profile)
        }
    }
}
#[cfg(target_os = "macos")]
pub use live::XctraceRecording;
