//! Local-process Execution Environment Adapter.
//!
//! This crate proves that a real OS process can attach to the frozen M5.6
//! contract without leaking model, provider, prompt orchestration, or
//! harness ownership into the Scheduler. The executable is user-owned
//! configuration (`target_options.command`); Core never sees it.

// Windows process-liveness query requires a tiny FFI surface. No other unsafe.

use agentype_adapter_api::{
    AdapterDeadline, AdapterError, AdapterResult, ExecutionAdapter, ExecutionObservation,
    ExecutionOutcome, ExecutionRequest, RuntimeHandle, StartObservation,
};
use agentype_core::{ExecutionState, FailureClass, RequestId};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

/// Frozen adapter_kind for this environment domain: this host's process
/// table as spawned by this runtime. Not a vendor/model name.
pub const ADAPTER_KIND: &str = "local_process";

const HANDLE_VERSION: i64 = 1;
const WAIT_SLICE: Duration = Duration::from_millis(10);

struct LiveProcess {
    child: Child,
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
        // Best-effort: do not wait. A dropped adapter must not leak OS processes.
        if let Ok(mut live) = self.live.lock() {
            for (_, mut proc) in live.drain() {
                kill_child(&mut proc.child);
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
    let request_id = obj
        .get("request_id")
        .and_then(Value::as_str)
        .unwrap_or("")
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

fn write_all_bounded<W>(writer: W, bytes: Vec<u8>, deadline: &AdapterDeadline) -> AdapterResult<()>
where
    W: Write + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let mut writer = writer;
        let result = writer
            .write_all(&bytes)
            .and_then(|_| writer.flush())
            .map_err(|_| ());
        drop(writer);
        let _ = tx.send(result);
    });
    match rx.recv_timeout(deadline.remaining()) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(())) => Err(AdapterError::other("stdin write failed")),
        Err(_) => Err(AdapterError::deadline_exceeded("stdin write blocked")),
    }
}

fn kill_child(child: &mut Child) {
    let _ = child.kill();
}

fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        pid_alive_windows(pid)
    }
    #[cfg(unix)]
    {
        pid_alive_unix(pid)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
        false
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn pid_alive_windows(pid: u32) -> bool {
    // PROCESS_QUERY_LIMITED_INFORMATION + GetExitCodeProcess STILL_ACTIVE.
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut core::ffi::c_void;
        fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
        fn GetExitCodeProcess(handle: *mut core::ffi::c_void, code: *mut u32) -> i32;
    }
    // SAFETY: OpenProcess returns either null or an owned handle we CloseHandle
    // before return. GetExitCodeProcess is only called on that live handle.
    // No other alias of the handle exists in this function.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        ok != 0 && code == STILL_ACTIVE
    }
}

#[cfg(unix)]
fn pid_alive_unix(pid: u32) -> bool {
    let path = format!("/proc/{pid}");
    Path::new(&path).exists()
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
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn()
        .map_err(|_| AdapterError::unavailable("failed to spawn local process"))
}

fn read_file_lossy(path: &Path) -> String {
    let mut buf = String::new();
    if let Ok(mut f) = File::open(path) {
        let _ = f.read_to_string(&mut buf);
    }
    buf
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
    let failure_class = obj
        .get("failure_class")
        .and_then(Value::as_str)
        .and_then(|s| FailureClass::parse_sql(s).ok())
        .filter(|c| c.is_mechanical())
        .unwrap_or(FailureClass::StartFailure);
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

impl ExecutionAdapter for LocalProcessAgentAdapter {
    fn start_execution(
        &self,
        request: &ExecutionRequest,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<StartObservation> {
        if deadline.is_expired() {
            return Err(AdapterError::deadline_exceeded(
                "start deadline already expired",
            ));
        }
        let spec = spec_from_options(request.target_options())?;
        let exec_dir =
            std::env::temp_dir().join(format!("agentype-exec-{}", request.request_id().as_str()));
        fs::create_dir_all(&exec_dir).map_err(|_| diagnostic("cannot create execution dir"))?;
        let stdout_path = exec_dir.join("stdout.txt");
        let stderr_path = exec_dir.join("stderr.txt");
        let mut child = spawn_child(&spec, &stdout_path, &stderr_path)?;
        let pid = child.id();
        let parsed = ParsedHandle {
            pid,
            request_id: request.request_id().as_str().to_string(),
            stdout_path: stdout_path.clone(),
            stderr_path: stderr_path.clone(),
        };
        let handle = encode_handle(&parsed);

        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                kill_child(&mut child);
                return Err(AdapterError::other("child stdin missing").with_handle_hint(handle));
            }
        };
        let prompt = request.prompt().as_bytes().to_vec();
        if let Err(err) = write_all_bounded(stdin, prompt, deadline) {
            kill_child(&mut child);
            let _ = child.try_wait();
            return Err(err.with_handle_hint(handle));
        }

        // Start creates the environment and returns a handle. Waiting for the
        // agent to finish is collect_outcome's operation (its own deadline).
        match child.try_wait() {
            Ok(Some(_)) => Ok(StartObservation {
                state: ExecutionState::Unknown,
                runtime_handle: handle,
                ambiguous: true,
                failure_class: None,
                detail: Some("process exited during start; collect for outcome".into()),
                terminal_confirmed: false,
                quiescent_confirmed: false,
            }),
            Ok(None) => {
                self.live
                    .lock()
                    .expect("live map")
                    .insert(pid, LiveProcess { child });
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
        if deadline.is_expired() {
            return Err(AdapterError::deadline_exceeded(
                "observe deadline already expired",
            ));
        }
        let parsed = parse_handle(handle)?;
        let mut live = self.live.lock().expect("live map");
        if let Some(proc) = live.get_mut(&parsed.pid) {
            match proc.child.try_wait() {
                Ok(Some(_)) => {
                    live.remove(&parsed.pid);
                    return Ok(liveness_observation(false));
                }
                Ok(None) => return Ok(liveness_observation(true)),
                Err(_) => return Err(AdapterError::other("observe wait failed")),
            }
        }
        drop(live);
        Ok(liveness_observation(pid_alive(parsed.pid)))
    }

    fn interrupt_execution(
        &self,
        handle: &RuntimeHandle,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<ExecutionObservation> {
        // Cooperative/platform interrupt is best-effort. This call must
        // still return by the deadline and MUST NOT claim Task cancellation,
        // writer safety, or quiescence.
        self.observe_execution(handle, deadline)
    }

    fn terminate_execution(
        &self,
        handle: &RuntimeHandle,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<ExecutionObservation> {
        if deadline.is_expired() {
            return Err(AdapterError::deadline_exceeded(
                "terminate deadline already expired",
            ));
        }
        let parsed = parse_handle(handle)?;
        let proc = {
            let mut live = self.live.lock().expect("live map");
            live.remove(&parsed.pid)
        };
        if let Some(mut proc) = proc {
            kill_child(&mut proc.child);
            if wait_child_exit(&mut proc.child, deadline)? {
                return Ok(ExecutionObservation {
                    state: ExecutionState::Terminated,
                    terminal_confirmed: false,
                    quiescent_confirmed: false,
                    detail: Some("kill issued; process exited".into()),
                });
            }
            return Err(deadline_exceeded_hint(
                "terminate wait exhausted; kill sent is not quiescence",
                handle.clone(),
            ));
        }
        // No live Child: best-effort observation only. We do not invent
        // TERMINATED/quiescence from a pid that we cannot wait on.
        Ok(liveness_observation(pid_alive(parsed.pid)))
    }

    fn collect_outcome(
        &self,
        handle: &RuntimeHandle,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<ExecutionOutcome> {
        if deadline.is_expired() {
            return Err(AdapterError::deadline_exceeded(
                "collect deadline already expired",
            ));
        }
        let parsed = parse_handle(handle)?;
        let proc = {
            let mut live = self.live.lock().expect("live map");
            live.remove(&parsed.pid)
        };
        if let Some(mut proc) = proc {
            if !wait_child_exit(&mut proc.child, deadline)? {
                kill_child(&mut proc.child);
                let _ = proc.child.try_wait();
                self.live.lock().expect("live map").insert(parsed.pid, proc);
                return Err(deadline_exceeded_hint(
                    "collect deadline exhausted before process exit",
                    handle.clone(),
                ));
            }
        } else {
            while pid_alive(parsed.pid) {
                match wait_slice(deadline) {
                    None => {
                        return Err(AdapterError::deadline_exceeded(
                            "collect deadline exhausted before process exit",
                        ));
                    }
                    Some(slice) => thread::sleep(slice),
                }
            }
        }
        let stdout = read_file_lossy(&parsed.stdout_path);
        parse_outcome_json(&stdout)
    }

    fn reconcile_start(
        &self,
        request_id: &RequestId,
        persisted_handle: Option<&RuntimeHandle>,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<StartObservation> {
        if deadline.is_expired() {
            return Err(AdapterError::deadline_exceeded(
                "reconcile deadline already expired",
            ));
        }
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
        if !parsed.request_id.is_empty() && parsed.request_id != request_id.as_str() {
            return Err(AdapterError::protocol(
                "persisted handle request_id does not match reconcile identity",
            ));
        }
        let alive = {
            let mut live = self.live.lock().expect("live map");
            if let Some(proc) = live.get_mut(&parsed.pid) {
                match proc.child.try_wait() {
                    Ok(Some(_)) => {
                        live.remove(&parsed.pid);
                        false
                    }
                    Ok(None) => true,
                    Err(_) => false,
                }
            } else {
                drop(live);
                pid_alive(parsed.pid)
            }
        };
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

// Live process tests live in tests/conformance.rs so CARGO_BIN_EXE_fake-agent
// is set. Helper-level deadline tests stay here (no binary required).

#[cfg(test)]
mod tests {
    use super::*;
    use agentype_adapter_api::AdapterErrorKind;
    use std::io::{self, Write};
    use std::time::Instant;

    struct BlockingWrite;
    impl Write for BlockingWrite {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            thread::sleep(Duration::from_secs(60));
            Ok(0)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct BlockingFlush;
    impl Write for BlockingFlush {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            thread::sleep(Duration::from_secs(60));
            Ok(())
        }
    }

    fn assert_returns_by_deadline(started: Instant, budget: Duration) {
        let elapsed = started.elapsed();
        assert!(
            elapsed < budget + Duration::from_secs(1),
            "call ignored deadline: elapsed {elapsed:?} budget {budget:?}"
        );
    }

    #[test]
    fn blocked_request_write_respects_deadline() {
        let budget = Duration::from_millis(200);
        let deadline = AdapterDeadline::after(budget).unwrap();
        let started = Instant::now();
        let err = write_all_bounded(BlockingWrite, vec![1, 2, 3], &deadline).unwrap_err();
        assert_returns_by_deadline(started, budget);
        assert_eq!(err.kind(), AdapterErrorKind::DeadlineExceeded);
    }

    #[test]
    fn blocked_flush_respects_deadline() {
        let budget = Duration::from_millis(200);
        let deadline = AdapterDeadline::after(budget).unwrap();
        let started = Instant::now();
        let err = write_all_bounded(BlockingFlush, vec![1, 2, 3], &deadline).unwrap_err();
        assert_returns_by_deadline(started, budget);
        assert_eq!(err.kind(), AdapterErrorKind::DeadlineExceeded);
    }

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
        let handle =
            RuntimeHandle(json!({"v": 1, "kind": "codex", "pid": 1, "stdout": "a", "stderr": "b"}));
        let err = parse_handle(&handle).unwrap_err();
        assert_eq!(err.kind(), AdapterErrorKind::Protocol);
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
    fn writer_quiescence_unknown_is_not_accepted_from_agent_json() {
        let out = parse_outcome_json(
            r#"{"ok":false,"failure_class":"WRITER_QUIESCENCE_UNKNOWN","summary":"nope"}"#,
        )
        .unwrap();
        assert_eq!(out.state, ExecutionState::Failed);
        assert_eq!(out.failure_class, Some(FailureClass::StartFailure));
        assert!(!out.quiescent_confirmed);
    }

    #[test]
    fn handle_json_roundtrip_preserves_locator() {
        let parsed = ParsedHandle {
            pid: 4242,
            request_id: "req-1".into(),
            stdout_path: PathBuf::from("stdout.txt"),
            stderr_path: PathBuf::from("stderr.txt"),
        };
        let encoded = encode_handle(&parsed);
        let text = serde_json::to_string(&encoded.0).unwrap();
        let restored = RuntimeHandle(serde_json::from_str(&text).unwrap());
        let again = parse_handle(&restored).unwrap();
        assert_eq!(again.pid, 4242);
        assert_eq!(again.request_id, "req-1");
    }
}
