use super::{Frame, Image, Language, Profile, SourceLocation};
use crate::process::{Error, ErrorKind, ProcessInstance, Result, inspect_process};
use std::{
    collections::BTreeMap,
    mem,
    path::{Path, PathBuf},
    ptr,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use windows_sys::{
    Win32::{
        Foundation::*,
        System::{
            Diagnostics::{Debug::*, Etw::*, ToolHelp::*},
            Performance::QueryPerformanceFrequency,
        },
    },
    core::GUID,
};
const PERF_INFO: GUID = GUID::from_u128(0xce1dbfb4_137e_4da6_87b0_3f59aa102cbc);
const STACK_WALK: GUID = GUID::from_u128(0xdef2fe46_7bd6_4b80_bd94_f57fe20d0ce3);
fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    value.encode_wide().chain(Some(0)).collect()
}
fn win_error(code: u32, action: &str) -> Error {
    Error::new(
        if code == ERROR_ACCESS_DENIED || code == ERROR_PRIVILEGE_NOT_HELD {
            ErrorKind::AccessDenied
        } else {
            ErrorKind::CollectorFailure
        },
        format!(
            "{action}: {}",
            std::io::Error::from_raw_os_error(code as i32)
        ),
    )
}
fn equal(a: GUID, b: GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}
fn guid(guid: GUID) -> String {
    format!(
        "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4[0],
        guid.data4[1],
        guid.data4[2],
        guid.data4[3],
        guid.data4[4],
        guid.data4[5],
        guid.data4[6],
        guid.data4[7]
    )
}

pub struct EtwRecording {
    handle: CONTROLTRACE_HANDLE,
    properties: Vec<u64>,
    pub path: PathBuf,
    pub target: ProcessInstance,
    start: Instant,
    closed: bool,
    initial_images: Vec<Image>,
}
impl EtwRecording {
    pub fn start(target: &ProcessInstance, directory: &Path) -> Result<Self> {
        if !target.same_instance(&inspect_process(target.pid)?) {
            return Err(Error::new(ErrorKind::InstanceChanged, "etw target changed"));
        }
        let id =
            hunin_core::project::Project::new_id().map_err(|e| Error::invalid(e.to_string()))?;
        let name = wide(std::ffi::OsStr::new(&format!("hunin-native-{id}")));
        let path = directory.join(format!("native-{id}.etl"));
        if path.exists() {
            return Err(Error::invalid("etw output already exists"));
        }
        let filename = wide(path.as_os_str());
        let base = mem::size_of::<EVENT_TRACE_PROPERTIES>();
        let length = base + (name.len() + filename.len()) * 2;
        let mut storage = vec![0u64; length.div_ceil(8)];
        let properties = storage.as_mut_ptr().cast::<EVENT_TRACE_PROPERTIES>();
        unsafe {
            let p = &mut *properties;
            p.Wnode.BufferSize = (storage.len() * 8) as u32;
            p.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
            p.Wnode.ClientContext = 1;
            p.Wnode.Guid = GUID::from_u128(
                u128::from_str_radix(&id, 16).map_err(|_| Error::invalid("invalid trace uuid"))?,
            );
            p.BufferSize = 64;
            p.MinimumBuffers = 16;
            p.MaximumBuffers = 64;
            p.MaximumFileSize = 256;
            p.LogFileMode = EVENT_TRACE_FILE_MODE_SEQUENTIAL | EVENT_TRACE_SYSTEM_LOGGER_MODE;
            p.FlushTimer = 1;
            p.EnableFlags = EVENT_TRACE_FLAG_PROFILE
                | EVENT_TRACE_FLAG_PROCESS
                | EVENT_TRACE_FLAG_THREAD
                | EVENT_TRACE_FLAG_IMAGE_LOAD;
            p.LoggerNameOffset = base as u32;
            p.LogFileNameOffset = (base + name.len() * 2) as u32;
            ptr::copy_nonoverlapping(
                name.as_ptr(),
                storage.as_mut_ptr().cast::<u8>().add(base).cast::<u16>(),
                name.len(),
            );
            ptr::copy_nonoverlapping(
                filename.as_ptr(),
                storage
                    .as_mut_ptr()
                    .cast::<u8>()
                    .add(base + name.len() * 2)
                    .cast::<u16>(),
                filename.len(),
            );
        }
        let initial_images = modules(target.pid)?;
        let mut handle: CONTROLTRACE_HANDLE = unsafe { mem::zeroed() };
        let status = unsafe { StartTraceW(&mut handle, name.as_ptr(), properties) };
        if status != 0 {
            return Err(win_error(status, "StartTraceW"));
        }
        let mut recording = Self {
            handle,
            properties: storage,
            path,
            target: target.clone(),
            start: Instant::now(),
            closed: false,
            initial_images,
        };
        let mut event = CLASSIC_EVENT_ID {
            EventGuid: PERF_INFO,
            Type: 46,
            Reserved: [0; 7],
        };
        let status = unsafe {
            TraceSetInformation(
                handle,
                TraceStackTracingInfo,
                (&mut event as *mut CLASSIC_EVENT_ID).cast(),
                mem::size_of_val(&event) as u32,
            )
        };
        if status != 0 {
            let _ = recording.stop_session();
            return Err(win_error(status, "enable sampled-profile stacks"));
        }
        if !target.same_instance(&inspect_process(target.pid)?) {
            let _ = recording.stop_session();
            return Err(Error::new(
                ErrorKind::InstanceChanged,
                "etw target changed while arming",
            ));
        }
        Ok(recording)
    }
    fn stop_session(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        let status = unsafe {
            ControlTraceW(
                self.handle,
                ptr::null(),
                self.properties.as_mut_ptr().cast(),
                EVENT_TRACE_CONTROL_STOP,
            )
        };
        if status != 0 {
            return Err(win_error(status, "stop owned etw session"));
        }
        self.closed = true;
        Ok(())
    }
    pub fn stop(mut self, language: Language) -> Result<Profile> {
        self.stop_session()?;
        let recording_elapsed = self.start.elapsed();
        let properties = unsafe { &*self.properties.as_ptr().cast::<EVENT_TRACE_PROPERTIES>() };
        let mut profile = import_etl(
            &self.path,
            &self.target,
            language,
            self.initial_images.clone(),
            Duration::from_secs(30),
        )?;
        profile.metadata["recording_elapsed_ns"] =
            serde_json::json!(recording_elapsed.as_nanos().min(u64::MAX as u128) as u64);
        profile.metadata["etw_events_lost"] = properties.EventsLost.into();
        profile.metadata["etw_log_buffers_lost"] = properties.LogBuffersLost.into();
        if properties.EventsLost > 0 || properties.LogBuffersLost > 0 {
            profile
                .issues
                .push("etw recording reports lost events or buffers".into());
        }
        Ok(profile)
    }
}
impl Drop for EtwRecording {
    fn drop(&mut self) {
        let _ = self.stop_session();
    }
}

fn modules(pid: u32) -> Result<Vec<Image>> {
    let snapshot =
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error().into());
    }
    let result = (|| {
        let mut entry: MODULEENTRY32W = unsafe { mem::zeroed() };
        entry.dwSize = mem::size_of_val(&entry) as u32;
        let mut yes = unsafe { Module32FirstW(snapshot, &mut entry) };
        let mut images = Vec::new();
        while yes != 0 {
            let path = PathBuf::from(String::from_utf16_lossy(
                &entry.szExePath[..entry
                    .szExePath
                    .iter()
                    .position(|&n| n == 0)
                    .unwrap_or(entry.szExePath.len())],
            ));
            let identity = super::symbols::pe_pdb_identity(&path)
                .map(|v| v.0)
                .unwrap_or_default();
            images.push(Image {
                path,
                architecture: "x86_64".into(),
                identity_kind: "pe_codeview_guid_age".into(),
                identity,
                load_address: entry.modBaseAddr as u64,
                file_offset: 0,
                mapped_virtual_address: None,
                end_address: (entry.modBaseAddr as u64)
                    .checked_add(entry.modBaseSize as u64)
                    .and_then(|v| v.checked_sub(1)),
            });
            if images.len() > 100000 {
                return Err(Error::new(
                    ErrorKind::LimitExceeded,
                    "too many loaded modules",
                ));
            }
            yes = unsafe { Module32NextW(snapshot, &mut entry) };
        }
        let code = unsafe { GetLastError() };
        if code != ERROR_NO_MORE_FILES {
            return Err(win_error(code, "module discovery"));
        }
        Ok(images)
    })();
    unsafe {
        CloseHandle(snapshot);
    }
    result
}

struct Consumer {
    target: u32,
    frequency: u64,
    profile: Profile,
    stacks: BTreeMap<(u64, u32), super::etw::StackEvent>,
    threads: BTreeMap<u32, u32>,
    error: Option<Error>,
    deadline: Instant,
    maximum_bytes: usize,
    bytes: usize,
}
unsafe extern "system" fn buffer_callback(log: *mut EVENT_TRACE_LOGFILEW) -> u32 {
    let consumer = unsafe { &mut *((*log).Context.cast::<Consumer>()) };
    if consumer.deadline <= Instant::now() || consumer.error.is_some() {
        if consumer.error.is_none() {
            consumer.error = Some(Error::new(
                ErrorKind::Timeout,
                "etl extraction deadline exceeded",
            ));
        }
        0
    } else {
        1
    }
}
unsafe extern "system" fn event_callback(raw: *mut EVENT_RECORD) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let event = unsafe { &*raw };
        let consumer = unsafe { &mut *(event.UserContext.cast::<Consumer>()) };
        if consumer.error.is_some() {
            return;
        }
        if let Err(error) = consume(event, consumer) {
            consumer.error = Some(error);
        }
    }));
    if result.is_err() {
        let event = unsafe { &*raw };
        unsafe { &mut *(event.UserContext.cast::<Consumer>()) }.error = Some(Error::new(
            ErrorKind::CollectorFailure,
            "etl event decoder panicked",
        ));
    }
}
fn consume(event: &EVENT_RECORD, state: &mut Consumer) -> Result<()> {
    let header = &event.EventHeader;
    let opcode = header.EventDescriptor.Opcode;
    let version = header.EventDescriptor.Version;
    let width = if header.Flags & EVENT_HEADER_FLAG_32_BIT_HEADER as u16 != 0 {
        4
    } else {
        8
    };
    let data = if event.UserDataLength == 0 {
        &[][..]
    } else {
        if event.UserData.is_null() {
            return Err(Error::invalid("etw payload pointer is null"));
        }
        unsafe {
            std::slice::from_raw_parts(event.UserData.cast::<u8>(), event.UserDataLength as usize)
        }
    };
    state.bytes = state
        .bytes
        .checked_add(data.len() + 256)
        .ok_or_else(|| Error::invalid("etw extraction size overflow"))?;
    if state.bytes > state.maximum_bytes {
        return Err(Error::new(
            ErrorKind::LimitExceeded,
            "etl extraction exceeds 64 mib budget",
        ));
    }
    if equal(header.ProviderId, STACK_WALK) && opcode == 32 {
        if let Some(stack) = super::etw::target_stack_event(data, width, version, state.target)? {
            let key = (stack.timestamp, stack.thread_id);
            if state.stacks.insert(key, stack).is_some() {
                state.profile.issues.push("multiple stackwalk records share original timestamp/thread; ambiguous stack rejected".into());
                state.stacks.remove(&key);
            }
        }
        return Ok(());
    }
    if equal(header.ProviderId, PERF_INFO) && opcode == 46 {
        let sample = super::etw::profile_event(data, width, version)?;
        if header.TimeStamp < 0 {
            return Err(Error::invalid("negative qpc event timestamp"));
        }
        let pid = state
            .threads
            .get(&sample.thread_id)
            .copied()
            .unwrap_or(header.ProcessId);
        if pid != state.target {
            return Ok(());
        }
        let timestamp = header.TimeStamp as u64;
        state.profile.samples.push(super::Sample {
            instruction_pointer: sample.instruction_pointer,
            producer_fields: serde_json::json!({"provider":guid(header.ProviderId),"opcode":opcode,"version":version,"pointer_width":width,"unused_count":sample.count}),
            process_id: pid,
            timestamp,
            thread_id: sample.thread_id as u64,
            cpu: None,
            weight: 1,
            weight_unit: "sample".into(),
            frames: Vec::new(),
        });
        return Ok(());
    }
    // TDH owns version-specific lifetime and image field decoding; retain the decoded values.
    let provider = guid(header.ProviderId);
    if [
        "3D6FA8D0-FE05-11D0-9DDA-00C04FD7BA7C",
        "3D6FA8D1-FE05-11D0-9DDA-00C04FD7BA7C",
        "2CB15D1D-5FC1-11D2-ABE1-00A0C911F518",
    ]
    .contains(&provider.as_str())
    {
        let properties = tdh_fields(event)?;
        if provider.starts_with("3D6FA8D1")
            && let (Some(tid), Some(pid)) = (
                properties
                    .get("TThreadId")
                    .or_else(|| properties.get("ThreadId"))
                    .and_then(|v| v.as_u64()),
                properties.get("ProcessId").and_then(|v| v.as_u64()),
            )
        {
            if opcode == 2 {
                state.threads.remove(&(tid as u32));
            } else {
                state.threads.insert(tid as u32, pid as u32);
            }
        }
        if properties.get("ProcessId").and_then(|v| v.as_u64()) == Some(state.target as u64) {
            let events = state.profile.metadata["lifetime_events"]
                .as_array_mut()
                .unwrap();
            events.push(serde_json::json!({"provider":provider,"opcode":opcode,"version":version,"timestamp":header.TimeStamp,"properties":properties}));
            if provider.starts_with("2CB15D1D") && [2, 10].contains(&opcode) {
                let issue = "image lifetime changed; initial module snapshot is not used for subsequent name attribution";
                if !state.profile.issues.iter().any(|s| s == issue) {
                    state.profile.issues.push(issue.into());
                }
            }
        }
    }
    Ok(())
}
fn bounded_wide(bytes: &[u8], offset: usize) -> Result<Vec<u16>> {
    if !offset.is_multiple_of(2) || offset >= bytes.len() {
        return Err(Error::invalid("invalid tdh string offset"));
    }
    let mut result = Vec::new();
    for pair in bytes[offset..].chunks_exact(2).take(32768) {
        let n = u16::from_le_bytes([pair[0], pair[1]]);
        result.push(n);
        if n == 0 {
            return Ok(result);
        }
    }
    Err(Error::invalid("unterminated tdh property name"))
}
fn tdh_fields(event: &EVENT_RECORD) -> Result<serde_json::Map<String, serde_json::Value>> {
    let mut size = 0;
    let status =
        unsafe { TdhGetEventInformation(event, 0, ptr::null(), ptr::null_mut(), &mut size) };
    if status != ERROR_INSUFFICIENT_BUFFER {
        return Err(win_error(status, "tdh schema size"));
    }
    if size as usize > 1024 * 1024 {
        return Err(Error::new(
            ErrorKind::LimitExceeded,
            "tdh schema exceeds 1 mib",
        ));
    }
    let mut storage = vec![0u64; (size as usize).div_ceil(8)];
    let info = storage.as_mut_ptr().cast::<TRACE_EVENT_INFO>();
    let status = unsafe { TdhGetEventInformation(event, 0, ptr::null(), info, &mut size) };
    if status != 0 {
        return Err(win_error(status, "tdh schema"));
    }
    let info = unsafe { &*info };
    let bytes = unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), size as usize) };
    let offset = mem::offset_of!(TRACE_EVENT_INFO, EventPropertyInfoArray);
    let count = info.TopLevelPropertyCount as usize;
    if count > 256
        || offset
            .checked_add(count * mem::size_of::<EVENT_PROPERTY_INFO>())
            .is_none_or(|n| n > bytes.len())
    {
        return Err(Error::invalid("tdh property table exceeds bounds"));
    }
    let mut result = serde_json::Map::new();
    for i in 0..count {
        let property = unsafe {
            ptr::read_unaligned(
                bytes
                    .as_ptr()
                    .add(offset + i * mem::size_of::<EVENT_PROPERTY_INFO>())
                    .cast::<EVENT_PROPERTY_INFO>(),
            )
        };
        let name = bounded_wide(bytes, property.NameOffset as usize)?;
        let key = String::from_utf16_lossy(&name[..name.len() - 1]);
        let descriptor = PROPERTY_DATA_DESCRIPTOR {
            PropertyName: name.as_ptr() as u64,
            ArrayIndex: u32::MAX,
            Reserved: 0,
        };
        let mut length = 0;
        let status =
            unsafe { TdhGetPropertySize(event, 0, ptr::null(), 1, &descriptor, &mut length) };
        if status != 0 {
            return Err(win_error(status, "tdh property size"));
        }
        if length > 1024 * 1024 {
            return Err(Error::new(
                ErrorKind::LimitExceeded,
                "tdh property exceeds 1 mib",
            ));
        }
        let mut value = vec![0; length as usize];
        let status = unsafe {
            TdhGetProperty(
                event,
                0,
                ptr::null(),
                1,
                &descriptor,
                length,
                value.as_mut_ptr(),
            )
        };
        if status != 0 {
            return Err(win_error(status, "tdh property value"));
        }
        let kind = unsafe { property.Anonymous1.nonStructType.InType } as i32;
        let decoded = match (
            if property.Flags & PropertyStruct != 0 {
                -1
            } else {
                kind
            },
            value.len(),
        ) {
            (TDH_INTYPE_UINT8, 1) => serde_json::json!(value[0]),
            (TDH_INTYPE_UINT16, 2) => {
                serde_json::json!(u16::from_le_bytes(value[..2].try_into().unwrap()))
            }
            (TDH_INTYPE_UINT32, 4) => {
                serde_json::json!(u32::from_le_bytes(value[..4].try_into().unwrap()))
            }
            (TDH_INTYPE_UINT64, 8) | (TDH_INTYPE_POINTER, 8) => {
                serde_json::json!(u64::from_le_bytes(value[..8].try_into().unwrap()))
            }
            (TDH_INTYPE_POINTER, 4) => {
                serde_json::json!(u32::from_le_bytes(value[..4].try_into().unwrap()))
            }
            (TDH_INTYPE_UNICODESTRING, _) => {
                if !value.len().is_multiple_of(2) {
                    return Err(Error::invalid("odd utf16 property"));
                }
                let words: Vec<u16> = value
                    .chunks_exact(2)
                    .map(|p| u16::from_le_bytes([p[0], p[1]]))
                    .take_while(|&n| n != 0)
                    .collect();
                serde_json::json!(String::from_utf16_lossy(&words))
            }
            _ => {
                serde_json::json!({"unsupported_in_type":kind,"flags":property.Flags,"bytes":value})
            }
        };
        result.insert(key, decoded);
    }
    Ok(result)
}

fn retain_etl_header(profile: &mut Profile, header: &TRACE_LOGFILE_HEADER) -> Result<()> {
    let details = unsafe { header.Anonymous2.Anonymous };
    if header.ReservedFlags != 1 || header.PerfFreq <= 0 {
        return Err(Error::new(
            ErrorKind::UnsupportedFormat,
            "etl requires qpc timestamps and a positive recorded frequency",
        ));
    }
    if details.PointerSize as usize != mem::size_of::<usize>() {
        return Err(Error::new(
            ErrorKind::UnsupportedFormat,
            "etl header pointer layout differs from this consumer",
        ));
    }
    profile.metadata["etl_header"] = serde_json::json!({"clock_type":header.ReservedFlags,"qpc_frequency":header.PerfFreq,"pointer_size":details.PointerSize,"events_lost":details.EventsLost,"buffers_lost":header.BuffersLost,"end_filetime":header.EndTime,"finalized":header.EndTime != 0,"loss_scope":"entire recording session; not selected-process loss"});
    if header.EndTime == 0 {
        profile.issues.push(
            "etl file was not finalized; retained observations do not establish complete coverage"
                .into(),
        );
    }
    if details.EventsLost > 0 || header.BuffersLost > 0 {
        profile
            .issues
            .push("etl header reports session-wide lost events or buffers".into());
    }
    Ok(())
}

pub fn import_etl(
    path: &Path,
    target: &ProcessInstance,
    language: Language,
    images: Vec<Image>,
    timeout: Duration,
) -> Result<Profile> {
    if std::fs::metadata(path)?.len() > 256 * 1024 * 1024 {
        return Err(Error::new(ErrorKind::LimitExceeded, "etl exceeds 256 mib"));
    }
    let mut name = wide(path.as_os_str());
    let mut log: EVENT_TRACE_LOGFILEW = unsafe { mem::zeroed() };
    log.LogFileName = name.as_mut_ptr();
    log.Anonymous1.ProcessTraceMode =
        PROCESS_TRACE_MODE_EVENT_RECORD | PROCESS_TRACE_MODE_RAW_TIMESTAMP;
    log.Anonymous2.EventRecordCallback = Some(event_callback);
    log.BufferCallback = Some(buffer_callback);
    let mut frequency = 0i64;
    if unsafe { QueryPerformanceFrequency(&mut frequency) } == 0 || frequency <= 0 {
        return Err(Error::new(
            ErrorKind::CollectorFailure,
            "qpc frequency unavailable",
        ));
    }
    let profile = Profile {
        schema: "windows.etw.sampled-profile.stackwalk.v2".into(),
        producer: "etw".into(),
        language,
        images,
        samples: Vec::new(),
        aggregates: Vec::new(),
        clock: Some(format!("windows.etl.qpc:{}", path.display())),
        ticks_per_second: frequency as u64,
        period: None,
        start: None,
        end: None,
        issues: Vec::new(),
        metadata: serde_json::json!({"lifetime_events":[],"timestamp_unit":"qpc_ticks","qpc_frequency":frequency,"sample_count_field":"documented unused payload field is retained in producer_fields.unused_count; weight is one sample, not cpu duration"}),
    };
    let mut consumer = Consumer {
        target: target.pid,
        frequency: frequency as u64,
        profile,
        stacks: BTreeMap::new(),
        threads: BTreeMap::new(),
        error: None,
        deadline: Instant::now() + timeout,
        maximum_bytes: 64 * 1024 * 1024,
        bytes: 0,
    };
    log.Context = (&mut consumer as *mut Consumer).cast();
    let handle = unsafe { OpenTraceW(&mut log) };
    if handle.Value == u64::MAX {
        return Err(std::io::Error::last_os_error().into());
    }
    if let Err(error) = retain_etl_header(&mut consumer.profile, &log.LogfileHeader) {
        unsafe {
            CloseTrace(handle);
        }
        return Err(error);
    }
    consumer.frequency = log.LogfileHeader.PerfFreq as u64;
    let status = unsafe { ProcessTrace(&handle, 1, ptr::null(), ptr::null()) };
    unsafe {
        CloseTrace(handle);
    }
    if let Some(error) = consumer.error {
        return Err(error);
    }
    if status != 0 {
        return Err(win_error(status, "ProcessTrace"));
    }
    if consumer.frequency == 0 {
        return Err(Error::invalid("etl declares zero qpc frequency"));
    }
    consumer.profile.metadata["qpc_frequency"] = consumer.frequency.into();
    consumer.profile.ticks_per_second = consumer.frequency;
    let mapping_stable = !consumer
        .profile
        .issues
        .iter()
        .any(|s| s.starts_with("image lifetime changed"));
    for sample in &mut consumer.profile.samples {
        let key = (sample.timestamp, sample.thread_id as u32);
        let stack = consumer.stacks.remove(&key);
        let addresses = stack
            .as_ref()
            .map(|s| s.addresses.clone())
            .unwrap_or_else(|| vec![sample.instruction_pointer]);
        if stack.is_none() {
            consumer.profile.issues.push(format!(
                "missing stackwalk for tid {} at {}",
                sample.thread_id, sample.timestamp
            ));
        }
        for address in addresses {
            sample.frames.push(Frame {
                address,
                alternate_addresses: Vec::new(),
                addresses_truncated: false,
                image: if mapping_stable {
                    consumer.profile.images.iter().position(|image| {
                        image.load_address <= address
                            && image.end_address.is_some_and(|end| address <= end)
                    })
                } else {
                    None
                },
                function: None,
                reported_function: None,
                inline_sites: Vec::new(),
                source: None,
                resolution: "unresolved; source lookup requires a matching pe/pdb identity".into(),
            });
        }
    }
    consumer.profile.start = consumer.profile.samples.iter().map(|s| s.timestamp).min();
    consumer.profile.end = consumer.profile.samples.iter().map(|s| s.timestamp).max();
    consumer.profile.metadata["unmatched_stackwalk_count"] = consumer.stacks.len().into();
    if consumer.profile.samples.is_empty() {
        return Err(Error::new(
            ErrorKind::CollectorFailure,
            "etl has no target sampled-profile events",
        ));
    }
    Ok(consumer.profile)
}

pub fn resolve_pdb(image: &Image, pdb: &Path, frames: &mut [Frame]) -> Result<()> {
    let selected_pdb = std::fs::canonicalize(pdb).map_err(|error| {
        Error::new(
            ErrorKind::SymbolMismatch,
            format!("selected pdb is unavailable: {error}"),
        )
    })?;
    if !selected_pdb.is_file() {
        return Err(Error::new(
            ErrorKind::SymbolMismatch,
            "selected pdb is not a file",
        ));
    }
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| Error::new(ErrorKind::CollectorFailure, "dbghelp lock poisoned"))?;
    let (identity, _) = super::symbols::pe_pdb_identity(&image.path)?;
    if identity != image.identity {
        return Err(Error::new(
            ErrorKind::SymbolMismatch,
            "pe codeview identity differs from recording",
        ));
    }
    let marker = Box::new(0u64);
    let process = (&*marker as *const u64) as HANDLE;
    let parent = pdb
        .parent()
        .ok_or_else(|| Error::invalid("pdb has no parent directory"))?;
    let search = wide(parent.as_os_str());
    let prior = unsafe { SymGetOptions() };
    unsafe {
        SymSetOptions(
            (prior & !SYMOPT_DEFERRED_LOADS)
                | SYMOPT_EXACT_SYMBOLS
                | SYMOPT_LOAD_LINES
                | SYMOPT_FAIL_CRITICAL_ERRORS
                | SYMOPT_NO_PROMPTS,
        );
    }
    if unsafe { SymInitializeW(process, search.as_ptr(), 0) } == 0 {
        let error = std::io::Error::last_os_error();
        unsafe {
            SymSetOptions(prior);
        }
        return Err(error.into());
    }
    struct Session(HANDLE, u32);
    impl Drop for Session {
        fn drop(&mut self) {
            unsafe {
                SymCleanup(self.0);
                SymSetOptions(self.1);
            }
        }
    }
    let _session = Session(process, prior);
    let path = wide(image.path.as_os_str());
    let base = unsafe {
        SymLoadModuleExW(
            process,
            ptr::null_mut(),
            path.as_ptr(),
            ptr::null(),
            image.load_address,
            0,
            ptr::null(),
            0,
        )
    };
    if base == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut module: IMAGEHLP_MODULEW64 = unsafe { mem::zeroed() };
    module.SizeOfStruct = mem::size_of_val(&module) as u32;
    if unsafe { SymGetModuleInfoW64(process, base, &mut module) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let loaded = PathBuf::from(String::from_utf16_lossy(
        &module.LoadedPdbName[..module
            .LoadedPdbName
            .iter()
            .position(|&n| n == 0)
            .unwrap_or(module.LoadedPdbName.len())],
    ));
    let identity = format!("{}:{}", guid(module.PdbSig70), module.PdbAge);
    let loaded_pdb = std::fs::canonicalize(loaded).map_err(|error| {
        Error::new(
            ErrorKind::SymbolMismatch,
            format!("loaded pdb is unavailable: {error}"),
        )
    })?;
    if module.PdbUnmatched != 0
        || module.DbgUnmatched != 0
        || identity != image.identity
        || loaded_pdb != selected_pdb
    {
        return Err(Error::new(
            ErrorKind::SymbolMismatch,
            "loaded pdb path/guid/age does not match explicitly selected debug artifact",
        ));
    }
    for frame in frames {
        if frame.address < image.load_address
            || image.end_address.is_none_or(|end| frame.address > end)
        {
            continue;
        }
        let mut storage = vec![0u64; (mem::size_of::<SYMBOL_INFOW>() + 4096).div_ceil(8)];
        let symbol = storage.as_mut_ptr().cast::<SYMBOL_INFOW>();
        unsafe {
            (*symbol).SizeOfStruct = mem::size_of::<SYMBOL_INFOW>() as u32;
            (*symbol).MaxNameLen = 2000;
        }
        let mut displacement = 0;
        if unsafe { SymFromAddrW(process, frame.address, &mut displacement, symbol) } != 0 {
            let count = unsafe { (*symbol).NameLen } as usize;
            if count > 2000 {
                return Err(Error::invalid("dbghelp name exceeds capacity"));
            }
            let name = unsafe { std::slice::from_raw_parts((*symbol).Name.as_ptr(), count) };
            frame.reported_function = Some(String::from_utf16_lossy(name));
            frame.function = frame.reported_function.clone();
        }
        let mut line: IMAGEHLP_LINEW64 = unsafe { mem::zeroed() };
        line.SizeOfStruct = mem::size_of_val(&line) as u32;
        let mut displacement = 0;
        if unsafe { SymGetLineFromAddrW64(process, frame.address, &mut displacement, &mut line) }
            != 0
            && line.LineNumber > 0
            && !line.FileName.is_null()
        {
            let mut len = 0;
            while len < 32768 && unsafe { *line.FileName.add(len) } != 0 {
                len += 1;
            }
            if len == 32768 {
                return Err(Error::invalid("dbghelp source filename exceeds bound"));
            }
            frame.source = Some(SourceLocation {
                file: String::from_utf16_lossy(unsafe {
                    std::slice::from_raw_parts(line.FileName, len)
                }),
                line: line.LineNumber,
                column: None,
            });
        }
        frame.resolution = if frame.source.is_some() {
            "matched_pe_pdb_guid_age"
        } else {
            "matched_build_no_source"
        }
        .into();
    }
    Ok(())
}

impl EtwRecording {
    pub fn definition(
        &self,
        id: &str,
        target: hunin_core::project::TargetRef,
    ) -> Result<hunin_core::records::SourceDefinition> {
        let mut frequency = 0i64;
        if unsafe { QueryPerformanceFrequency(&mut frequency) } == 0 || frequency <= 0 {
            return Err(Error::new(
                ErrorKind::CollectorFailure,
                "qpc frequency unavailable",
            ));
        }
        let profile = Profile {
            schema: "windows.etw.sampled-profile.stackwalk.v2".into(),
            producer: "etw".into(),
            language: Language::C,
            images: self.initial_images.clone(),
            samples: Vec::new(),
            aggregates: Vec::new(),
            clock: Some(format!("windows.etl.qpc:{}", self.path.display())),
            ticks_per_second: frequency as u64,
            period: None,
            start: None,
            end: None,
            issues: Vec::new(),
            metadata: serde_json::json!({"client_context":1,"sample_scope":"selected process id and its threads; child process sources are separate","source_language":"caller binds the workload build; ETW does not infer source language","timestamp_unit":"qpc_ticks","qpc_frequency":frequency}),
        };
        let mut definition = profile.definition(id, target);
        definition.metadata["language"] = serde_json::Value::Null;
        definition.metadata["language_origin"] =
            "not inferred from ETW; caller run conditions identify the workload build".into();
        Ok(definition)
    }
}
pub fn resolve_pdb_profile(profile: &mut Profile, binary: &Path, pdb: &Path) -> Result<()> {
    let (identity, _) = super::symbols::pe_pdb_identity(binary)?;
    let indices: Vec<_> = profile
        .images
        .iter()
        .enumerate()
        .filter(|(_, image)| image.identity == identity)
        .map(|(i, _)| i)
        .collect();
    if indices.is_empty() {
        return Err(Error::new(
            ErrorKind::SymbolMismatch,
            "expected PE/PDB identity is absent from recorded modules",
        ));
    }
    let mut count = 0;
    for image in indices {
        let mut unique = BTreeMap::new();
        for sample in &profile.samples {
            for frame in &sample.frames {
                if frame.image == Some(image) {
                    unique.entry(frame.address).or_insert_with(|| frame.clone());
                }
            }
        }
        let mut frames: Vec<_> = unique.into_values().collect();
        if frames.is_empty() {
            continue;
        }
        resolve_pdb(&profile.images[image], pdb, &mut frames)?;
        count += frames.len();
        let resolved: BTreeMap<_, _> = frames
            .into_iter()
            .map(|frame| (frame.address, frame))
            .collect();
        for sample in &mut profile.samples {
            for frame in &mut sample.frames {
                if frame.image == Some(image)
                    && let Some(symbol) = resolved.get(&frame.address)
                {
                    frame.function = symbol.function.clone();
                    frame.reported_function = symbol.reported_function.clone();
                    frame.source = symbol.source.clone();
                    frame.resolution = symbol.resolution.clone();
                }
            }
        }
    }
    if count == 0 {
        return Err(Error::new(
            ErrorKind::CollectorFailure,
            "no samples have an unambiguous mapping for this executable",
        ));
    }
    profile.issues.push(
        "dbghelp resolves physical instruction pointers; inline call chains are not expanded"
            .into(),
    );
    Ok(())
}
#[cfg(test)]
mod header_tests {
    use super::*;
    #[test]
    fn raw_clock_loss_and_finalization_remain_interpretable() {
        let mut profile = Profile {
            schema: String::new(),
            producer: String::new(),
            language: Language::C,
            images: Vec::new(),
            samples: Vec::new(),
            aggregates: Vec::new(),
            clock: None,
            ticks_per_second: 1,
            period: None,
            start: None,
            end: None,
            issues: Vec::new(),
            metadata: serde_json::json!({}),
        };
        let mut header: TRACE_LOGFILE_HEADER = unsafe { mem::zeroed() };
        header.ReservedFlags = 1;
        header.PerfFreq = 10_000_000;
        header.EndTime = 1;
        header.Anonymous2.Anonymous.PointerSize = mem::size_of::<usize>() as u32;
        retain_etl_header(&mut profile, &header).unwrap();
        assert!(profile.issues.is_empty());
        header.EndTime = 0;
        header.BuffersLost = 2;
        header.Anonymous2.Anonymous.EventsLost = 3;
        retain_etl_header(&mut profile, &header).unwrap();
        assert_eq!(profile.metadata["etl_header"]["events_lost"], 3);
        assert_eq!(profile.metadata["etl_header"]["buffers_lost"], 2);
        assert_eq!(profile.metadata["etl_header"]["finalized"], false);
        assert_eq!(profile.issues.len(), 2);
        header.ReservedFlags = 2;
        assert!(retain_etl_header(&mut profile, &header).is_err());
        header.ReservedFlags = 1;
        header.PerfFreq = 0;
        assert!(retain_etl_header(&mut profile, &header).is_err());
        header.PerfFreq = -1;
        assert!(retain_etl_header(&mut profile, &header).is_err());
        header.PerfFreq = 10_000_000;
        header.Anonymous2.Anonymous.PointerSize = 0;
        assert!(retain_etl_header(&mut profile, &header).is_err());
    }
}
