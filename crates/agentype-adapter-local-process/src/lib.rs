//! Local-process Execution Environment Adapter.
//!
//! This crate proves that a real OS process can attach to the frozen M5.6
//! contract without leaking model, provider, prompt orchestration, or
//! harness ownership into the Scheduler. The executable is user-owned
//! configuration (`target_options.command`); Core never sees it.
//!
//! Platform FFI is confined to pid liveness, process birth, cancellable
//! stdin, interrupt, and pid-level terminate. No helper threads.

use agentype_adapter_api::{
    AdapterDeadline, AdapterError, AdapterResult, ExecutionAdapter, ExecutionObservation,
    ExecutionOutcome, ExecutionRequest, ImportableAdapter, RuntimeHandle, StartObservation,
};
use agentype_core::{ExecutionState, RequestId};
use agentype_execution_config::AdapterBindingKey;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

mod pin;

/// Hidden helper for the Linux pidfd PID-recycle regression binary.
#[cfg(target_os = "linux")]
#[doc(hidden)]
pub fn pidfd_recycle_experiment() -> i32 {
    pin::recycle_pid_experiment()
}

/// Frozen adapter_kind for this environment domain: this host's process
/// table as spawned by this runtime. Not a vendor/model name.
pub const ADAPTER_KIND: &str = "local_process";

/// Collectable stdout is bounded. Arbitrary file slurps are not evidence.
pub const MAX_STDOUT_BYTES: usize = 256 * 1024;

const HANDLE_VERSION: i64 = 1;
const WAIT_SLICE: Duration = Duration::from_millis(10);
const READ_CHUNK: usize = 8 * 1024;

struct LiveProcess {
    child: Child,
    birth: u64,
}

/// Spawn and control a user-configured local executable.
pub struct LocalProcessAgentAdapter {
    live: Mutex<HashMap<u32, LiveProcess>>,
    binding_key: AdapterBindingKey,
}

impl Default for LocalProcessAgentAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalProcessAgentAdapter {
    /// Install this adapter only when the host domain identity can be proved.
    /// Missing boot, PID namespace, or mount namespace is Unavailable — never
    /// a synthetic `unknown-*` key.
    pub fn try_new() -> AdapterResult<Self> {
        Ok(Self {
            live: Mutex::new(HashMap::new()),
            binding_key: capture_domain_key()?,
        })
    }

    pub fn new() -> Self {
        Self::try_new().expect("local-process domain identity is required")
    }

    pub fn binding_key(&self) -> &AdapterBindingKey {
        &self.binding_key
    }
}

impl ImportableAdapter for LocalProcessAgentAdapter {
    fn import_kind(&self) -> &str {
        ADAPTER_KIND
    }

    fn import_binding_key(&self) -> AdapterBindingKey {
        self.binding_key.clone()
    }

    fn import_attempt_isolation(&self) -> bool {
        false
    }
}

fn capture_domain_key() -> AdapterResult<AdapterBindingKey> {
    static DOMAIN: OnceLock<AdapterResult<AdapterBindingKey>> = OnceLock::new();
    DOMAIN.get_or_init(compute_domain_key).clone()
}

fn compute_domain_key() -> AdapterResult<AdapterBindingKey> {
    #[cfg(target_os = "linux")]
    {
        let boot = read_trimmed("/proc/sys/kernel/random/boot_id")
            .ok_or_else(|| AdapterError::unavailable("linux boot_id unavailable"))?;
        let pid_ns = read_ns("pid")?;
        let mnt_ns = read_ns("mnt")?;
        linux_domain_key(&boot, &pid_ns, &mnt_ns)
    }
    #[cfg(windows)]
    {
        let host = std::env::var("COMPUTERNAME")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AdapterError::unavailable("windows COMPUTERNAME unavailable"))?;
        let boot = windows_boot_identifier()
            .ok_or_else(|| AdapterError::unavailable("windows boot identifier unavailable"))?;
        AdapterBindingKey::new(format!("win:{host}:{boot}"))
            .map_err(|_| AdapterError::unavailable("windows domain key invalid"))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Err(AdapterError::unavailable(
            "local_process adapter unsupported on this OS",
        ))
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_domain_key(boot: &str, pid_ns: &str, mnt_ns: &str) -> AdapterResult<AdapterBindingKey> {
    if boot.trim().is_empty() || pid_ns.trim().is_empty() || mnt_ns.trim().is_empty() {
        return Err(AdapterError::unavailable(
            "linux domain identity components cannot be empty",
        ));
    }
    if boot.contains("unknown") || pid_ns.contains("unknown") || mnt_ns.contains("unknown") {
        return Err(AdapterError::unavailable(
            "linux domain identity cannot use unknown placeholders",
        ));
    }
    AdapterBindingKey::new(format!("linux:{boot}:{pid_ns}:{mnt_ns}"))
        .map_err(|_| AdapterError::unavailable("linux domain key invalid"))
}

#[cfg(target_os = "linux")]
fn read_ns(kind: &str) -> AdapterResult<String> {
    let path = format!("/proc/self/ns/{kind}");
    std::fs::read_link(&path)
        .ok()
        .and_then(|p| p.to_str().map(str::to_string))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AdapterError::unavailable(format!("linux {kind} namespace unavailable")))
}

#[cfg(target_os = "linux")]
fn read_trimmed(path: &str) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn windows_boot_identifier() -> Option<String> {
    #[repr(C)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }
    #[repr(C)]
    struct SystemBootEnvironmentInformation {
        boot_identifier: Guid,
        firmware_type: u32,
        boot_flags: u64,
    }
    const SYSTEM_BOOT_ENVIRONMENT_INFORMATION: u32 = 90;
    #[link(name = "ntdll")]
    extern "system" {
        fn NtQuerySystemInformation(
            class: u32,
            info: *mut SystemBootEnvironmentInformation,
            len: u32,
            ret_len: *mut u32,
        ) -> i32;
    }
    let mut info = std::mem::MaybeUninit::<SystemBootEnvironmentInformation>::zeroed();
    let status = unsafe {
        NtQuerySystemInformation(
            SYSTEM_BOOT_ENVIRONMENT_INFORMATION,
            info.as_mut_ptr(),
            std::mem::size_of::<SystemBootEnvironmentInformation>() as u32,
            std::ptr::null_mut(),
        )
    };
    if status != 0 {
        return None;
    }
    let info = unsafe { info.assume_init() };
    let g = info.boot_identifier;
    Some(format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        g.data1,
        g.data2,
        g.data3,
        g.data4[0],
        g.data4[1],
        g.data4[2],
        g.data4[3],
        g.data4[4],
        g.data4[5],
        g.data4[6],
        g.data4[7]
    ))
}

impl Drop for LocalProcessAgentAdapter {
    fn drop(&mut self) {
        // Dropping adapter ownership is not terminate_execution.
        // `Child` has no Drop (does not wait or kill). Reap already-dead
        // children with WNOHANG; leave running processes alive.
        if let Ok(mut live) = self.live.lock() {
            for (_, mut proc) in live.drain() {
                let _ = proc.child.try_wait();
            }
        }
    }
}

#[derive(Debug)]
struct ProcessSpec {
    command: String,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    env: Vec<(String, String)>,
}

#[derive(Debug)]
struct ParsedHandle {
    pid: u32,
    birth: u64,
    request_id: String,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

fn spec_from_options(options: &Value) -> AdapterResult<ProcessSpec> {
    let command = options
        .get("command")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AdapterError::unavailable("target_options.command is required"))?;
    let args = match options.get("args") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let Some(s) = item.as_str() else {
                    return Err(AdapterError::protocol(
                        "target_options.args must be an array of strings",
                    ));
                };
                out.push(s.to_string());
            }
            out
        }
        Some(_) => {
            return Err(AdapterError::protocol(
                "target_options.args must be an array of strings",
            ));
        }
    };
    let cwd = match options.get("cwd") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if !s.trim().is_empty() => Some(PathBuf::from(s)),
        Some(_) => {
            return Err(AdapterError::protocol(
                "target_options.cwd must be a string path",
            ));
        }
    };
    let env = match options.get("env") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Object(map)) => {
            let mut out = Vec::with_capacity(map.len());
            for (k, v) in map {
                let Some(s) = v.as_str() else {
                    return Err(AdapterError::protocol(
                        "target_options.env values must be strings",
                    ));
                };
                out.push((k.clone(), s.to_string()));
            }
            out
        }
        Some(_) => {
            return Err(AdapterError::protocol(
                "target_options.env must be an object of strings",
            ));
        }
    };
    Ok(ProcessSpec {
        command: command.to_string(),
        args,
        cwd,
        env,
    })
}

fn encode_handle(parsed: &ParsedHandle) -> RuntimeHandle {
    RuntimeHandle(json!({
        "v": HANDLE_VERSION,
        "kind": ADAPTER_KIND,
        "pid": parsed.pid,
        "birth": parsed.birth,
        "request_id": parsed.request_id,
        "stdout": parsed.stdout_path.to_string_lossy(),
        "stderr": parsed.stderr_path.to_string_lossy(),
    }))
}

fn parse_handle(handle: &RuntimeHandle) -> AdapterResult<ParsedHandle> {
    let obj = handle
        .0
        .as_object()
        .ok_or_else(|| AdapterError::protocol("local_process handle must be a JSON object"))?;
    let v = obj.get("v").and_then(Value::as_i64).unwrap_or(0);
    if v != HANDLE_VERSION {
        return Err(AdapterError::protocol(
            "unsupported local_process handle version",
        ));
    }
    let kind = obj.get("kind").and_then(Value::as_str).unwrap_or("");
    if kind != ADAPTER_KIND {
        return Err(AdapterError::protocol(
            "handle kind is not local_process; no adapter fallback",
        ));
    }
    let pid = obj
        .get("pid")
        .and_then(Value::as_u64)
        .ok_or_else(|| AdapterError::protocol("handle.pid missing"))?;
    let pid = u32::try_from(pid).map_err(|_| AdapterError::protocol("handle.pid out of range"))?;
    let birth = obj
        .get("birth")
        .and_then(Value::as_u64)
        .ok_or_else(|| AdapterError::protocol("handle.birth missing"))?;
    let request_id = obj
        .get("request_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AdapterError::protocol("handle.request_id missing"))?
        .to_string();
    let stdout = obj
        .get("stdout")
        .and_then(Value::as_str)
        .ok_or_else(|| AdapterError::protocol("handle.stdout missing"))?;
    let stderr = obj
        .get("stderr")
        .and_then(Value::as_str)
        .ok_or_else(|| AdapterError::protocol("handle.stderr missing"))?;
    Ok(ParsedHandle {
        pid,
        birth,
        request_id,
        stdout_path: PathBuf::from(stdout),
        stderr_path: PathBuf::from(stderr),
    })
}

fn diagnostic(msg: &'static str) -> AdapterError {
    AdapterError::other(msg)
}

fn deadline_exceeded_hint(msg: &'static str, handle: RuntimeHandle) -> AdapterError {
    AdapterError::deadline_exceeded(msg).with_handle_hint(handle)
}

fn require_deadline(
    deadline: &AdapterDeadline,
    msg: &'static str,
    hint: Option<&RuntimeHandle>,
) -> AdapterResult<()> {
    if deadline.is_expired() {
        let err = AdapterError::deadline_exceeded(msg);
        return Err(match hint {
            Some(h) => err.with_handle_hint(h.clone()),
            None => err,
        });
    }
    Ok(())
}

fn wait_slice(deadline: &AdapterDeadline) -> Option<Duration> {
    if deadline.is_expired() {
        return None;
    }
    let rem = deadline.remaining();
    if rem.is_zero() {
        None
    } else if rem < WAIT_SLICE {
        Some(rem)
    } else {
        Some(WAIT_SLICE)
    }
}

/// Wait until the child is reaped or the deadline expires.
///
/// `true` means the process exited (including Unix signal death, where
/// `ExitStatus::code()` is `None`). `false` means the deadline ended while
/// the child was still running — not proof of TERMINATED or quiescence.
fn wait_child_exit(child: &mut Child, deadline: &AdapterDeadline) -> AdapterResult<bool> {
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => return Ok(true),
            Ok(None) => {}
            Err(_) => return Err(AdapterError::other("wait on child failed")),
        }
        match wait_slice(deadline) {
            None => return Ok(false),
            Some(slice) => thread::sleep(slice),
        }
    }
}

fn wait_instance_exit(pid: u32, birth: u64, deadline: &AdapterDeadline) -> AdapterResult<bool> {
    match pin::pin_instance(pid, birth, deadline)? {
        pin::PinOutcome::Gone | pin::PinOutcome::Mismatch => Ok(true),
        pin::PinOutcome::Pinned(pinned) => pinned.wait_exit(deadline),
    }
}

fn write_stdin_deadline(
    stdin: ChildStdin,
    bytes: &[u8],
    deadline: &AdapterDeadline,
) -> AdapterResult<()> {
    #[cfg(windows)]
    {
        write_stdin_windows(stdin, bytes, deadline)
    }
    #[cfg(unix)]
    {
        write_stdin_unix(stdin, bytes, deadline)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (stdin, bytes, deadline);
        Err(AdapterError::unavailable(
            "local_process stdin I/O unsupported on this platform",
        ))
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn write_stdin_windows(
    stdin: ChildStdin,
    bytes: &[u8],
    deadline: &AdapterDeadline,
) -> AdapterResult<()> {
    use std::os::windows::io::AsRawHandle;
    const PIPE_NOWAIT: u32 = 0x0000_0001;
    const ERROR_NO_DATA: u32 = 232;
    const ERROR_BROKEN_PIPE: u32 = 109;
    extern "system" {
        fn SetNamedPipeHandleState(
            h: *mut core::ffi::c_void,
            mode: *mut u32,
            max_col: *mut u32,
            timeout: *mut u32,
        ) -> i32;
        fn WriteFile(
            h: *mut core::ffi::c_void,
            buf: *const u8,
            n: u32,
            written: *mut u32,
            overlapped: *mut core::ffi::c_void,
        ) -> i32;
        fn GetLastError() -> u32;
    }
    let handle = stdin.as_raw_handle();
    // SAFETY: handle is the owned ChildStdin pipe; PIPE_NOWAIT and WriteFile
    // use it until stdin is dropped at the end of this function.
    unsafe {
        let mut mode = PIPE_NOWAIT;
        if SetNamedPipeHandleState(
            handle,
            &mut mode,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(AdapterError::other("cannot set stdin PIPE_NOWAIT"));
        }
        let mut off = 0usize;
        while off < bytes.len() {
            if deadline.is_expired() {
                return Err(AdapterError::deadline_exceeded("stdin write blocked"));
            }
            let want = (bytes.len() - off).min(u32::MAX as usize) as u32;
            let mut written = 0u32;
            let ok = WriteFile(
                handle,
                bytes.as_ptr().add(off),
                want,
                &mut written,
                std::ptr::null_mut(),
            );
            if ok != 0 && written > 0 {
                off += written as usize;
                continue;
            }
            let err = if ok == 0 {
                GetLastError()
            } else {
                ERROR_NO_DATA
            };
            if err == ERROR_BROKEN_PIPE {
                return Err(AdapterError::other("stdin write failed"));
            }
            if err != ERROR_NO_DATA && ok == 0 && written == 0 && err != 0 {
                // Would-block or empty write: poll remaining.
            }
            match wait_slice(deadline) {
                None => return Err(AdapterError::deadline_exceeded("stdin write blocked")),
                Some(slice) => thread::sleep(slice),
            }
        }
    }
    drop(stdin);
    Ok(())
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn write_stdin_unix(
    stdin: ChildStdin,
    bytes: &[u8],
    deadline: &AdapterDeadline,
) -> AdapterResult<()> {
    use std::os::unix::io::AsRawFd;
    const F_GETFL: i32 = 3;
    const F_SETFL: i32 = 4;
    const O_NONBLOCK: i32 = 0o4000;
    const POLLOUT: i16 = 0x0004;
    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }
    extern "C" {
        fn fcntl(fd: i32, cmd: i32, arg: i32) -> i32;
        fn poll(fds: *mut PollFd, nfds: u64, timeout: i32) -> i32;
        fn write(fd: i32, buf: *const u8, n: usize) -> isize;
    }
    let fd = stdin.as_raw_fd();
    // SAFETY: fd is the owned ChildStdin; O_NONBLOCK + poll + write stay on
    // this thread until stdin is dropped.
    unsafe {
        let flags = fcntl(fd, F_GETFL, 0);
        if flags < 0 || fcntl(fd, F_SETFL, flags | O_NONBLOCK) < 0 {
            return Err(AdapterError::other("cannot set stdin O_NONBLOCK"));
        }
        let mut off = 0usize;
        while off < bytes.len() {
            if deadline.is_expired() {
                return Err(AdapterError::deadline_exceeded("stdin write blocked"));
            }
            let n = write(fd, bytes.as_ptr().add(off), bytes.len() - off);
            if n > 0 {
                off += n as usize;
                continue;
            }
            let timeout_ms = match wait_slice(deadline) {
                None => return Err(AdapterError::deadline_exceeded("stdin write blocked")),
                Some(slice) => i32::try_from(slice.as_millis()).unwrap_or(i32::MAX),
            };
            let mut pfd = PollFd {
                fd,
                events: POLLOUT,
                revents: 0,
            };
            let _ = poll(&mut pfd, 1, timeout_ms);
        }
    }
    drop(stdin);
    Ok(())
}

fn kill_child(child: &mut Child) {
    let _ = child.kill();
}

pub(crate) fn process_birth(pid: u32, deadline: &AdapterDeadline) -> AdapterResult<u64> {
    require_deadline(
        deadline,
        "deadline exhausted before process birth probe",
        None,
    )?;
    process_stat(pid, deadline)?
        .map(|(_, birth)| birth)
        .ok_or_else(|| AdapterError::unavailable("cannot read process birth identity"))
}

fn instance_presence(
    pid: u32,
    birth: u64,
    deadline: &AdapterDeadline,
) -> AdapterResult<InstancePresence> {
    match pin::pin_instance(pid, birth, deadline)? {
        pin::PinOutcome::Gone => Ok(InstancePresence::Ended),
        pin::PinOutcome::Mismatch => Ok(InstancePresence::Absent),
        pin::PinOutcome::Pinned(pinned) => {
            if pinned.is_alive(deadline)? {
                Ok(InstancePresence::Running)
            } else {
                Ok(InstancePresence::Ended)
            }
        }
    }
}

fn process_stat(pid: u32, deadline: &AdapterDeadline) -> AdapterResult<Option<(char, u64)>> {
    require_deadline(
        deadline,
        "deadline exhausted before process stat probe",
        None,
    )?;
    #[cfg(windows)]
    {
        Ok(process_birth_windows(pid)?.map(|b| ('R', b)))
    }
    #[cfg(target_os = "linux")]
    {
        process_stat_linux(pid)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = pid;
        Ok(None)
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn process_birth_windows(pid: u32) -> AdapterResult<Option<u64>> {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut core::ffi::c_void;
        fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
        fn GetExitCodeProcess(handle: *mut core::ffi::c_void, code: *mut u32) -> i32;
        fn GetProcessTimes(
            handle: *mut core::ffi::c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            let last = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            return match pin::classify_open_process(true, last)? {
                pin::WindowsOpen::Gone => Ok(None),
                pin::WindowsOpen::Present => unreachable!("null OpenProcess is not Present"),
            };
        }
        let mut code = 0u32;
        let query_ok = GetExitCodeProcess(handle, &mut code) != 0;
        let still = match pin::classify_still_active(query_ok, code) {
            Ok(still) => still,
            Err(err) => {
                CloseHandle(handle);
                return Err(err);
            }
        };
        let mut creation = FileTime { low: 0, high: 0 };
        let mut exit = FileTime { low: 0, high: 0 };
        let mut kernel = FileTime { low: 0, high: 0 };
        let mut user = FileTime { low: 0, high: 0 };
        let times_ok =
            GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) != 0;
        CloseHandle(handle);
        let birth = pin::classify_process_times(
            times_ok,
            ((creation.high as u64) << 32) | creation.low as u64,
        )?;
        if !still {
            return Ok(None);
        }
        Ok(Some(birth))
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn process_stat_linux(pid: u32) -> AdapterResult<Option<(char, u64)>> {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(text) => stat_identity_from_text(&text).map(Some),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(AdapterError::other("process stat probe failed")),
    }
}

/// Parse `/proc/<pid>/stat` identity. Malformed text is an observation
/// error, not process absence.
#[cfg(any(target_os = "linux", test))]
fn stat_identity_from_text(text: &str) -> AdapterResult<(char, u64)> {
    parse_stat_identity(text).ok_or_else(|| AdapterError::other("process stat parse failed"))
}

#[cfg(any(target_os = "linux", test))]
fn parse_stat_identity(stat: &str) -> Option<(char, u64)> {
    let close = stat.rfind(')')?;
    let rest = stat.get(close + 1..)?.trim_start();
    let mut fields = rest.split_whitespace();
    let state = fields.next()?.chars().next()?;
    let starttime = fields.nth(18)?.parse().ok()?;
    Some((state, starttime))
}

fn spawn_child(
    spec: &ProcessSpec,
    stdout_path: &Path,
    stderr_path: &Path,
    deadline: &AdapterDeadline,
) -> AdapterResult<Child> {
    require_deadline(
        deadline,
        "start deadline exhausted before stdout file",
        None,
    )?;
    let stdout = File::create(stdout_path).map_err(|_| diagnostic("cannot create stdout file"))?;
    require_deadline(
        deadline,
        "start deadline exhausted before stderr file",
        None,
    )?;
    let stderr = File::create(stderr_path).map_err(|_| diagnostic("cannot create stderr file"))?;
    require_deadline(deadline, "start deadline exhausted before spawn", None)?;
    let mut cmd = Command::new(&spec.command);
    cmd.args(&spec.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.spawn()
        .map_err(|_| AdapterError::unavailable("failed to spawn local process"))
}

fn read_stdout_bounded(path: &Path, deadline: &AdapterDeadline) -> AdapterResult<String> {
    require_deadline(deadline, "collect stdout read: deadline exhausted", None)?;
    let mut f = File::open(path).map_err(|_| AdapterError::protocol("cannot open stdout file"))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; READ_CHUNK];
    loop {
        require_deadline(deadline, "collect stdout read: deadline exhausted", None)?;
        match f.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if buf.len().saturating_add(n) > MAX_STDOUT_BYTES {
                    return Err(AdapterError::protocol(
                        "process stdout exceeds collect size bound",
                    ));
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(AdapterError::other("stdout read failed")),
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn parse_outcome_json(stdout: &str) -> AdapterResult<ExecutionOutcome> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Err(AdapterError::protocol(
            "process produced no collectable stdout",
        ));
    }
    let value: Value = serde_json::from_str(trimmed)
        .map_err(|_| AdapterError::protocol("process stdout is not JSON"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| AdapterError::protocol("process stdout JSON must be an object"))?;
    let ok = match obj.get("ok") {
        Some(Value::Bool(flag)) => *flag,
        _ => {
            return Err(AdapterError::protocol(
                "process stdout JSON must include boolean ok",
            ))
        }
    };
    let summary = obj
        .get("summary")
        .and_then(Value::as_str)
        .map(str::to_string);
    if obj.contains_key("failure_class") {
        return Err(AdapterError::protocol(
            "adapter outcome must not include Scheduler failure_class",
        ));
    }
    if ok {
        let payload = obj.get("payload").cloned();
        return Ok(ExecutionOutcome {
            state: ExecutionState::Succeeded,
            payload,
            summary,
            terminal_confirmed: true,
            quiescent_confirmed: false,
            incarnation_reusable: false,
        });
    }
    Ok(ExecutionOutcome {
        state: ExecutionState::Failed,
        payload: None,
        summary,
        terminal_confirmed: true,
        quiescent_confirmed: false,
        incarnation_reusable: false,
    })
}

enum InstancePresence {
    Running,
    Ended,
    Absent,
}

fn liveness_observation(presence: InstancePresence) -> ExecutionObservation {
    match presence {
        InstancePresence::Running => ExecutionObservation {
            state: ExecutionState::Running,
            terminal_confirmed: false,
            quiescent_confirmed: false,
            detail: None,
        },
        InstancePresence::Ended => ExecutionObservation {
            state: ExecutionState::Terminated,
            terminal_confirmed: false,
            quiescent_confirmed: false,
            detail: Some("identified process ended; collect for outcome".into()),
        },
        InstancePresence::Absent => ExecutionObservation {
            state: ExecutionState::Unknown,
            terminal_confirmed: false,
            quiescent_confirmed: false,
            detail: Some("process instance not found".into()),
        },
    }
}

fn ended_start_observation(handle: RuntimeHandle) -> StartObservation {
    StartObservation {
        state: ExecutionState::Terminated,
        runtime_handle: handle,
        ambiguous: false,
        detail: Some("identified process ended; collect for outcome".into()),
        terminal_confirmed: false,
        quiescent_confirmed: false,
    }
}

fn terminated_observation() -> ExecutionObservation {
    ExecutionObservation {
        state: ExecutionState::Terminated,
        terminal_confirmed: false,
        quiescent_confirmed: false,
        detail: Some("kill issued; process exited".into()),
    }
}

impl ExecutionAdapter for LocalProcessAgentAdapter {
    fn start_execution(
        &self,
        request: &ExecutionRequest,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<StartObservation> {
        require_deadline(deadline, "start deadline already expired", None)?;
        let spec = spec_from_options(request.target_options())?;
        require_deadline(deadline, "start deadline exhausted before spawn", None)?;
        let exec_dir =
            std::env::temp_dir().join(format!("agentype-exec-{}", request.request_id().as_str()));
        fs::create_dir_all(&exec_dir).map_err(|_| diagnostic("cannot create execution dir"))?;
        let stdout_path = exec_dir.join("stdout.txt");
        let stderr_path = exec_dir.join("stderr.txt");
        require_deadline(deadline, "start deadline exhausted before spawn", None)?;
        let mut child = spawn_child(&spec, &stdout_path, &stderr_path, deadline)?;
        let pid = child.id();
        if let Err(err) = require_deadline(deadline, "start deadline exhausted after spawn", None) {
            kill_child(&mut child);
            let _ = child.try_wait();
            return Err(err);
        }
        let birth = match process_birth(pid, deadline) {
            Ok(b) => b,
            Err(err) => {
                kill_child(&mut child);
                let _ = child.try_wait();
                return Err(err);
            }
        };
        let parsed = ParsedHandle {
            pid,
            birth,
            request_id: request.request_id().as_str().to_string(),
            stdout_path: stdout_path.clone(),
            stderr_path: stderr_path.clone(),
        };
        let handle = encode_handle(&parsed);
        require_deadline(
            deadline,
            "start deadline exhausted after spawn",
            Some(&handle),
        )?;

        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                kill_child(&mut child);
                return Err(AdapterError::other("child stdin missing").with_handle_hint(handle));
            }
        };
        let input = serde_json::to_vec(request.payload()).unwrap_or_else(|_| b"{}".to_vec());
        if let Err(err) = write_stdin_deadline(stdin, &input, deadline) {
            kill_child(&mut child);
            let _ = child.try_wait();
            return Err(err.with_handle_hint(handle));
        }
        require_deadline(
            deadline,
            "start deadline exhausted after stdin",
            Some(&handle),
        )?;

        // Start creates the environment and returns a handle. Waiting for the
        // agent to finish is collect_outcome's operation (its own deadline).
        match child.try_wait() {
            Ok(Some(_)) => {
                require_deadline(
                    deadline,
                    "start deadline exhausted before observation",
                    Some(&handle),
                )?;
                Ok(ended_start_observation(handle))
            }
            Ok(None) => {
                require_deadline(
                    deadline,
                    "start deadline exhausted before RUNNING",
                    Some(&handle),
                )?;
                self.live
                    .lock()
                    .expect("live map")
                    .insert(pid, LiveProcess { child, birth });
                Ok(StartObservation {
                    state: ExecutionState::Running,
                    runtime_handle: handle,
                    ambiguous: false,
                    detail: None,
                    terminal_confirmed: false,
                    quiescent_confirmed: false,
                })
            }
            Err(_) => {
                kill_child(&mut child);
                Err(AdapterError::other("child wait failed").with_handle_hint(handle))
            }
        }
    }

    fn observe_execution(
        &self,
        handle: &RuntimeHandle,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<ExecutionObservation> {
        require_deadline(deadline, "observe deadline already expired", None)?;
        let parsed = parse_handle(handle)?;
        let mut live = self.live.lock().expect("live map");
        if let Some(proc) = live.get_mut(&parsed.pid) {
            if proc.birth != parsed.birth {
                drop(live);
                let presence = instance_presence(parsed.pid, parsed.birth, deadline)?;
                require_deadline(deadline, "observe deadline exhausted", Some(handle))?;
                return Ok(liveness_observation(presence));
            }
            match proc.child.try_wait() {
                Ok(Some(_)) => {
                    live.remove(&parsed.pid);
                    require_deadline(deadline, "observe deadline exhausted", Some(handle))?;
                    return Ok(liveness_observation(InstancePresence::Ended));
                }
                Ok(None) => {
                    require_deadline(deadline, "observe deadline exhausted", Some(handle))?;
                    return Ok(liveness_observation(InstancePresence::Running));
                }
                Err(_) => return Err(AdapterError::other("observe wait failed")),
            }
        }
        drop(live);
        let presence = instance_presence(parsed.pid, parsed.birth, deadline)?;
        require_deadline(deadline, "observe deadline exhausted", Some(handle))?;
        if matches!(presence, InstancePresence::Running) {
            require_deadline(
                deadline,
                "observe deadline exhausted before RUNNING",
                Some(handle),
            )?;
        }
        Ok(liveness_observation(presence))
    }

    fn interrupt_execution(
        &self,
        handle: &RuntimeHandle,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<ExecutionObservation> {
        require_deadline(deadline, "interrupt deadline already expired", None)?;
        let parsed = parse_handle(handle)?;
        match pin::pin_instance(parsed.pid, parsed.birth, deadline)? {
            pin::PinOutcome::Pinned(pinned) => pinned.interrupt(deadline)?,
            pin::PinOutcome::Gone | pin::PinOutcome::Mismatch => {}
        }
        require_deadline(
            deadline,
            "interrupt deadline exhausted after signal",
            Some(handle),
        )?;
        self.observe_execution(handle, deadline)
    }

    fn terminate_execution(
        &self,
        handle: &RuntimeHandle,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<ExecutionObservation> {
        require_deadline(deadline, "terminate deadline already expired", None)?;
        let parsed = parse_handle(handle)?;
        let proc = {
            let mut live = self.live.lock().expect("live map");
            match live.remove(&parsed.pid) {
                Some(proc) if proc.birth == parsed.birth => Some(proc),
                Some(proc) => {
                    live.insert(parsed.pid, proc);
                    None
                }
                None => None,
            }
        };
        if let Some(mut proc) = proc {
            match pin::pin_instance(parsed.pid, parsed.birth, deadline)? {
                pin::PinOutcome::Pinned(pinned) => pinned.kill(deadline)?,
                pin::PinOutcome::Gone => {
                    let _ = proc.child.try_wait();
                    return Ok(liveness_observation(InstancePresence::Ended));
                }
                pin::PinOutcome::Mismatch => {
                    let _ = proc.child.try_wait();
                    return Ok(liveness_observation(InstancePresence::Absent));
                }
            }
            if wait_child_exit(&mut proc.child, deadline)? {
                require_deadline(
                    deadline,
                    "terminate deadline exhausted after wait",
                    Some(handle),
                )?;
                return Ok(terminated_observation());
            }
            let _ = proc.child.try_wait();
            return Err(deadline_exceeded_hint(
                "terminate wait exhausted; kill sent is not quiescence",
                handle.clone(),
            ));
        }
        match pin::pin_instance(parsed.pid, parsed.birth, deadline)? {
            pin::PinOutcome::Pinned(pinned) => {
                pinned.kill(deadline)?;
                if pinned.wait_exit(deadline)? {
                    require_deadline(
                        deadline,
                        "terminate deadline exhausted after wait",
                        Some(handle),
                    )?;
                    return Ok(terminated_observation());
                }
                Err(deadline_exceeded_hint(
                    "terminate wait exhausted; kill sent is not quiescence",
                    handle.clone(),
                ))
            }
            pin::PinOutcome::Gone => {
                require_deadline(deadline, "terminate deadline exhausted", Some(handle))?;
                Ok(liveness_observation(InstancePresence::Ended))
            }
            pin::PinOutcome::Mismatch => {
                require_deadline(deadline, "terminate deadline exhausted", Some(handle))?;
                Ok(liveness_observation(InstancePresence::Absent))
            }
        }
    }

    fn collect_outcome(
        &self,
        handle: &RuntimeHandle,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<ExecutionOutcome> {
        require_deadline(deadline, "collect deadline already expired", None)?;
        let parsed = parse_handle(handle)?;
        let proc = {
            let mut live = self.live.lock().expect("live map");
            match live.remove(&parsed.pid) {
                Some(proc) if proc.birth == parsed.birth => Some(proc),
                Some(proc) => {
                    live.insert(parsed.pid, proc);
                    None
                }
                None => None,
            }
        };
        if let Some(mut proc) = proc {
            if !wait_child_exit(&mut proc.child, deadline)? {
                self.live.lock().expect("live map").insert(parsed.pid, proc);
                return Err(deadline_exceeded_hint(
                    "collect deadline exhausted before process exit",
                    handle.clone(),
                ));
            }
        } else if !wait_instance_exit(parsed.pid, parsed.birth, deadline)? {
            return Err(deadline_exceeded_hint(
                "collect deadline exhausted before process exit",
                handle.clone(),
            ));
        }
        require_deadline(
            deadline,
            "collect deadline exhausted before stdout read",
            Some(handle),
        )?;
        let stdout = read_stdout_bounded(&parsed.stdout_path, deadline).map_err(|err| match err
            .runtime_handle_hint()
        {
            Some(_) => err,
            None => err.with_handle_hint(handle.clone()),
        })?;
        require_deadline(
            deadline,
            "collect deadline exhausted before outcome parse",
            Some(handle),
        )?;
        parse_outcome_json(&stdout)
    }

    fn reconcile_start(
        &self,
        request_id: &RequestId,
        persisted_handle: Option<&RuntimeHandle>,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<StartObservation> {
        require_deadline(deadline, "reconcile deadline already expired", None)?;
        let Some(handle) = persisted_handle else {
            return Ok(StartObservation {
                state: ExecutionState::Unknown,
                runtime_handle: RuntimeHandle::default(),
                ambiguous: true,
                detail: Some("no persisted handle; identity is request_id only".into()),
                terminal_confirmed: false,
                quiescent_confirmed: false,
            });
        };
        let parsed = match parse_handle(handle) {
            Ok(p) => p,
            Err(err) => {
                return Err(AdapterError::protocol(
                    err.diagnostic().unwrap_or("invalid persisted handle"),
                ));
            }
        };
        if parsed.request_id != request_id.as_str() {
            return Err(AdapterError::protocol(
                "persisted handle request_id does not match reconcile identity",
            ));
        }
        let presence = {
            let mut live = self.live.lock().expect("live map");
            if let Some(proc) = live.get_mut(&parsed.pid) {
                if proc.birth != parsed.birth {
                    drop(live);
                    instance_presence(parsed.pid, parsed.birth, deadline)?
                } else {
                    match proc.child.try_wait() {
                        Ok(Some(_)) => {
                            live.remove(&parsed.pid);
                            InstancePresence::Ended
                        }
                        Ok(None) => InstancePresence::Running,
                        Err(_) => {
                            return Err(AdapterError::other("reconcile wait failed"));
                        }
                    }
                }
            } else {
                drop(live);
                instance_presence(parsed.pid, parsed.birth, deadline)?
            }
        };
        if matches!(presence, InstancePresence::Running) {
            require_deadline(
                deadline,
                "reconcile deadline exhausted before RUNNING",
                Some(handle),
            )?;
        }
        Ok(match presence {
            InstancePresence::Running => StartObservation {
                state: ExecutionState::Running,
                runtime_handle: handle.clone(),
                ambiguous: false,
                detail: None,
                terminal_confirmed: false,
                quiescent_confirmed: false,
            },
            InstancePresence::Ended => ended_start_observation(handle.clone()),
            InstancePresence::Absent => StartObservation {
                state: ExecutionState::Unknown,
                runtime_handle: handle.clone(),
                ambiguous: true,
                detail: Some("process instance not found".into()),
                terminal_confirmed: false,
                quiescent_confirmed: false,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentype_adapter_api::AdapterErrorKind;

    #[test]
    fn spec_rejects_missing_command_as_unavailable() {
        let err = spec_from_options(&json!({})).unwrap_err();
        assert_eq!(err.kind(), AdapterErrorKind::Unavailable);
    }

    #[test]
    fn spec_rejects_non_string_args_as_protocol() {
        let err = spec_from_options(&json!({"command": "x", "args": [1]})).unwrap_err();
        assert_eq!(err.kind(), AdapterErrorKind::Protocol);
    }

    #[test]
    fn parse_handle_rejects_wrong_kind() {
        let handle = RuntimeHandle(json!({
            "v": 1,
            "kind": "codex",
            "pid": 1,
            "birth": 1,
            "request_id": "r",
            "stdout": "a",
            "stderr": "b"
        }));
        let err = parse_handle(&handle).unwrap_err();
        assert_eq!(err.kind(), AdapterErrorKind::Protocol);
    }

    #[test]
    fn parse_handle_rejects_missing_request_id_and_birth() {
        let no_req = RuntimeHandle(json!({
            "v": 1,
            "kind": ADAPTER_KIND,
            "pid": 1,
            "birth": 9,
            "stdout": "a",
            "stderr": "b"
        }));
        assert_eq!(
            parse_handle(&no_req).unwrap_err().kind(),
            AdapterErrorKind::Protocol
        );
        let no_birth = RuntimeHandle(json!({
            "v": 1,
            "kind": ADAPTER_KIND,
            "pid": 1,
            "request_id": "r",
            "stdout": "a",
            "stderr": "b"
        }));
        assert_eq!(
            parse_handle(&no_birth).unwrap_err().kind(),
            AdapterErrorKind::Protocol
        );
        let empty_req = RuntimeHandle(json!({
            "v": 1,
            "kind": ADAPTER_KIND,
            "pid": 1,
            "birth": 9,
            "request_id": "",
            "stdout": "a",
            "stderr": "b"
        }));
        assert_eq!(
            parse_handle(&empty_req).unwrap_err().kind(),
            AdapterErrorKind::Protocol
        );
    }

    #[test]
    fn malformed_stdout_is_protocol_not_scheduler_failure() {
        let err = parse_outcome_json("this is not json {{{").unwrap_err();
        assert_eq!(err.kind(), AdapterErrorKind::Protocol);
    }

    #[test]
    fn successful_json_does_not_claim_quiescence() {
        let out =
            parse_outcome_json(r#"{"ok":true,"payload":{"echo":true},"summary":"x"}"#).unwrap();
        assert_eq!(out.state, ExecutionState::Succeeded);
        assert!(out.terminal_confirmed);
        assert!(!out.quiescent_confirmed);
        assert!(!out.incarnation_reusable);
    }

    #[test]
    fn writer_quiescence_unknown_from_json_is_protocol() {
        let err = parse_outcome_json(
            r#"{"ok":false,"failure_class":"WRITER_QUIESCENCE_UNKNOWN","summary":"nope"}"#,
        )
        .unwrap_err();
        assert_eq!(err.kind(), AdapterErrorKind::Protocol);
    }

    #[test]
    fn unknown_failure_class_from_json_is_protocol() {
        let err = parse_outcome_json(r#"{"ok":false,"failure_class":"NOT_A_CLASS"}"#).unwrap_err();
        assert_eq!(err.kind(), AdapterErrorKind::Protocol);
    }

    #[test]
    fn omitted_failure_class_is_failed_without_scheduler_class() {
        let out = parse_outcome_json(r#"{"ok":false,"summary":"nope"}"#).unwrap();
        assert_eq!(out.state, ExecutionState::Failed);
        assert!(!out.quiescent_confirmed);
    }

    #[test]
    fn missing_or_non_bool_ok_is_protocol_not_failed() {
        for raw in [
            "{}",
            r#"{"ok":"banana"}"#,
            r#"{"ok":1}"#,
            r#"{"summary":"x"}"#,
        ] {
            let err = parse_outcome_json(raw).unwrap_err();
            assert_eq!(
                err.kind(),
                AdapterErrorKind::Protocol,
                "stdout {raw} must not become terminal Failed"
            );
        }
    }

    #[test]
    fn handle_json_roundtrip_preserves_locator_and_birth() {
        let parsed = ParsedHandle {
            pid: 4242,
            birth: 99,
            request_id: "req-1".into(),
            stdout_path: PathBuf::from("stdout.txt"),
            stderr_path: PathBuf::from("stderr.txt"),
        };
        let encoded = encode_handle(&parsed);
        let text = serde_json::to_string(&encoded.0).unwrap();
        let restored = RuntimeHandle(serde_json::from_str(&text).unwrap());
        let again = parse_handle(&restored).unwrap();
        assert_eq!(again.pid, 4242);
        assert_eq!(again.birth, 99);
        assert_eq!(again.request_id, "req-1");
    }

    #[test]
    fn proc_stat_starttime_is_field_22() {
        let line = "1234 (fake-agent) S 1 1 1 0 -1 4194304 0 0 0 0 0 0 0 0 20 0 1 0 99999 0";
        assert_eq!(parse_stat_identity(line), Some(('S', 99999)));
    }

    #[test]
    fn proc_stat_zombie_is_not_alive() {
        let line = "1234 (fake-agent) Z 1 1 1 0 -1 4194304 0 0 0 0 0 0 0 0 20 0 1 0 99999 0";
        assert_eq!(parse_stat_identity(line), Some(('Z', 99999)));
        assert!(matches!(parse_stat_identity(line), Some(('Z', _))));
    }

    #[test]
    fn malformed_proc_stat_is_adapter_error_not_gone() {
        let err = stat_identity_from_text("not-a-stat-line").unwrap_err();
        assert_eq!(err.kind(), AdapterErrorKind::Other);
        assert!(parse_stat_identity("1234 (no-fields)").is_none());
        assert_eq!(
            stat_identity_from_text("1234 (no-fields)")
                .unwrap_err()
                .kind(),
            AdapterErrorKind::Other
        );
    }

    #[test]
    fn domain_key_rejects_empty_or_unknown_placeholders() {
        for (boot, pid, mnt) in [
            ("", "pid:[1]", "mnt:[1]"),
            ("boot", "", "mnt:[1]"),
            ("boot", "pid:[1]", ""),
            ("unknown-boot", "pid:[1]", "mnt:[1]"),
            ("boot", "unknown-ns", "mnt:[1]"),
        ] {
            let err = linux_domain_key(boot, pid, mnt).unwrap_err();
            assert_eq!(err.kind(), AdapterErrorKind::Unavailable);
        }
        let key = linux_domain_key("boot-id", "pid:[1]", "mnt:[2]").unwrap();
        assert_eq!(key.as_str(), "linux:boot-id:pid:[1]:mnt:[2]");
        assert!(!key.as_str().contains("unknown"));
    }
}
