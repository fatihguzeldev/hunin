use crate::process::{Counter, Error, ErrorKind, MemoryReading, ProcessInstance, Result};
use std::{mem, path::PathBuf};

fn boot_identity() -> Result<String> {
    let mut size = 0;
    let key = c"kern.bootsessionuuid";
    if unsafe {
        libc::sysctlbyname(
            key.as_ptr(),
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    if size == 0 || size > 4096 {
        return Err(Error::invalid("invalid boot uuid size"));
    }
    let mut buffer = vec![0_u8; size];
    if unsafe {
        libc::sysctlbyname(
            key.as_ptr(),
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let end = buffer.iter().position(|&x| x == 0).unwrap_or(buffer.len());
    String::from_utf8(buffer[..end].to_vec()).map_err(|_| Error::invalid("boot uuid is not utf8"))
}
fn bsd_info(pid: u32) -> Result<libc::proc_bsdinfo> {
    let mut value: libc::proc_bsdinfo = unsafe { mem::zeroed() };
    let size = mem::size_of::<libc::proc_bsdinfo>();
    let n = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut value as *mut libc::proc_bsdinfo).cast(),
            size as i32,
        )
    };
    if n != size as i32 {
        let error = std::io::Error::last_os_error();
        return Err(Error::new(
            if error.raw_os_error() == Some(libc::EPERM)
                || error.raw_os_error() == Some(libc::EACCES)
            {
                ErrorKind::AccessDenied
            } else if error.raw_os_error() == Some(libc::ESRCH) || n == 0 {
                ErrorKind::ProcessGone
            } else {
                ErrorKind::Io
            },
            format!("proc_pidinfo({pid}) returned {n}: {error}"),
        ));
    }
    Ok(value)
}
fn usage(pid: u32) -> Result<libc::rusage_info_v4> {
    let mut value: libc::rusage_info_v4 = unsafe { mem::zeroed() };
    let n = unsafe {
        libc::proc_pid_rusage(
            pid as i32,
            libc::RUSAGE_INFO_V4,
            (&mut value as *mut libc::rusage_info_v4).cast(),
        )
    };
    if n != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(value)
}
pub(crate) fn inspect(pid: u32) -> Result<ProcessInstance> {
    let info = bsd_info(pid)?;
    let rusage = usage(pid)?;
    let mut path = vec![0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let length =
        unsafe { libc::proc_pidpath(pid as i32, path.as_mut_ptr().cast(), path.len() as u32) };
    let executable = if length > 0 {
        let end = path.iter().position(|&c| c == 0).unwrap_or(path.len());
        Some(PathBuf::from(
            String::from_utf8_lossy(&path[..end]).into_owned(),
        ))
    } else {
        None
    };
    let bytes: Vec<u8> = info
        .pbi_comm
        .iter()
        .map(|&c| c as u8)
        .take_while(|&c| c != 0)
        .collect();
    let name = String::from_utf8_lossy(&bytes).into_owned();
    let after = bsd_info(pid)?;
    if info.pbi_start_tvsec != after.pbi_start_tvsec
        || info.pbi_start_tvusec != after.pbi_start_tvusec
    {
        return Err(Error::new(
            ErrorKind::InstanceChanged,
            "process changed during identity read",
        ));
    }
    Ok(ProcessInstance {
        platform: "macos".into(),
        pid,
        parent_pid: Some(info.pbi_ppid),
        boot_identity: Some(boot_identity()?),
        namespace: None,
        start_identity: format!(
            "{}:{}:{}",
            info.pbi_start_tvsec, info.pbi_start_tvusec, rusage.ri_proc_start_abstime
        ),
        executable,
        name,
    })
}
pub(crate) fn pids() -> Result<Vec<u32>> {
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut capacity = count as usize + 64;
    for _ in 0..4 {
        if capacity > 1_000_000 {
            return Err(Error::new(
                ErrorKind::LimitExceeded,
                "process discovery exceeds one million ids",
            ));
        }
        let mut ids = vec![0_i32; capacity];
        let count = unsafe {
            libc::proc_listallpids(
                ids.as_mut_ptr().cast(),
                (ids.len() * mem::size_of::<i32>()) as i32,
            )
        };
        if count < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if count as usize >= capacity {
            capacity *= 2;
            continue;
        }
        ids.truncate(count as usize);
        return Ok(ids
            .into_iter()
            .filter(|&pid| pid > 0)
            .map(|pid| pid as u32)
            .collect());
    }
    Err(Error::new(
        ErrorKind::CollectorFailure,
        "process list kept growing during discovery",
    ))
}
fn timebase() -> Result<mach2::mach_time::mach_timebase_info> {
    let mut info = mach2::mach_time::mach_timebase_info { numer: 0, denom: 0 };
    if unsafe { mach2::mach_time::mach_timebase_info(&mut info) } != 0
        || info.numer == 0
        || info.denom == 0
    {
        return Err(Error::new(
            ErrorKind::CollectorFailure,
            "mach timebase unavailable",
        ));
    }
    Ok(info)
}
pub(crate) fn resources(pid: u32) -> Result<(Counter, Counter, Vec<MemoryReading>)> {
    let info = usage(pid)?;
    let scale = timebase()?;
    Ok((
        Counter::new(
            info.ri_user_time,
            scale.numer as u64,
            scale.denom as u64,
            "mach_ticks",
        )?,
        Counter::new(
            info.ri_system_time,
            scale.numer as u64,
            scale.denom as u64,
            "mach_ticks",
        )?,
        vec![
            MemoryReading {
                definition: "macos.rusage_info_v4.resident_size".into(),
                bytes: info.ri_resident_size,
                approximate: false,
            },
            MemoryReading {
                definition: "macos.rusage_info_v4.phys_footprint".into(),
                bytes: info.ri_phys_footprint,
                approximate: false,
            },
        ],
    ))
}
pub(crate) fn monotonic_ns() -> Result<u64> {
    let scale = timebase()?;
    Counter::new(
        unsafe { mach2::mach_time::mach_absolute_time() },
        scale.numer as u64,
        scale.denom as u64,
        "mach_ticks",
    )?
    .nanoseconds()
}
pub(crate) fn clock_identity() -> Result<String> {
    Ok(format!("macos.mach_absolute_time:{}", boot_identity()?))
}
