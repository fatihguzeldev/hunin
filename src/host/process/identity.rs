/// Native process birth identity, retaining the OS precision rather than
/// sysinfo's whole-second presentation. Unavailable identities stay unknown.
pub(crate) fn birth(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    native_birth(pid)
}

#[cfg(target_os = "linux")]
fn native_birth(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    linux_start_ticks(&stat).map(|ticks| format!("linux-start-ticks:{ticks}"))
}

#[cfg(any(target_os = "linux", test))]
fn linux_start_ticks(stat: &str) -> Option<u64> {
    // comm can contain spaces and closing parentheses. Fields after its last
    // closing parenthesis start at field3; process starttime is field22.
    stat.get(stat.rfind(')')? + 1..)?
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

#[cfg(target_os = "macos")]
fn native_birth(pid: u32) -> Option<String> {
    let pid = i32::try_from(pid).ok()?;
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of_val(&info) as i32;
    let count = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut _,
            size,
        )
    };
    (count == size && info.pbi_pid == pid as u32 && info.pbi_start_tvsec > 0).then(|| {
        format!(
            "macos-start-us:{}:{}",
            info.pbi_start_tvsec, info.pbi_start_tvusec
        )
    })
}

#[cfg(windows)]
fn native_birth(pid: u32) -> Option<String> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FILETIME},
        System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut created: FILETIME = std::mem::zeroed();
        let mut exited: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        let success =
            GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) != 0;
        CloseHandle(handle);
        let creation = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
        (success && creation > 0).then(|| format!("windows-creation-100ns:{creation}"))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn native_birth(_: u32) -> Option<String> {
    None
}

/// Parent IDs are historical on Windows. A known child birth must be at
/// least as recent as its current parent's birth before adding a tree edge.
pub(crate) fn child_not_older_than_parent(parent: &str, child: &str) -> bool {
    fn value(identity: &str) -> Option<(&str, u128)> {
        let (domain, raw) = identity.split_once(':')?;
        let time = match domain {
            "linux-start-ticks" | "windows-creation-100ns" => raw.parse().ok()?,
            "macos-start-us" => {
                let (seconds, micros) = raw.split_once(':')?;
                let seconds = seconds.parse::<u128>().ok()?;
                let micros = micros.parse::<u128>().ok()?;
                if micros >= 1_000_000 {
                    return None;
                }
                seconds.checked_mul(1_000_000)?.checked_add(micros)?
            }
            _ => return None,
        };
        Some((domain, time))
    }
    match (value(parent), value(child)) {
        (Some((parent_domain, parent)), Some((child_domain, child))) => {
            parent_domain == child_domain && child >= parent
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn birth_identity_is_stable_for_this_process_and_zero_is_unknown() {
        let first = birth(std::process::id()).expect("own process birth must be readable");
        assert_eq!(birth(std::process::id()).as_ref(), Some(&first));
        assert_eq!(birth(0), None);
    }

    #[test]
    fn linux_stat_retains_tick_precision_and_parentheses_inside_command() {
        let line =
            |ticks| format!("7 (a ) b) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 {ticks} 99");
        assert_eq!(linux_start_ticks(&line(12301)), Some(12301));
        assert_eq!(linux_start_ticks(&line(12302)), Some(12302));
        assert_eq!(linux_start_ticks("truncated"), None);
    }
}
