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

#[derive(Debug)]
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

#[derive(Debug)]
pub(crate) enum PinOutcome {
    Pinned(PinnedInstance),
    /// PID does not exist: the identified instance has ended (or never ran).
    Gone,
    /// A process exists at this PID but birth does not match.
    Mismatch,
}

/// Result of classifying a Windows `OpenProcess` return + `GetLastError`.
/// Compiled on Windows production and on every-OS tests.
#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowsOpen {
    Present,
    Gone,
}

#[cfg(any(windows, test))]
const ERROR_ACCESS_DENIED: i32 = 5;
#[cfg(any(windows, test))]
const ERROR_INVALID_PARAMETER: i32 = 87;
#[cfg(any(windows, test))]
const STILL_ACTIVE: u32 = 259;

/// Only `ERROR_INVALID_PARAMETER` is positive PID-absence. Access and
/// unknown failures must not become `Gone`.
#[cfg(any(windows, test))]
pub(crate) fn classify_open_process(
    handle_is_null: bool,
    last_error: i32,
) -> AdapterResult<WindowsOpen> {
    if !handle_is_null {
        return Ok(WindowsOpen::Present);
    }
    match last_error {
        ERROR_INVALID_PARAMETER => Ok(WindowsOpen::Gone),
        ERROR_ACCESS_DENIED => Err(AdapterError::unavailable("OpenProcess access denied")),
        _ => Err(AdapterError::other("OpenProcess failed")),
    }
}

/// `GetProcessTimes` failure is an observation error, not process absence.
#[cfg(any(windows, test))]
pub(crate) fn classify_process_times(ok: bool, creation: u64) -> AdapterResult<u64> {
    if !ok {
        return Err(AdapterError::other("GetProcessTimes failed"));
    }
    Ok(creation)
}

/// `GetExitCodeProcess` failure is an observation error, not "not alive".
#[cfg(any(windows, test))]
pub(crate) fn classify_still_active(query_ok: bool, code: u32) -> AdapterResult<bool> {
    if !query_ok {
        return Err(AdapterError::other("GetExitCodeProcess failed"));
    }
    Ok(code == STILL_ACTIVE)
}

/// Terminate + query failure must not become terminate success.
#[cfg(any(windows, test))]
pub(crate) fn classify_terminate(
    terminate_ok: bool,
    still: AdapterResult<bool>,
) -> AdapterResult<()> {
    if terminate_ok {
        return Ok(());
    }
    match still {
        Ok(true) => Err(AdapterError::unavailable("TerminateProcess failed")),
        Ok(false) => Ok(()),
        Err(err) => Err(err),
    }
}

/// Open a pinned identity for `pid` and confirm `expected_birth`.
pub(crate) fn pin_instance(
    pid: u32,
    expected_birth: u64,
    deadline: &AdapterDeadline,
) -> AdapterResult<PinOutcome> {
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
    pub(crate) fn interrupt(&self, deadline: &AdapterDeadline) -> AdapterResult<()> {
        require_deadline(deadline, "deadline exhausted before interrupt", None)?;
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

    pub(crate) fn kill(&self, deadline: &AdapterDeadline) -> AdapterResult<()> {
        require_deadline(deadline, "deadline exhausted before terminate", None)?;
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
            Ok(match super::process_stat_linux(self.pid)? {
                Some((state, _)) => !matches!(state, 'Z' | 'X' | 'x'),
                None => false,
            })
        }
        #[cfg(windows)]
        {
            windows_still_active(self.handle)
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
) -> AdapterResult<PinOutcome> {
    let raw = match i32::try_from(pid) {
        Ok(p) => p,
        Err(_) => return Ok(PinOutcome::Gone),
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
            return Ok(PinOutcome::Gone);
        }
        return Err(AdapterError::unavailable("pidfd_open failed"));
    }
    let pinned = PinnedInstance { pid, pidfd };
    match process_stat(pid, deadline)? {
        Some((_, birth)) if birth == expected_birth => Ok(PinOutcome::Pinned(pinned)),
        Some(_) => Ok(PinOutcome::Mismatch),
        None => Ok(PinOutcome::Gone),
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
) -> AdapterResult<PinOutcome> {
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
        let last = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        return match classify_open_process(true, last)? {
            WindowsOpen::Gone => Ok(PinOutcome::Gone),
            WindowsOpen::Present => unreachable!("null OpenProcess is not Present"),
        };
    }
    let pinned = PinnedInstance { pid, handle };
    let birth = windows_creation(handle)?;
    if birth != expected_birth {
        return Ok(PinOutcome::Mismatch);
    }
    Ok(PinOutcome::Pinned(pinned))
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn windows_creation(handle: *mut core::ffi::c_void) -> AdapterResult<u64> {
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
    classify_process_times(
        ok != 0,
        ((creation.high as u64) << 32) | creation.low as u64,
    )
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn windows_still_active(handle: *mut core::ffi::c_void) -> AdapterResult<bool> {
    extern "system" {
        fn GetExitCodeProcess(handle: *mut core::ffi::c_void, code: *mut u32) -> i32;
    }
    let mut code = 0u32;
    let query_ok = unsafe { GetExitCodeProcess(handle, &mut code) != 0 };
    classify_still_active(query_ok, code)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn kill_windows_pinned(handle: *mut core::ffi::c_void) -> AdapterResult<()> {
    extern "system" {
        fn TerminateProcess(handle: *mut core::ffi::c_void, code: u32) -> i32;
    }
    let terminate_ok = unsafe { TerminateProcess(handle, 1) != 0 };
    classify_terminate(terminate_ok, windows_still_active(handle))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process_birth;
    use agentype_adapter_api::AdapterErrorKind;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn spawn_hang() -> std::process::Child {
        #[cfg(windows)]
        {
            Command::new("ping")
                .args(["-n", "30", "127.0.0.1"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("ping")
        }
        #[cfg(not(windows))]
        {
            Command::new("sleep")
                .arg("30")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("sleep")
        }
    }

    #[test]
    fn expired_deadline_after_pin_does_not_kill() {
        let mut child = spawn_hang();
        let pid = child.id();
        std::thread::sleep(Duration::from_millis(80));
        let long = AdapterDeadline::after(Duration::from_secs(2)).unwrap();
        let birth = process_birth(pid, &long).expect("birth");
        let pinned = match pin_instance(pid, birth, &long).unwrap() {
            PinOutcome::Pinned(p) => p,
            other => panic!("expected pin, got {other:?}"),
        };
        let expired = AdapterDeadline::from_instant(Instant::now() - Duration::from_secs(1));
        let err = pinned.kill(&expired).unwrap_err();
        assert_eq!(err.kind(), AdapterErrorKind::DeadlineExceeded);
        assert!(
            matches!(
                pin_instance(pid, birth, &long).unwrap(),
                PinOutcome::Pinned(_)
            ),
            "expired kill must not terminate the pinned instance"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn open_process_invalid_parameter_is_gone() {
        assert_eq!(
            classify_open_process(false, 0).unwrap(),
            WindowsOpen::Present
        );
        assert_eq!(
            classify_open_process(true, ERROR_INVALID_PARAMETER).unwrap(),
            WindowsOpen::Gone
        );
    }

    #[test]
    fn open_process_access_denied_is_unavailable_not_gone() {
        let err = classify_open_process(true, ERROR_ACCESS_DENIED).unwrap_err();
        assert_eq!(err.kind(), AdapterErrorKind::Unavailable);
    }

    #[test]
    fn open_process_unknown_failure_is_other_not_gone() {
        let err = classify_open_process(true, 1234).unwrap_err();
        assert_eq!(err.kind(), AdapterErrorKind::Other);
    }

    #[test]
    fn get_process_times_failure_is_adapter_error_not_gone() {
        let err = classify_process_times(false, 0).unwrap_err();
        assert_eq!(err.kind(), AdapterErrorKind::Other);
        assert_eq!(classify_process_times(true, 99).unwrap(), 99);
    }

    #[test]
    fn get_exit_code_failure_is_adapter_error_not_dead() {
        let err = classify_still_active(false, 0).unwrap_err();
        assert_eq!(err.kind(), AdapterErrorKind::Other);
        assert!(classify_still_active(true, STILL_ACTIVE).unwrap());
        assert!(!classify_still_active(true, 1).unwrap());
    }

    #[test]
    fn terminate_plus_query_failure_is_adapter_error_not_success() {
        let err = classify_terminate(false, Err(AdapterError::other("GetExitCodeProcess failed")))
            .unwrap_err();
        assert_eq!(err.kind(), AdapterErrorKind::Other);
        let live = classify_terminate(false, Ok(true)).unwrap_err();
        assert_eq!(live.kind(), AdapterErrorKind::Unavailable);
        classify_terminate(false, Ok(false)).unwrap();
        classify_terminate(true, Err(AdapterError::other("ignored"))).unwrap();
    }
}
