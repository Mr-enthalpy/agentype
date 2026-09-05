//! Pin a process instance before any control side effect.
//!
//! Stale RuntimeHandle must never signal or reap another occupant of the
//! same PID. Linux uses pidfd; Windows keeps one PROCESS handle for verify
//! and act. If the platform cannot pin, control is Unavailable.
#![allow(unsafe_code)]

#[cfg(target_os = "linux")]
use super::process_stat;
use super::{require_deadline, AdapterDeadline, AdapterError, AdapterResult};
use std::time::Duration;

pub(crate) struct PinnedInstance {
    pid: u32,
    #[cfg(target_os = "linux")]
    pidfd: i32,
    #[cfg(windows)]
    handle: *mut core::ffi::c_void,
}

impl Drop for PinnedInstance {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        {
            unsafe {
                libc::close(self.pidfd);
            }
        }
        #[cfg(windows)]
        {
            extern "system" {
                fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
            }
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

/// Open a pinned identity for `pid` and confirm `expected_birth`.
/// `Ok(None)` means this is not that instance (gone or reused).
pub(crate) fn pin_instance(
    pid: u32,
    expected_birth: u64,
    deadline: &AdapterDeadline,
) -> AdapterResult<Option<PinnedInstance>> {
    require_deadline(deadline, "deadline exhausted before pinning process", None)?;
    #[cfg(target_os = "linux")]
    {
        pin_linux(pid, expected_birth, deadline)
    }
    #[cfg(windows)]
    {
        pin_windows(pid, expected_birth, deadline)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (pid, expected_birth);
        Err(AdapterError::unavailable(
            "cannot pin process instance on this OS",
        ))
    }
}

impl PinnedInstance {
    pub(crate) fn interrupt(&self) -> AdapterResult<()> {
        #[cfg(target_os = "linux")]
        {
            send_linux(self.pidfd, libc::SIGINT)
        }
        #[cfg(windows)]
        {
            interrupt_windows_pinned(self.pid)
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            Err(AdapterError::unavailable("interrupt unsupported"))
        }
    }

    pub(crate) fn kill(&self) -> AdapterResult<()> {
        #[cfg(target_os = "linux")]
        {
            send_linux(self.pidfd, libc::SIGKILL)
        }
        #[cfg(windows)]
        {
            kill_windows_pinned(self.handle)
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            Err(AdapterError::unavailable("terminate unsupported"))
        }
    }

    pub(crate) fn is_alive(&self, deadline: &AdapterDeadline) -> AdapterResult<bool> {
        require_deadline(deadline, "deadline exhausted before pinned liveness", None)?;
        #[cfg(target_os = "linux")]
        {
            Ok(match super::process_stat_linux(self.pid) {
                Some((state, _)) => !matches!(state, 'Z' | 'X' | 'x'),
                None => false,
            })
        }
        #[cfg(windows)]
        {
            Ok(windows_still_active(self.handle))
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            Ok(false)
        }
    }

    pub(crate) fn wait_exit(&self, deadline: &AdapterDeadline) -> AdapterResult<bool> {
        loop {
            if !self.is_alive(deadline)? {
                return Ok(true);
            }
            match super::wait_slice(deadline) {
                None => return Ok(false),
                Some(slice) => {
                    #[cfg(target_os = "linux")]
                    {
                        poll_pidfd(self.pidfd, slice);
                    }
                    #[cfg(windows)]
                    {
                        wait_windows(self.handle, slice);
                    }
                    #[cfg(not(any(windows, target_os = "linux")))]
                    {
                        std::thread::sleep(slice);
                    }
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn pin_linux(
    pid: u32,
    expected_birth: u64,
    deadline: &AdapterDeadline,
) -> AdapterResult<Option<PinnedInstance>> {
    let raw = match i32::try_from(pid) {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };
    let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, raw, 0i32) as i32 };
    if pidfd < 0 {
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        if errno == libc::ENOSYS {
            return Err(AdapterError::unavailable(
                "pidfd_open unsupported; refusing unpinned process control",
            ));
        }
        if errno == libc::ESRCH || errno == libc::ENOENT {
            return Ok(None);
        }
        return Err(AdapterError::unavailable("pidfd_open failed"));
    }
    let pinned = PinnedInstance { pid, pidfd };
    match process_stat(pid, deadline)? {
        Some((_, birth)) if birth == expected_birth => Ok(Some(pinned)),
        _ => Ok(None),
    }
}

#[cfg(target_os = "linux")]
fn send_linux(pidfd: i32, sig: i32) -> AdapterResult<()> {
    let rc = unsafe { libc::syscall(libc::SYS_pidfd_send_signal, pidfd, sig, 0isize, 0i32) as i32 };
    if rc == 0 {
        return Ok(());
    }
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    if errno == libc::ESRCH || errno == libc::ENOENT {
        return Ok(());
    }
    if errno == libc::ENOSYS {
        return Err(AdapterError::unavailable(
            "pidfd_send_signal unsupported; refusing unpinned process control",
        ));
    }
    Err(AdapterError::unavailable("pinned signal not delivered"))
}

#[cfg(target_os = "linux")]
fn poll_pidfd(pidfd: i32, slice: Duration) {
    let timeout_ms = i32::try_from(slice.as_millis()).unwrap_or(i32::MAX);
    let mut pfd = libc::pollfd {
        fd: pidfd,
        events: libc::POLLIN,
        revents: 0,
    };
    unsafe {
        libc::poll(&mut pfd, 1, timeout_ms);
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn pin_windows(
    pid: u32,
    expected_birth: u64,
    deadline: &AdapterDeadline,
) -> AdapterResult<Option<PinnedInstance>> {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const PROCESS_TERMINATE: u32 = 0x0001;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut core::ffi::c_void;
    }
    require_deadline(deadline, "deadline exhausted before OpenProcess", None)?;
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | SYNCHRONIZE,
            0,
            pid,
        )
    };
    if handle.is_null() {
        return Ok(None);
    }
    let pinned = PinnedInstance { pid, handle };
    let birth = match windows_creation(handle) {
        Some(b) => b,
        None => return Ok(None),
    };
    if birth != expected_birth {
        return Ok(None);
    }
    Ok(Some(pinned))
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn windows_creation(handle: *mut core::ffi::c_void) -> Option<u64> {
    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    extern "system" {
        fn GetProcessTimes(
            handle: *mut core::ffi::c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }
    let mut creation = FileTime { low: 0, high: 0 };
    let mut exit = FileTime { low: 0, high: 0 };
    let mut kernel = FileTime { low: 0, high: 0 };
    let mut user = FileTime { low: 0, high: 0 };
    let ok = unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
    if ok == 0 {
        return None;
    }
    Some(((creation.high as u64) << 32) | creation.low as u64)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn windows_still_active(handle: *mut core::ffi::c_void) -> bool {
    const STILL_ACTIVE: u32 = 259;
    extern "system" {
        fn GetExitCodeProcess(handle: *mut core::ffi::c_void, code: *mut u32) -> i32;
    }
    let mut code = 0u32;
    unsafe { GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn kill_windows_pinned(handle: *mut core::ffi::c_void) -> AdapterResult<()> {
    extern "system" {
        fn TerminateProcess(handle: *mut core::ffi::c_void, code: u32) -> i32;
    }
    unsafe {
        if TerminateProcess(handle, 1) == 0 && windows_still_active(handle) {
            return Err(AdapterError::unavailable("TerminateProcess failed"));
        }
    }
    Ok(())
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn interrupt_windows_pinned(pid: u32) -> AdapterResult<()> {
    const CTRL_BREAK_EVENT: u32 = 1;
    extern "system" {
        fn AttachConsole(pid: u32) -> i32;
        fn FreeConsole() -> i32;
        fn GenerateConsoleCtrlEvent(event: u32, group: u32) -> i32;
    }
    unsafe {
        let attached = AttachConsole(pid) != 0;
        let ok = GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) != 0;
        if attached {
            let _ = FreeConsole();
        }
        if ok {
            Ok(())
        } else {
            Err(AdapterError::unavailable(
                "interrupt not delivered (ctrl event failed)",
            ))
        }
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn wait_windows(handle: *mut core::ffi::c_void, slice: Duration) {
    const WAIT_OBJECT_0: u32 = 0;
    extern "system" {
        fn WaitForSingleObject(handle: *mut core::ffi::c_void, ms: u32) -> u32;
    }
    let ms = u32::try_from(slice.as_millis()).unwrap_or(u32::MAX);
    unsafe {
        let _ = WaitForSingleObject(handle, ms);
    }
    let _ = WAIT_OBJECT_0;
}
