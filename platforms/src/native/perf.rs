use super::{Frame, Image, Sample};
use crate::process::{Error, ErrorKind, Result};

pub const SAMPLE_TYPE: u64 = 1 | 2 | 4 | 32 | 128 | 256;

pub fn decode_sample(payload: &[u8], maximum_frames: usize) -> Result<Sample> {
    if payload.len() < 48 {
        return Err(Error::invalid("short perf sample"));
    }
    let u64_at =
        |offset| -> u64 { u64::from_ne_bytes(payload[offset..offset + 8].try_into().unwrap()) };
    let u32_at =
        |offset| -> u32 { u32::from_ne_bytes(payload[offset..offset + 4].try_into().unwrap()) };
    let count =
        usize::try_from(u64_at(40)).map_err(|_| Error::invalid("perf callchain count overflow"))?;
    if count > maximum_frames
        || count.checked_mul(8).and_then(|n| n.checked_add(48)) != Some(payload.len())
    {
        return Err(Error::new(
            ErrorKind::LimitExceeded,
            "perf callchain size exceeds configured layout or frame budget",
        ));
    }
    let frames = (0..count)
        .map(|i| {
            let address = u64_at(48 + i * 8);
            Frame {
                address,
                alternate_addresses: Vec::new(),
                addresses_truncated: false,
                image: None,
                function: None,
                reported_function: None,
                inline_sites: Vec::new(),
                source: None,
                resolution: if address >= u64::MAX - 4095 {
                    "perf_context_marker".into()
                } else {
                    "unresolved".into()
                },
            }
        })
        .collect();
    Ok(Sample {
        instruction_pointer: u64_at(0),
        producer_fields: serde_json::json!({"sample_type":SAMPLE_TYPE,"reserved_cpu_field":u32_at(28)}),
        process_id: u32_at(8),
        thread_id: u32_at(12) as u64,
        timestamp: u64_at(16),
        cpu: Some(u32_at(24)),
        weight: u64_at(32),
        weight_unit: "cpu_clock_ns".into(),
        frames,
    })
}

pub fn parse_maps(text: &str) -> Result<Vec<Image>> {
    if text.len() > 16 * 1024 * 1024 {
        return Err(Error::new(
            ErrorKind::LimitExceeded,
            "process maps exceed 16 mib",
        ));
    }
    let mut result = Vec::new();
    for line in text.lines() {
        // proc maps aligns columns with spaces; split once after the first five fields.
        let words: Vec<_> = line.split_whitespace().take(5).collect();
        if words.len() != 5 {
            return Err(Error::invalid("short process map"));
        }
        let mut offset = 0;
        for _ in 0..5 {
            offset += line[offset..]
                .find(|c: char| !c.is_whitespace())
                .ok_or_else(|| Error::invalid("bad maps fields"))?;
            offset += line[offset..]
                .find(char::is_whitespace)
                .unwrap_or(line.len() - offset);
        }
        let path = line[offset..].trim_start();
        if !words[1].contains('x') || !path.starts_with('/') {
            continue;
        }
        let (start, end) = words[0]
            .split_once('-')
            .ok_or_else(|| Error::invalid("invalid map range"))?;
        let parse =
            |s: &str| u64::from_str_radix(s, 16).map_err(|_| Error::invalid("invalid map address"));
        let start = parse(start)?;
        let end = parse(end)?;
        if start >= end {
            return Err(Error::invalid("reversed process map"));
        }
        result.push(Image {
            path: path.into(),
            architecture: std::env::consts::ARCH.into(),
            identity_kind: "elf_build_id".into(),
            identity: String::new(),
            load_address: start,
            file_offset: parse(words[2])?,
            mapped_virtual_address: None,
            end_address: Some(end - 1),
        });
        if result.len() > 100_000 {
            return Err(Error::new(
                ErrorKind::LimitExceeded,
                "too many executable maps",
            ));
        }
    }
    Ok(result)
}

#[cfg(any(target_os = "linux", test))]
fn bind_sample_images(
    sample: &mut Sample,
    target_pid: u32,
    images_valid_after: u64,
    images: &[Image],
    invalid_images: &std::collections::BTreeSet<usize>,
) -> bool {
    if sample.process_id != target_pid || sample.timestamp < images_valid_after {
        for frame in &mut sample.frames {
            frame.image = None;
            if frame.resolution != "perf_context_marker" {
                frame.resolution = if sample.process_id != target_pid {
                    "sample process differs from target; no target image attribution"
                } else {
                    "sample precedes armed image snapshot; no image attribution"
                }
                .into();
            }
        }
        return sample.process_id == target_pid;
    }
    for frame in &mut sample.frames {
        if frame.resolution.starts_with("sample process differs")
            || frame
                .resolution
                .starts_with("sample precedes armed image snapshot")
        {
            frame.resolution = "unresolved".into();
        }
        frame.image = images.iter().enumerate().position(|(i, image)| {
            !invalid_images.contains(&i)
                && image.load_address <= frame.address
                && image.end_address.is_some_and(|end| frame.address <= end)
        });
    }
    true
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::native::{Language, Profile};
    use crate::process::{ProcessInstance, inspect_process};
    use std::{
        fs, mem,
        os::fd::{AsRawFd, FromRawFd, OwnedFd},
        ptr,
        sync::atomic::{AtomicU64, Ordering},
    };

    fn mapped_build_id(
        pid: u32,
        image: &Image,
        maps: &str,
        page_size: u64,
    ) -> Result<(String, u16, u64)> {
        use std::os::unix::fs::MetadataExt;
        let row = maps
            .lines()
            .find(|line| line.starts_with(&format!("{:x}-", image.load_address)))
            .ok_or_else(|| Error::invalid("mapped image missing from initial snapshot"))?;
        let fields: Vec<_> = row.split_whitespace().take(5).collect();
        let inode: u64 = fields[4]
            .parse()
            .map_err(|_| Error::invalid("invalid maps inode"))?;
        let (major, minor) = fields[3]
            .split_once(':')
            .ok_or_else(|| Error::invalid("invalid maps device"))?;
        let major =
            u64::from_str_radix(major, 16).map_err(|_| Error::invalid("invalid maps device"))?;
        let minor =
            u64::from_str_radix(minor, 16).map_err(|_| Error::invalid("invalid maps device"))?;
        let mapped = std::path::PathBuf::from(format!(
            "/proc/{pid}/map_files/{:x}-{:x}",
            image.load_address,
            image.end_address.unwrap() + 1
        ));
        for path in [
            mapped,
            std::path::PathBuf::from(format!("/proc/{pid}/exe")),
            image.path.clone(),
        ] {
            if let Ok(metadata) = fs::metadata(&path)
                && metadata.ino() == inode
                && libc::major(metadata.dev()) as u64 == major
                && libc::minor(metadata.dev()) as u64 == minor
            {
                let (id, kind) = super::super::symbols::elf_build_id(&path)?;
                let virtual_address = super::super::symbols::elf_virtual_address(
                    &path,
                    image.file_offset,
                    Some(page_size),
                )?;
                return Ok((id, kind, virtual_address));
            }
        }
        Err(Error::new(
            ErrorKind::SymbolMismatch,
            "cannot verify loaded mapping backing-file inode and device",
        ))
    }
    #[repr(C)]
    #[derive(Default)]
    struct Attr {
        kind: u32,
        size: u32,
        config: u64,
        period: u64,
        sample_type: u64,
        read_format: u64,
        flags: u64,
        wakeup_events: u32,
        bp_type: u32,
        bp_addr: u64,
        bp_len: u64,
        branch_sample_type: u64,
        sample_regs_user: u64,
        sample_stack_user: u32,
        clockid: i32,
        sample_regs_intr: u64,
        aux_watermark: u32,
        sample_max_stack: u16,
        reserved_2: u16,
        aux_sample_size: u32,
        reserved_3: u32,
        sig_data: u64,
    }
    struct Ring {
        fd: OwnedFd,
        memory: *mut u8,
        length: usize,
        data_offset: usize,
        data_size: usize,
    }
    // Each ring is drained by the owning collector thread; the kernel publishes head with release ordering.
    unsafe impl Send for Ring {}
    impl Drop for Ring {
        fn drop(&mut self) {
            unsafe {
                libc::munmap(self.memory.cast(), self.length);
            }
        }
    }

    pub struct PerfCollector {
        target: ProcessInstance,
        rings: Vec<Ring>,
        profile: Profile,
        maximum_samples: usize,
        maximum_frames: usize,
        enabled: bool,
        extracted_bytes: usize,
        captured_bytes: u64,
        invalid_images: std::collections::BTreeSet<usize>,
        images_valid_after: u64,
    }
    impl PerfCollector {
        pub fn start(
            target: &ProcessInstance,
            language: Language,
            frequency: u32,
            maximum_samples: usize,
            maximum_frames: usize,
        ) -> Result<Self> {
            if frequency == 0
                || frequency > 10_000
                || maximum_samples == 0
                || maximum_frames == 0
                || maximum_frames > 1024
            {
                return Err(Error::invalid("invalid perf limits"));
            }
            if !target.same_instance(&inspect_process(target.pid)?) {
                return Err(Error::new(
                    ErrorKind::InstanceChanged,
                    "perf target changed",
                ));
            }
            let kernel_stack_limit: u16 =
                fs::read_to_string("/proc/sys/kernel/perf_event_max_stack")?
                    .trim()
                    .parse()
                    .map_err(|_| Error::invalid("invalid kernel perf stack limit"))?;
            if kernel_stack_limit == 0 {
                return Err(Error::invalid("kernel perf stack limit is zero"));
            }
            let requested_stack = (maximum_frames as u16).min(kernel_stack_limit);
            let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
            if page < 4096 || !(page as u64).is_power_of_two() {
                return Err(Error::new(
                    ErrorKind::CollectorFailure,
                    "unsupported perf page size",
                ));
            }
            let mut rings = Vec::new();
            let mut affinity: libc::cpu_set_t = unsafe { mem::zeroed() };
            if unsafe {
                libc::sched_getaffinity(
                    target.pid as i32,
                    mem::size_of_val(&affinity),
                    &mut affinity,
                )
            } != 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            let cpus: Vec<_> = (0..libc::CPU_SETSIZE as usize)
                .filter(|&cpu| unsafe { libc::CPU_ISSET(cpu, &affinity) })
                .collect();
            if cpus.is_empty() {
                return Err(Error::new(
                    ErrorKind::CollectorFailure,
                    "target has no schedulable cpu",
                ));
            }
            let ring_limit = (128 * 1024 * 1024 / (page as usize * 65)).min(4096);
            for entry in fs::read_dir(format!("/proc/{}/task", target.pid))? {
                let entry = entry?;
                let tid: i32 = entry
                    .file_name()
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| Error::invalid("invalid task id"))?;
                for &cpu in &cpus {
                    if rings.len() >= ring_limit {
                        return Err(Error::new(
                            ErrorKind::LimitExceeded,
                            "perf ring budget exceeds 128 mib",
                        ));
                    }
                    let mut attr = Attr {
                        kind: 1,
                        size: mem::size_of::<Attr>() as u32,
                        config: 0,
                        period: 1_000_000_000 / frequency as u64,
                        sample_type: SAMPLE_TYPE,
                        flags: 1
                            | (1 << 1)
                            | (1 << 5)
                            | (1 << 6)
                            | (1 << 8)
                            | (1 << 9)
                            | (1 << 13)
                            | (1 << 25)
                            | (1 << 35),
                        wakeup_events: 1,
                        clockid: libc::CLOCK_MONOTONIC,
                        sample_max_stack: requested_stack,
                        ..Attr::default()
                    };
                    let raw = unsafe {
                        libc::syscall(
                            libc::SYS_perf_event_open,
                            &mut attr as *mut Attr,
                            tid,
                            cpu as i32,
                            -1,
                            8,
                        )
                    };
                    if raw < 0 {
                        return Err(Error::new(
                            if std::io::Error::last_os_error().kind()
                                == std::io::ErrorKind::PermissionDenied
                            {
                                ErrorKind::AccessDenied
                            } else {
                                ErrorKind::CollectorFailure
                            },
                            format!(
                                "perf_event_open for tid {tid}: {}",
                                std::io::Error::last_os_error()
                            ),
                        ));
                    }
                    let fd = unsafe { OwnedFd::from_raw_fd(raw as i32) };
                    let length = page as usize * 65;
                    let memory = unsafe {
                        libc::mmap(
                            ptr::null_mut(),
                            length,
                            libc::PROT_READ | libc::PROT_WRITE,
                            libc::MAP_SHARED,
                            fd.as_raw_fd(),
                            0,
                        )
                    };
                    if memory == libc::MAP_FAILED {
                        return Err(std::io::Error::last_os_error().into());
                    }
                    rings.push(Ring {
                        fd,
                        memory: memory.cast(),
                        length,
                        data_offset: page as usize,
                        data_size: page as usize * 64,
                    });
                    if rings.len() > 4096 {
                        return Err(Error::new(
                            ErrorKind::LimitExceeded,
                            "too many threads for perf collection",
                        ));
                    }
                }
            }
            if rings.is_empty() {
                return Err(Error::new(
                    ErrorKind::ProcessGone,
                    "target has no observable threads",
                ));
            }
            let enabled_start = crate::os::monotonic_ns()?;
            for ring in &rings {
                if unsafe { libc::ioctl(ring.fd.as_raw_fd(), 0x2400, 0) } < 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
            }
            // Initial bindings are acquired only while every ring can report mapping changes.
            let image_snapshot_start = crate::os::monotonic_ns()?;
            let maps = fs::read_to_string(format!("/proc/{}/maps", target.pid))?;
            let mut images = parse_maps(&maps)?;
            let mut issues = Vec::new();
            for image in &mut images {
                match mapped_build_id(target.pid, image, &maps, page as u64) {
                    Ok((id, _, virtual_address)) => {
                        image.identity = id;
                        image.mapped_virtual_address = Some(virtual_address);
                    }
                    Err(error) => issues.push(format!(
                        "image build identity unavailable for {}: {error}",
                        image.path.display()
                    )),
                }
            }
            if !target.same_instance(&inspect_process(target.pid)?) {
                return Err(Error::new(
                    ErrorKind::InstanceChanged,
                    "perf target changed while acquiring armed image bindings",
                ));
            }
            let images_valid_after = crate::os::monotonic_ns()?;
            Ok(Self {
                target: target.clone(),
                rings,
                profile: Profile {
                    schema: "linux.perf.cpu-clock.callchain.v1".into(),
                    producer: "perf_event_open".into(),
                    language,
                    images,
                    samples: Vec::new(),
                    aggregates: Vec::new(),
                    clock: Some(crate::os::clock_identity()?),
                    ticks_per_second: 1_000_000_000,
                    period: Some(1_000_000_000 / frequency as u64),
                    start: Some(enabled_start),
                    end: None,
                    issues,
                    metadata: serde_json::json!({"sample_type":SAMPLE_TYPE,"lifetime_events":[],"frequency":frequency,"inherit":true,"inherit_thread":true,"cpus":cpus,"exclude_kernel":true,"callchain":"frame_pointer","sample_max_stack":requested_stack,"kernel_perf_event_max_stack":kernel_stack_limit,"mapping_page_size":page,"pid":target.pid,"armed_image_snapshot_start_ns":image_snapshot_start,"images_valid_after_ns":images_valid_after,"image_binding_scope":"initial snapshot acquired after all rings enabled; earlier sample addresses remain unbound; later overlapping mapping events invalidate bindings"}),
                },
                maximum_samples,
                maximum_frames,
                enabled: true,
                extracted_bytes: 0,
                captured_bytes: 0,
                invalid_images: std::collections::BTreeSet::new(),
                images_valid_after,
            })
        }
        pub fn drain(&mut self) -> Result<usize> {
            let before = self.profile.samples.len();
            for ring in &mut self.rings {
                let head = unsafe {
                    (&*ring.memory.add(1024).cast::<AtomicU64>()).load(Ordering::Acquire)
                };
                let tail_ptr = unsafe { &*ring.memory.add(1032).cast::<AtomicU64>() };
                let mut tail = tail_ptr.load(Ordering::Relaxed);
                if head.wrapping_sub(tail) > ring.data_size as u64 {
                    return Err(Error::new(
                        ErrorKind::LimitExceeded,
                        "perf producer overran ring buffer",
                    ));
                }
                let copy =
                    |position: u64, count: usize| -> Vec<u8> {
                        (0..count)
                            .map(|i| unsafe {
                                ptr::read_volatile(ring.memory.add(
                                    ring.data_offset + (position as usize + i) % ring.data_size,
                                ))
                            })
                            .collect()
                    };
                while tail < head {
                    if head - tail < 8 {
                        return Err(Error::invalid("partial published perf record"));
                    }
                    let header = copy(tail, 8);
                    let kind = u32::from_ne_bytes(header[..4].try_into().unwrap());
                    let size = u16::from_ne_bytes(header[6..8].try_into().unwrap()) as usize;
                    if size < 8 || size > ring.data_size || size as u64 > head - tail {
                        return Err(Error::invalid("invalid perf record length"));
                    }
                    self.captured_bytes = self
                        .captured_bytes
                        .checked_add(size as u64)
                        .ok_or_else(|| Error::invalid("perf captured byte count overflow"))?;
                    let data = copy(tail + 8, size - 8);
                    self.extracted_bytes =
                        self.extracted_bytes
                            .checked_add(data.len() * 8 + 256)
                            .ok_or_else(|| Error::invalid("native extraction size overflow"))?;
                    if self.extracted_bytes > 64 * 1024 * 1024 {
                        return Err(Error::new(
                            ErrorKind::LimitExceeded,
                            "perf extracted profile exceeds 64 mib",
                        ));
                    }
                    match kind {
                        9 => {
                            if self.profile.samples.len() >= self.maximum_samples {
                                return Err(Error::new(
                                    ErrorKind::LimitExceeded,
                                    "perf sample budget exceeded",
                                ));
                            }
                            let mut sample = decode_sample(&data, self.maximum_frames)?;
                            if !bind_sample_images(
                                &mut sample,
                                self.target.pid,
                                self.images_valid_after,
                                &self.profile.images,
                                &self.invalid_images,
                            ) {
                                let issue = "sample process differs from target; foreign addresses remain unbound";
                                if !self.profile.issues.iter().any(|s| s == issue) {
                                    self.profile.issues.push(issue.into());
                                }
                            } else if sample.timestamp < self.images_valid_after {
                                let issue = "samples precede armed image snapshot; retained addresses have no image attribution";
                                if !self.profile.issues.iter().any(|s| s == issue) {
                                    self.profile.issues.push(issue.into());
                                }
                            }
                            self.profile.samples.push(sample);
                        }
                        1 | 2 | 3 | 4 | 7 | 10 => {
                            let event = decode_lifetime(kind, &data)?;
                            if kind == 1 || kind == 10 {
                                let address = event["properties"]["address"].as_u64().unwrap();
                                let length = event["properties"]["length"].as_u64().unwrap();
                                let end = address
                                    .checked_add(length)
                                    .ok_or_else(|| Error::invalid("perf mapping range overflow"))?;
                                for (i, image) in self.profile.images.iter().enumerate() {
                                    if image.load_address < end
                                        && image
                                            .end_address
                                            .is_some_and(|old_end| address <= old_end)
                                    {
                                        self.invalid_images.insert(i);
                                    }
                                }
                                for old_sample in &mut self.profile.samples {
                                    for frame in &mut old_sample.frames {
                                        if frame
                                            .image
                                            .is_some_and(|i| self.invalid_images.contains(&i))
                                        {
                                            frame.image = None;
                                            frame.resolution="mapping lifetime ambiguous; retained address without stale image attribution".into();
                                        }
                                    }
                                }
                                if !self
                                    .profile
                                    .issues
                                    .iter()
                                    .any(|s| s.starts_with("mapping changed"))
                                {
                                    self.profile.issues.push("mapping changed; overlapping initial mappings remain unresolved because event timestamps were not requested".into());
                                }
                            }
                            if kind == 2 {
                                self.profile
                                    .issues
                                    .push(format!("perf_lost:{}", event["properties"]["lost"]));
                            }
                            self.profile.metadata["lifetime_events"]
                                .as_array_mut()
                                .ok_or_else(|| Error::invalid("missing perf event list"))?
                                .push(event);
                        }
                        other => {
                            let issue = format!("unsupported perf record type:{other}");
                            if !self.profile.issues.contains(&issue) {
                                self.profile.issues.push(issue);
                            }
                        }
                    }
                    tail += size as u64;
                }
                tail_ptr.store(tail, Ordering::Release);
            }
            Ok(self.profile.samples.len() - before)
        }
        pub fn stop(mut self) -> Result<Profile> {
            for ring in &self.rings {
                if unsafe { libc::ioctl(ring.fd.as_raw_fd(), 0x2401, 0) } < 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
            }
            self.enabled = false;
            self.drain()?;
            self.profile.end = Some(crate::os::monotonic_ns()?);
            self.profile.metadata["captured_recording_bytes"] = self.captured_bytes.into();
            self.profile.metadata["kernel_ring_capacity_bytes"] =
                self.recording_buffer_bytes().into();

            Ok(std::mem::replace(
                &mut self.profile,
                Profile {
                    schema: String::new(),
                    producer: String::new(),
                    language: Language::C,
                    images: Vec::new(),
                    samples: Vec::new(),
                    aggregates: Vec::new(),
                    clock: None,
                    ticks_per_second: 1_000_000_000,
                    period: None,
                    start: None,
                    end: None,
                    issues: Vec::new(),
                    metadata: serde_json::Value::Null,
                },
            ))
        }
        pub fn recording_buffer_bytes(&self) -> u64 {
            self.rings.iter().map(|ring| ring.length as u64).sum()
        }
        pub fn definition(
            &self,
            id: &str,
            target: hunin_core::project::TargetRef,
        ) -> hunin_core::records::SourceDefinition {
            self.profile.definition(id, target)
        }
        pub fn target(&self) -> &ProcessInstance {
            &self.target
        }
    }
    impl Drop for PerfCollector {
        fn drop(&mut self) {
            if self.enabled {
                for ring in &self.rings {
                    unsafe {
                        libc::ioctl(ring.fd.as_raw_fd(), 0x2401, 0);
                    }
                }
            }
        }
    }
}
#[cfg(target_os = "linux")]
pub use linux::PerfCollector;

pub fn decode_lifetime(kind: u32, payload: &[u8]) -> Result<serde_json::Value> {
    let u32_at = |offset: usize| -> Result<u32> {
        Ok(u32::from_ne_bytes(
            payload
                .get(offset..offset + 4)
                .ok_or_else(|| Error::invalid("short perf lifetime record"))?
                .try_into()
                .unwrap(),
        ))
    };
    let u64_at = |offset: usize| -> Result<u64> {
        Ok(u64::from_ne_bytes(
            payload
                .get(offset..offset + 8)
                .ok_or_else(|| Error::invalid("short perf lifetime record"))?
                .try_into()
                .unwrap(),
        ))
    };
    let name = |offset: usize| -> Result<String> {
        let bytes = payload
            .get(offset..)
            .ok_or_else(|| Error::invalid("short perf string"))?;
        let length = bytes
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| Error::invalid("unterminated perf string"))?;
        std::str::from_utf8(&bytes[..length])
            .map(str::to_string)
            .map_err(|_| Error::new(ErrorKind::UnsupportedFormat, "non-utf8 perf pathname/name"))
    };
    let (timestamp, properties) = match kind {
        1 | 10 => {
            let mut fields = serde_json::json!({"pid":u32_at(0)?,"tid":u32_at(4)?,"address":u64_at(8)?,"length":u64_at(16)?,"file_offset":u64_at(24)?,"filename":name(if kind==1{32}else{64})?});
            if kind == 10 {
                fields["device_major"] = u32_at(32)?.into();
                fields["device_minor"] = u32_at(36)?.into();
                fields["inode"] = u64_at(40)?.into();
                fields["inode_generation"] = u64_at(48)?.into();
                fields["protection"] = u32_at(56)?.into();
                fields["mapping_flags"] = u32_at(60)?.into();
            }
            (serde_json::Value::Null, fields)
        }
        2 => (
            serde_json::Value::Null,
            serde_json::json!({"event_id":u64_at(0)?,"lost":u64_at(8)?}),
        ),
        3 => (
            serde_json::Value::Null,
            serde_json::json!({"pid":u32_at(0)?,"tid":u32_at(4)?,"comm":name(8)?}),
        ),
        4 | 7 => (
            u64_at(16)?.into(),
            serde_json::json!({"pid":u32_at(0)?,"parent_pid":u32_at(4)?,"tid":u32_at(8)?,"parent_tid":u32_at(12)?}),
        ),
        _ => {
            return Err(Error::new(
                ErrorKind::UnsupportedFormat,
                "unknown perf lifetime record type",
            ));
        }
    };
    Ok(
        serde_json::json!({"provider":format!("linux.perf.record.{kind}"),"timestamp":timestamp,"properties":properties}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn foreign_process_samples_never_borrow_target_image_addresses() {
        let mut payload = vec![0; 56];
        payload[..8].copy_from_slice(&0x1234u64.to_ne_bytes());
        payload[8..12].copy_from_slice(&7u32.to_ne_bytes());
        payload[12..16].copy_from_slice(&9u32.to_ne_bytes());
        payload[40..48].copy_from_slice(&1u64.to_ne_bytes());
        payload[48..56].copy_from_slice(&0x1234u64.to_ne_bytes());
        let mut sample = decode_sample(&payload, 8).unwrap();
        let images = parse_maps("1000-2000 r-xp 00000000 08:01 1 /fixture\n").unwrap();
        assert!(!bind_sample_images(
            &mut sample,
            8,
            0,
            &images,
            &BTreeSet::new()
        ));
        assert_eq!(sample.process_id, 7);
        assert_eq!(sample.instruction_pointer, 0x1234);
        assert_eq!(sample.frames[0].address, 0x1234);
        assert_eq!(sample.frames[0].image, None);
        assert!(sample.frames[0].resolution.contains("process"));
        assert!(bind_sample_images(
            &mut sample,
            7,
            0,
            &images,
            &BTreeSet::new()
        ));
        assert_eq!(sample.frames[0].image, Some(0));
        assert!(bind_sample_images(
            &mut sample,
            7,
            1,
            &images,
            &BTreeSet::new()
        ));
        assert_eq!(sample.frames[0].image, None);
        assert!(
            sample.frames[0]
                .resolution
                .contains("precedes armed image snapshot")
        );
        assert!(bind_sample_images(
            &mut sample,
            7,
            0,
            &images,
            &BTreeSet::from([0])
        ));
        assert_eq!(sample.frames[0].image, None);
    }
}
