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
    ExecutionOutcome, ExecutionRequest, RuntimeHandle, StartObservation,
};
use agentype_core::{ExecutionState, FailureClass, RequestId};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

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
}

impl Default for LocalProcessAgentAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalProcessAgentAdapter {
    pub fn new() -> Self {
        Self {
            live: Mutex::new(HashMap::new()),
        }
    }
}

impl Drop for LocalProcessAgentAdapter {
    fn drop(&mut self) {
        // Dropping adapter ownership is not terminate_execution. Child::drop
        // waits, so the live Child is forgotten: the OS process continues.
        if let Ok(mut live) = self.live.lock() {
            for (_, proc) in live.drain() {
                std::mem::forget(proc.child);
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

fn wait_instance_exit(pid: u32, birth: u64, deadline: &AdapterDeadline) -> bool {
    loop {
        if !instance_alive(pid, birth) {
            return true;
        }
        match wait_slice(deadline) {
            None => return false,
            Some(slice) => thread::sleep(slice),
        }
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

fn kill_pid(pid: u32) {
    #[cfg(windows)]
    {
        kill_pid_windows(pid);
    }
    #[cfg(unix)]
    {
        signal_pid_unix(pid, 9);
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
    }
}

fn interrupt_pid(pid: u32) -> AdapterResult<()> {
    #[cfg(unix)]
    {
        let _ = signal_pid_unix(pid, 2);
        Ok(())
    }
    #[cfg(windows)]
    {
        interrupt_pid_windows(pid)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
        Err(AdapterError::unavailable(
            "interrupt unsupported on this platform",
        ))
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn kill_pid_windows(pid: u32) {
    const PROCESS_TERMINATE: u32 = 0x0001;
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut core::ffi::c_void;
        fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
        fn TerminateProcess(handle: *mut core::ffi::c_void, code: u32) -> i32;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if handle.is_null() {
            return;
        }
        let _ = TerminateProcess(handle, 1);
        CloseHandle(handle);
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn interrupt_pid_windows(pid: u32) -> AdapterResult<()> {
    const CTRL_BREAK_EVENT: u32 = 1;
    extern "system" {
        fn AttachConsole(pid: u32) -> i32;
        fn FreeConsole() -> i32;
        fn GenerateConsoleCtrlEvent(event: u32, group: u32) -> i32;
    }
    // SAFETY: AttachConsole/GenerateConsoleCtrlEvent/FreeConsole are the
    // documented best-effort console interrupt; we detach if we attached.
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

#[cfg(unix)]
#[allow(unsafe_code)]
fn signal_pid_unix(pid: u32, sig: i32) -> bool {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    unsafe { kill(raw, sig) == 0 }
}

fn process_birth(pid: u32) -> Option<u64> {
    #[cfg(windows)]
    {
        process_birth_windows(pid)
    }
    #[cfg(unix)]
    {
        process_birth_unix(pid)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
        None
    }
}

fn instance_alive(pid: u32, birth: u64) -> bool {
    process_birth(pid) == Some(birth)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn process_birth_windows(pid: u32) -> Option<u64> {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
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
            return None;
        }
        let mut code = 0u32;
        let alive = GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE;
        let mut creation = FileTime { low: 0, high: 0 };
        let mut exit = FileTime { low: 0, high: 0 };
        let mut kernel = FileTime { low: 0, high: 0 };
        let mut user = FileTime { low: 0, high: 0 };
        let times = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
        CloseHandle(handle);
        if !alive || times == 0 {
            return None;
        }
        Some(((creation.high as u64) << 32) | creation.low as u64)
    }
}

#[cfg(unix)]
fn process_birth_unix(pid: u32) -> Option<u64> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_stat_starttime(&text)
}

#[cfg(any(unix, test))]
fn parse_stat_starttime(stat: &str) -> Option<u64> {
    let close = stat.rfind(')')?;
    let rest = stat.get(close + 1..)?;
    rest.split_whitespace().nth(19)?.parse().ok()
}

fn spawn_child(spec: &ProcessSpec, stdout_path: &Path, stderr_path: &Path) -> AdapterResult<Child> {
    let stdout = File::create(stdout_path).map_err(|_| diagnostic("cannot create stdout file"))?;
    let stderr = File::create(stderr_path).map_err(|_| diagnostic("cannot create stderr file"))?;
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
    let ok = obj.get("ok").and_then(Value::as_bool).unwrap_or(false);
    let summary = obj
        .get("summary")
        .and_then(Value::as_str)
        .map(str::to_string);
    if ok {
        let payload = obj.get("payload").cloned();
        return Ok(ExecutionOutcome {
            state: ExecutionState::Succeeded,
            payload,
            summary,
            failure_class: None,
            terminal_confirmed: true,
            quiescent_confirmed: false,
            incarnation_reusable: false,
        });
    }
    let failure_class = match obj.get("failure_class") {
        None | Some(Value::Null) => FailureClass::StartFailure,
        Some(Value::String(s)) => match FailureClass::parse_sql(s) {
            Ok(c) if c.is_mechanical() => c,
            Ok(_) => {
                return Err(AdapterError::protocol(
                    "scheduler-derived failure_class is not adapter-authored",
                ));
            }
            Err(_) => {
                return Err(AdapterError::protocol("unknown failure_class"));
            }
        },
        Some(_) => {
            return Err(AdapterError::protocol("failure_class must be a string"));
        }
    };
    Ok(ExecutionOutcome {
        state: ExecutionState::Failed,
        payload: None,
        summary,
        failure_class: Some(failure_class),
        terminal_confirmed: true,
        quiescent_confirmed: false,
        incarnation_reusable: false,
    })
}

fn liveness_observation(alive: bool) -> ExecutionObservation {
    if alive {
        ExecutionObservation {
            state: ExecutionState::Running,
            terminal_confirmed: false,
            quiescent_confirmed: false,
            detail: None,
        }
    } else {
        ExecutionObservation {
            state: ExecutionState::Unknown,
            terminal_confirmed: false,
            quiescent_confirmed: false,
            detail: Some("process not running".into()),
        }
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
        let mut child = spawn_child(&spec, &stdout_path, &stderr_path)?;
        let pid = child.id();
        let birth = match process_birth(pid) {
            Some(b) => b,
            None => {
                kill_child(&mut child);
                let _ = child.try_wait();
                return Err(diagnostic("cannot read process birth identity"));
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
        let prompt = request.prompt().as_bytes().to_vec();
        if let Err(err) = write_stdin_deadline(stdin, &prompt, deadline) {
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
                Ok(StartObservation {
                    state: ExecutionState::Unknown,
                    runtime_handle: handle,
                    ambiguous: true,
                    failure_class: None,
                    detail: Some("process exited during start; collect for outcome".into()),
                    terminal_confirmed: false,
                    quiescent_confirmed: false,
                })
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
                    failure_class: None,
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
                let alive = instance_alive(parsed.pid, parsed.birth);
                require_deadline(deadline, "observe deadline exhausted", Some(handle))?;
                return Ok(liveness_observation(alive));
            }
            match proc.child.try_wait() {
                Ok(Some(_)) => {
                    live.remove(&parsed.pid);
                    require_deadline(deadline, "observe deadline exhausted", Some(handle))?;
                    return Ok(liveness_observation(false));
                }
                Ok(None) => {
                    require_deadline(deadline, "observe deadline exhausted", Some(handle))?;
                    return Ok(liveness_observation(true));
                }
                Err(_) => return Err(AdapterError::other("observe wait failed")),
            }
        }
        drop(live);
        let alive = instance_alive(parsed.pid, parsed.birth);
        require_deadline(deadline, "observe deadline exhausted", Some(handle))?;
        if alive {
            require_deadline(
                deadline,
                "observe deadline exhausted before RUNNING",
                Some(handle),
            )?;
        }
        Ok(liveness_observation(alive))
    }

    fn interrupt_execution(
        &self,
        handle: &RuntimeHandle,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<ExecutionObservation> {
        require_deadline(deadline, "interrupt deadline already expired", None)?;
        let parsed = parse_handle(handle)?;
        // Physical interrupt is attempted even if the live Child is gone.
        // Failure to deliver is explicit Unavailable, not a silent observe.
        interrupt_pid(parsed.pid)?;
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
            kill_child(&mut proc.child);
            if wait_child_exit(&mut proc.child, deadline)? {
                require_deadline(
                    deadline,
                    "terminate deadline exhausted after wait",
                    Some(handle),
                )?;
                return Ok(terminated_observation());
            }
            std::mem::forget(proc.child);
            return Err(deadline_exceeded_hint(
                "terminate wait exhausted; kill sent is not quiescence",
                handle.clone(),
            ));
        }
        if instance_alive(parsed.pid, parsed.birth) {
            kill_pid(parsed.pid);
            if wait_instance_exit(parsed.pid, parsed.birth, deadline) {
                require_deadline(
                    deadline,
                    "terminate deadline exhausted after wait",
                    Some(handle),
                )?;
                return Ok(terminated_observation());
            }
            return Err(deadline_exceeded_hint(
                "terminate wait exhausted; kill sent is not quiescence",
                handle.clone(),
            ));
        }
        require_deadline(deadline, "terminate deadline exhausted", Some(handle))?;
        Ok(liveness_observation(false))
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
        } else if !wait_instance_exit(parsed.pid, parsed.birth, deadline) {
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
                failure_class: None,
                detail: Some("no persisted handle; identity is request_id only".into()),
                terminal_confirmed: false,
                quiescent_confirmed: false,
            });
        };
        let parsed = match parse_handle(handle) {
            Ok(p) => p,
            Err(err) => {
                return Ok(StartObservation {
                    state: ExecutionState::Unknown,
                    runtime_handle: handle.clone(),
                    ambiguous: true,
                    failure_class: Some(FailureClass::AdapterProtocolFailure),
                    detail: err.diagnostic().map(str::to_string),
                    terminal_confirmed: false,
                    quiescent_confirmed: false,
                });
            }
        };
        if parsed.request_id != request_id.as_str() {
            return Err(AdapterError::protocol(
                "persisted handle request_id does not match reconcile identity",
            ));
        }
        let alive = {
            let mut live = self.live.lock().expect("live map");
            if let Some(proc) = live.get_mut(&parsed.pid) {
                if proc.birth != parsed.birth {
                    drop(live);
                    instance_alive(parsed.pid, parsed.birth)
                } else {
                    match proc.child.try_wait() {
                        Ok(Some(_)) => {
                            live.remove(&parsed.pid);
                            false
                        }
                        Ok(None) => true,
                        Err(_) => false,
                    }
                }
            } else {
                drop(live);
                instance_alive(parsed.pid, parsed.birth)
            }
        };
        if alive {
            require_deadline(
                deadline,
                "reconcile deadline exhausted before RUNNING",
                Some(handle),
            )?;
        }
        Ok(StartObservation {
            state: if alive {
                ExecutionState::Running
            } else {
                ExecutionState::Unknown
            },
            runtime_handle: handle.clone(),
            ambiguous: !alive,
            failure_class: None,
            detail: None,
            terminal_confirmed: false,
            quiescent_confirmed: false,
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
    fn omitted_failure_class_defaults_to_start_failure() {
        let out = parse_outcome_json(r#"{"ok":false,"summary":"nope"}"#).unwrap();
        assert_eq!(out.state, ExecutionState::Failed);
        assert_eq!(out.failure_class, Some(FailureClass::StartFailure));
        assert!(!out.quiescent_confirmed);
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
        assert_eq!(parse_stat_starttime(line), Some(99999));
    }
}
