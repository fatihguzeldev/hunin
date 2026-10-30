//! Native sampling prerequisite evidence, without starting a target session.
#[cfg(windows)]
use super::probe::checked;
use serde_json::{Value, json};
#[cfg(windows)]
use std::process::Command;

pub(super) fn inspect() -> Value {
    #[cfg(target_os = "linux")]
    {
        // Test one inactive user-only CPU counter, without sampling any target.
        #[repr(C)]
        struct Attr {
            kind: u32,
            size: u32,
            config: u64,
            period: u64,
            sample_type: u64,
            read_format: u64,
            flags: u64,
            wakeup: u32,
            bp_type: u32,
            config1: u64,
        }
        let attr = Attr {
            kind: 1,
            size: std::mem::size_of::<Attr>() as u32,
            config: 0,
            period: 0,
            sample_type: 0,
            read_format: 0,
            flags: 1 | (1 << 5) | (1 << 6),
            wakeup: 0,
            bp_type: 0,
            config1: 0,
        };
        let fd = unsafe { libc::syscall(libc::SYS_perf_event_open, &attr, 0, -1, -1, 0) };
        if fd >= 0 {
            unsafe {
                libc::close(fd as i32);
            };
            return json!({"state":"basic_permission_observed","probe":"inactive self-process perf software CPU counter; complete samply recording remains unverified"});
        }
        json!({"state":"permission_unavailable","error":std::io::Error::last_os_error().to_string(),"perf_event_paranoid":std::fs::read_to_string("/proc/sys/kernel/perf_event_paranoid").ok().map(|s|s.trim().to_owned())})
    }
    #[cfg(windows)]
    {
        let Some(xperf) = crate::collection::native::tool::xperf_path() else {
            return json!({"state":"native_prerequisite_missing","prerequisite":"xperf.exe","error":"Install Windows Performance Toolkit (WPT) from the Windows ADK; xperf.exe was absent from PATH and the standard ProgramFiles(x86) installation","setup_url":"https://learn.microsoft.com/en-us/windows-hardware/test/wpt/"});
        };
        return match checked(Command::new("whoami.exe").arg("/groups")) {
            Ok(groups) => {
                json!({"state":if groups.contains("S-1-16-12288")||groups.contains("S-1-16-16384"){"elevated_token_observed"}else{"elevation_required"},"xperf_executable":xperf,"probe":"bounded current-token check and WPT executable discovery; actual ETW session not started; occupied NT Kernel Logger causes actionable refusal"})
            }
            Err(error) => json!({"state":"permission_unverified","error":format!("{error:#}")}),
        };
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    json!({"state":"target_permission_unverified","probe":"macOS task access/signing depends on the measured executable; discovery does not attach or request permission"})
}
