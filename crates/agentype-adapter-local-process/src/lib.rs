//! Local-process Execution Environment Adapter.
//!
//! This crate proves that a real OS process can attach to the frozen M5.6
//! contract without leaking model, provider, prompt orchestration, or
//! harness ownership into the Scheduler. The executable is user-owned
//! configuration (`target_options.command`); Core never sees it.
//!
//! Platform FFI is confined to pid liveness, process birth, cancellable
//! stdin, interrupt, and pid-level terminate. No helper threads.

#[cfg(test)]
use agentype_adapter_api::worker_protocol_v01::ExecutionOutcome;
use agentype_adapter_api::{
    AdapterDeadline, AdapterError, AdapterResult, EnvironmentStartRequest, ExecutionAdapter,
    ExecutionObservation, ImportableAdapter, PhysicalExecutionOutcome, RuntimeHandle,
    StartObservation,
};
use agentype_core::{ExecutionState, RequestId};
use agentype_execution_config::AdapterBindingKey;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod jail;
mod pin;

pub use jail::{
    JailLaunchDescriptor, JailedProcessAgentAdapter, JAIL_ADAPTER_KIND, JAIL_PROVISIONING_PROTOCOL,
};

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

const INSTANCE_ENV: &str = "AGENTYPE_INSTANCE";

struct LiveProcess {
    child: Child,
    birth: u64,
    inst: String,
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

    fn presence_for(
        &self,
        parsed: &ParsedHandle,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<InstancePresence> {
        let mut live = self.live.lock().expect("live map");
        if let Some(proc) = live.get_mut(&parsed.pid) {
            if proc.birth != parsed.birth || proc.inst != parsed.inst {
                drop(live);
                instance_presence(
                    parsed.pid,
                    parsed.birth,
                    &parsed.inst,
                    &parsed.stdout_path,
                    deadline,
                )
            } else {
                match proc.child.try_wait() {
                    Ok(Some(_)) => {
                        live.remove(&parsed.pid);
                        Ok(InstancePresence::Ended)
                    }
                    Ok(None) => Ok(InstancePresence::Running),
                    Err(_) => Err(AdapterError::other("reconcile wait failed")),
                }
            }
        } else {
            drop(live);
            instance_presence(
                parsed.pid,
                parsed.birth,
                &parsed.inst,
                &parsed.stdout_path,
                deadline,
            )
        }
    }

    fn control_parsed(
        &self,
        handle: &RuntimeHandle,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<ControlResolution> {
        match parse_control_handle(handle, ADAPTER_KIND)? {
            PhysicalControlHandle::Pinned(parsed) => Ok(ControlResolution {
                stdout_path: parsed.stdout_path.clone(),
                stderr_path: parsed.stderr_path.clone(),
                parsed: Some(parsed),
            }),
            PhysicalControlHandle::Locator(locator) => {
                let stdout_path = locator.stdout_path.clone();
                let stderr_path = locator.stderr_path.clone();
                Ok(ControlResolution {
                    parsed: resolve_locator_to_parsed(locator, deadline)?,
                    stdout_path,
                    stderr_path,
                })
            }
        }
    }
}

pub(crate) struct ControlResolution {
    pub(crate) parsed: Option<ParsedHandle>,
    pub(crate) stdout_path: PathBuf,
    pub(crate) stderr_path: PathBuf,
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

    fn import_provisioning_protocol(&self) -> Option<&str> {
        // This adapter launches a plain OS process from `target_options`; it does
        // NOT consume a typed provisioning descriptor, so it is not
        // provisioning-capable. It MUST NOT claim a descriptor grammar it ignores.
        None
    }

    // This adapter does not sandbox: `start_execution` launches a plain OS
    // process and does not confine the filesystem or network. It therefore
    // declares NO enforceable workspace mode or network policy (the conservative
    // `ImportableAdapter` default), rather than claiming an enforcement it does
    // not provide. A typed Task requiring any workspace/network mode is
    // ineligible for this source until a real sandboxed adapter exists.
}

fn capture_domain_key() -> AdapterResult<AdapterBindingKey> {
    static DOMAIN: OnceLock<AdapterResult<AdapterBindingKey>> = OnceLock::new();
    DOMAIN.get_or_init(compute_domain_key).clone()
}

fn compute_domain_key() -> AdapterResult<AdapterBindingKey> {
    let locator_root = locator_root();
    #[cfg(target_os = "linux")]
    {
        let boot = read_trimmed("/proc/sys/kernel/random/boot_id")
            .ok_or_else(|| AdapterError::unavailable("linux boot_id unavailable"))?;
        let pid_ns = read_ns("pid")?;
        let mnt_ns = read_ns("mnt")?;
        let (root_dev, root_ino) = linux_root_identity()?;
        linux_domain_key(
            &boot,
            &pid_ns,
            &mnt_ns,
            &root_dev.to_string(),
            &root_ino.to_string(),
            &locator_root,
        )
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
        windows_domain_key(&host, &boot, &locator_root)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Err(AdapterError::unavailable(
            "local_process adapter unsupported on this OS",
        ))
    }
}

/// The directory that holds durable locator records.
///
/// It is resolved exactly once per process. Every execution directory and the
/// adapter binding key are derived from this same value, so a later `TMPDIR`
/// or `TEMP` change cannot redirect this adapter to a different locator tree
/// while the domain key still claims the old physical identity. A restarted
/// adapter with a different configured temporary directory therefore computes
/// a different binding key and is treated as a different physical domain
/// instead of silently losing recovery evidence.
pub(crate) fn locator_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let configured = std::env::temp_dir();
        fs::canonicalize(&configured).unwrap_or(configured)
    })
    .clone()
}

#[cfg(any(target_os = "linux", test))]
fn linux_domain_key(
    boot: &str,
    pid_ns: &str,
    mnt_ns: &str,
    root_dev: &str,
    root_ino: &str,
    locator_root: &Path,
) -> AdapterResult<AdapterBindingKey> {
    let parts = [boot, pid_ns, mnt_ns, root_dev, root_ino];
    if parts.iter().any(|p| p.trim().is_empty()) {
        return Err(AdapterError::unavailable(
            "linux domain identity components cannot be empty",
        ));
    }
    if parts.iter().any(|p| p.contains("unknown")) {
        return Err(AdapterError::unavailable(
            "linux domain identity cannot use unknown placeholders",
        ));
    }
    let locator_root = locator_root.to_string_lossy();
    if locator_root.trim().is_empty() {
        return Err(AdapterError::unavailable(
            "linux locator root cannot be empty",
        ));
    }
    AdapterBindingKey::new(format!(
        "linux:{boot}:{pid_ns}:{mnt_ns}:{root_dev}:{root_ino}:locroot={locator_root}"
    ))
    .map_err(|_| AdapterError::unavailable("linux domain key invalid"))
}

#[cfg(windows)]
fn windows_domain_key(
    host: &str,
    boot: &str,
    locator_root: &Path,
) -> AdapterResult<AdapterBindingKey> {
    if host.trim().is_empty() || boot.trim().is_empty() {
        return Err(AdapterError::unavailable(
            "windows domain identity components cannot be empty",
        ));
    }
    let locator_root = locator_root.to_string_lossy();
    if locator_root.trim().is_empty() {
        return Err(AdapterError::unavailable(
            "windows locator root cannot be empty",
        ));
    }
    AdapterBindingKey::new(format!("win:{host}:{boot}:locroot={locator_root}"))
        .map_err(|_| AdapterError::unavailable("windows domain key invalid"))
}

#[cfg(target_os = "linux")]
fn linux_root_identity() -> AdapterResult<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata("/proc/self/root")
        .map_err(|_| AdapterError::unavailable("linux root identity unavailable"))?;
    Ok((meta.dev(), meta.ino()))
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
pub(crate) struct ParsedHandle {
    pub(crate) pid: u32,
    pub(crate) birth: u64,
    pub(crate) inst: String,
    pub(crate) request_id: String,
    pub(crate) stdout_path: PathBuf,
    pub(crate) stderr_path: PathBuf,
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
    encode_handle_with_kind(parsed, ADAPTER_KIND)
}

pub(crate) fn encode_handle_with_kind(parsed: &ParsedHandle, kind: &str) -> RuntimeHandle {
    RuntimeHandle(json!({
        "v": HANDLE_VERSION,
        "kind": kind,
        "pid": parsed.pid,
        "birth": parsed.birth,
        "inst": parsed.inst,
        "request_id": parsed.request_id,
        "stdout": parsed.stdout_path.to_string_lossy(),
        "stderr": parsed.stderr_path.to_string_lossy(),
    }))
}

/// Locator evidence after spawn, before birth is known. Not pin authority.
///
/// A locator is deliberately marked `identity_complete=false` and
/// `reconnectable=false`: it is not a stable process pin and must never be
/// treated as one. It exists so a recovery path can find the physical process
/// after a crash that happened between `spawn(2)` and the first successful
/// birth/instance-token observation.
#[cfg(test)]
pub(crate) fn encode_locator_hint(
    pid: u32,
    birth: Option<u64>,
    inst: &str,
    request_id: &str,
    stdout: &Path,
    stderr: &Path,
) -> RuntimeHandle {
    encode_locator_hint_with_kind(ADAPTER_KIND, pid, birth, inst, request_id, stdout, stderr)
}

pub(crate) fn encode_locator_hint_with_kind(
    kind: &str,
    pid: u32,
    birth: Option<u64>,
    inst: &str,
    request_id: &str,
    stdout: &Path,
    stderr: &Path,
) -> RuntimeHandle {
    let mut value = json!({
        "v": HANDLE_VERSION,
        "kind": kind,
        "pid": pid,
        "inst": inst,
        "request_id": request_id,
        "stdout": stdout.to_string_lossy(),
        "stderr": stderr.to_string_lossy(),
        "identity_complete": false,
        "reconnectable": false,
    });
    if let Some(birth) = birth {
        value["birth"] = json!(birth);
    }
    RuntimeHandle(value)
}

/// Durable, non-impersonating locator evidence for one physically created
/// process. It is persisted next to the execution's stdout/stderr before the
/// birth identity probe so a crash in the `spawn -> identity` window is still
/// recoverable by RequestId.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LocatorHandle {
    pub(crate) pid: u32,
    pub(crate) birth: Option<u64>,
    pub(crate) inst: String,
    pub(crate) request_id: String,
    pub(crate) stdout_path: PathBuf,
    pub(crate) stderr_path: PathBuf,
    /// Wall-clock lower bound recorded immediately before `spawn(2)`.
    pub(crate) spawn_after_ms: Option<u64>,
}

impl LocatorHandle {
    pub(crate) fn to_runtime_handle(&self, kind: &str) -> RuntimeHandle {
        encode_locator_hint_with_kind(
            kind,
            self.pid,
            self.birth,
            &self.inst,
            &self.request_id,
            &self.stdout_path,
            &self.stderr_path,
        )
    }

    pub(crate) fn into_parsed(self, birth: u64) -> ParsedHandle {
        ParsedHandle {
            pid: self.pid,
            birth,
            inst: self.inst,
            request_id: self.request_id,
            stdout_path: self.stdout_path,
            stderr_path: self.stderr_path,
        }
    }
}

pub(crate) fn locator_record_path(exec_dir: &Path) -> PathBuf {
    exec_dir.join("locator.json")
}

pub(crate) fn legacy_exec_dir(request_id: &str) -> PathBuf {
    locator_root().join(format!("agentype-exec-{request_id}"))
}

pub(crate) fn jail_exec_dir(request_id: &str) -> PathBuf {
    locator_root().join(format!("agentype-jail-exec-{request_id}"))
}

/// Persist locator evidence before the identity probe.
///
/// The record is written to a unique temporary file in the execution
/// directory, flushed, and atomically renamed over `locator.json`. A crash
/// during an overwrite therefore leaves either the previous complete record or
/// the new complete record, never a truncated one. Callers must treat a
/// failure as a real unknown-state boundary: the process already exists, so
/// the error carries the in-memory locator for the daemon's handle-hint
/// channel; it must not be reported as a clean start.
pub(crate) fn write_locator_record(
    exec_dir: &Path,
    kind: &str,
    locator: &LocatorHandle,
) -> AdapterResult<()> {
    let path = locator_record_path(exec_dir);
    let mut value = json!({
        "v": HANDLE_VERSION,
        "kind": kind,
        "pid": locator.pid,
        "inst": locator.inst,
        "request_id": locator.request_id,
        "stdout": locator.stdout_path.to_string_lossy(),
        "stderr": locator.stderr_path.to_string_lossy(),
        "identity_complete": false,
        "reconnectable": false,
    });
    if let Some(birth) = locator.birth {
        value["birth"] = json!(birth);
    }
    if let Some(spawn_after_ms) = locator.spawn_after_ms {
        value["spawn_after_ms"] = json!(spawn_after_ms);
    }
    write_locator_atomically(&path, value.to_string().as_bytes())
        .map_err(|_| AdapterError::unavailable("cannot persist process locator record"))
}

fn write_locator_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "locator path has no parent directory",
        )
    })?;
    let mut attempt = 0u32;
    let temp = loop {
        let candidate = parent.join(format!(".locator.{}.{}.tmp", std::process::id(), attempt));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                if let Err(err) = file.write_all(bytes).and_then(|_| file.sync_all()) {
                    drop(file);
                    let _ = fs::remove_file(&candidate);
                    return Err(err);
                }
                break candidate;
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && attempt < 32 => {
                attempt += 1;
            }
            Err(err) => return Err(err),
        }
    };
    // `std::fs::rename` replaces the destination on Unix and on Windows
    // (`MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`), so the canonical
    // record is never truncated in place.
    if let Err(err) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(err);
    }
    #[cfg(unix)]
    {
        // Make the directory entry durable too; otherwise a crash after the
        // rename may still leave the previous name on some filesystems.
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

pub(crate) fn read_locator_record(
    exec_dir: &Path,
    expected_kind: &str,
    request_id: &RequestId,
) -> Option<LocatorHandle> {
    let raw = fs::read_to_string(locator_record_path(exec_dir)).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    let handle = RuntimeHandle(value);
    let locator = parse_locator_with_kind(&handle, expected_kind).ok()?;
    (locator.request_id == request_id.as_str()).then_some(locator)
}

/// Parse a non-pin locator handle. This deliberately rejects a handle that
/// claims to be a complete pin: callers must use `parse_handle_with_kind` for
/// control authority and only use this for recovery evidence.
pub(crate) fn parse_locator_with_kind(
    handle: &RuntimeHandle,
    expected_kind: &str,
) -> AdapterResult<LocatorHandle> {
    let obj = handle
        .0
        .as_object()
        .ok_or_else(|| AdapterError::protocol("process locator must be a JSON object"))?;
    let v = obj.get("v").and_then(Value::as_i64).unwrap_or(0);
    if v != HANDLE_VERSION {
        return Err(AdapterError::protocol(
            "unsupported process locator version",
        ));
    }
    let kind = obj.get("kind").and_then(Value::as_str).unwrap_or("");
    if kind != expected_kind {
        return Err(AdapterError::protocol(
            "locator kind does not match the exact adapter; no adapter fallback",
        ));
    }
    if obj.get("identity_complete").and_then(Value::as_bool) != Some(false)
        || obj.get("reconnectable").and_then(Value::as_bool) != Some(false)
    {
        return Err(AdapterError::protocol(
            "handle is not a non-authoritative locator",
        ));
    }
    let pid = obj
        .get("pid")
        .and_then(Value::as_u64)
        .ok_or_else(|| AdapterError::protocol("locator.pid missing"))?;
    let pid = u32::try_from(pid).map_err(|_| AdapterError::protocol("locator.pid out of range"))?;
    let birth = match obj.get("birth") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .ok_or_else(|| AdapterError::protocol("locator.birth must be an integer"))?,
        ),
    };
    let inst = obj
        .get("inst")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AdapterError::protocol("locator.inst missing"))?
        .to_string();
    let request_id = obj
        .get("request_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AdapterError::protocol("locator.request_id missing"))?
        .to_string();
    let stdout = obj
        .get("stdout")
        .and_then(Value::as_str)
        .ok_or_else(|| AdapterError::protocol("locator.stdout missing"))?;
    let stderr = obj
        .get("stderr")
        .and_then(Value::as_str)
        .ok_or_else(|| AdapterError::protocol("locator.stderr missing"))?;
    let spawn_after_ms = obj.get("spawn_after_ms").and_then(Value::as_u64);
    Ok(LocatorHandle {
        pid,
        birth,
        inst,
        request_id,
        stdout_path: PathBuf::from(stdout),
        stderr_path: PathBuf::from(stderr),
        spawn_after_ms,
    })
}

/// Resolve a locator to a full `ParsedHandle` by querying the live process
/// birth identity. `Ok(None)` means the process is gone (the locator is still
/// valid evidence that a process was created).
pub(crate) fn resolve_locator_to_parsed(
    locator: LocatorHandle,
    deadline: &AdapterDeadline,
) -> AdapterResult<Option<ParsedHandle>> {
    let birth = match locator.birth {
        Some(birth) => Some(birth),
        None => process_birth_by_pid(locator.pid, deadline)?,
    };
    Ok(birth.map(|birth| locator.into_parsed(birth)))
}

pub(crate) enum PhysicalControlHandle {
    Pinned(ParsedHandle),
    Locator(LocatorHandle),
}

/// Accept either a complete pin or a non-pin locator for control/recovery.
/// Locators are resolved to a pin only when the live birth identity can be
/// observed; they are never silently upgraded without that observation.
pub(crate) fn parse_control_handle(
    handle: &RuntimeHandle,
    expected_kind: &str,
) -> AdapterResult<PhysicalControlHandle> {
    match parse_handle_with_kind(handle, expected_kind) {
        Ok(parsed) => Ok(PhysicalControlHandle::Pinned(parsed)),
        Err(pinned_err) => match parse_locator_with_kind(handle, expected_kind) {
            Ok(locator) => Ok(PhysicalControlHandle::Locator(locator)),
            Err(_) => Err(pinned_err),
        },
    }
}

#[cfg(test)]
fn parse_handle(handle: &RuntimeHandle) -> AdapterResult<ParsedHandle> {
    parse_handle_with_kind(handle, ADAPTER_KIND)
}

pub(crate) fn parse_handle_with_kind(
    handle: &RuntimeHandle,
    expected_kind: &str,
) -> AdapterResult<ParsedHandle> {
    let obj = handle
        .0
        .as_object()
        .ok_or_else(|| AdapterError::protocol("process handle must be a JSON object"))?;
    let v = obj.get("v").and_then(Value::as_i64).unwrap_or(0);
    if v != HANDLE_VERSION {
        return Err(AdapterError::protocol("unsupported process handle version"));
    }
    let kind = obj.get("kind").and_then(Value::as_str).unwrap_or("");
    if kind != expected_kind {
        return Err(AdapterError::protocol(
            "handle kind does not match the exact adapter; no adapter fallback",
        ));
    }
    if obj.get("identity_complete").and_then(Value::as_bool) == Some(false)
        || obj.get("reconnectable").and_then(Value::as_bool) == Some(false)
    {
        return Err(AdapterError::protocol("locator hint is not reconnectable"));
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
    let inst = obj
        .get("inst")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AdapterError::protocol("handle.inst missing"))?
        .to_string();
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
        inst,
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

/// Successful Scheduler-facing returns must qualify after the last
/// evidence-producing operation. Expired evidence is not an observation.
fn qualify_ok<T>(
    deadline: &AdapterDeadline,
    msg: &'static str,
    hint: Option<&RuntimeHandle>,
    value: T,
) -> AdapterResult<T> {
    require_deadline(deadline, msg, hint)?;
    Ok(value)
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

fn wait_instance_exit(
    pid: u32,
    birth: u64,
    inst: &str,
    stdout_path: &Path,
    deadline: &AdapterDeadline,
) -> AdapterResult<bool> {
    match pin::pin_instance(pid, birth, inst, stdout_path, deadline)? {
        pin::PinOutcome::Gone => Ok(true),
        pin::PinOutcome::Mismatch => Err(AdapterError::other(
            "process identity proven different; not collect-as-ended",
        )),
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
            let n = libc::write(
                fd,
                bytes.as_ptr().add(off) as *const libc::c_void,
                bytes.len() - off,
            );
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

#[cfg(any(test, not(windows)))]
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

/// Production birth query for a numeric PID. Used only by the durable locator
/// recovery path, never as ordinary control authority: a locator is not a pin
/// until this observation succeeds.
pub(crate) fn process_birth_by_pid(
    pid: u32,
    deadline: &AdapterDeadline,
) -> AdapterResult<Option<u64>> {
    require_deadline(
        deadline,
        "deadline exhausted before process birth probe",
        None,
    )?;
    #[cfg(windows)]
    {
        process_birth_windows(pid)
    }
    #[cfg(not(windows))]
    {
        process_stat(pid, deadline).map(|stat| stat.map(|(_, birth)| birth))
    }
}

fn instance_token() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed) as u128;
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    format!("{seq:016x}{t:016x}{pid:08x}")
}

pub(crate) fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn process_birth_from_child(child: &Child, deadline: &AdapterDeadline) -> AdapterResult<u64> {
    require_deadline(
        deadline,
        "deadline exhausted before process birth probe",
        None,
    )?;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        pin::creation_from_handle(child.as_raw_handle())
    }
    #[cfg(not(windows))]
    {
        process_birth(child.id(), deadline)
    }
}

fn instance_presence(
    pid: u32,
    birth: u64,
    inst: &str,
    stdout_path: &Path,
    deadline: &AdapterDeadline,
) -> AdapterResult<InstancePresence> {
    match pin::pin_instance(pid, birth, inst, stdout_path, deadline)? {
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

#[cfg(any(test, not(windows)))]
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

#[cfg(test)]
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
        request: &EnvironmentStartRequest,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<StartObservation> {
        require_deadline(deadline, "start deadline already expired", None)?;
        let mut spec = spec_from_options(request.target_options())?;
        require_deadline(deadline, "start deadline exhausted before spawn", None)?;
        let exec_dir = legacy_exec_dir(request.request_id().as_str());
        fs::create_dir_all(&exec_dir).map_err(|_| diagnostic("cannot create execution dir"))?;
        let stdout_path = exec_dir.join("stdout.txt");
        let stderr_path = exec_dir.join("stderr.txt");
        require_deadline(deadline, "start deadline exhausted before spawn", None)?;
        let token = instance_token();
        spec.env.retain(|(k, _)| k != INSTANCE_ENV);
        spec.env.push((INSTANCE_ENV.to_string(), token.clone()));
        let spawn_after_ms = unix_ms();
        let mut child = spawn_child(&spec, &stdout_path, &stderr_path, deadline)?;
        let pid = child.id();
        let locator_before_identity = LocatorHandle {
            pid,
            birth: None,
            inst: token.clone(),
            request_id: request.request_id().as_str().to_string(),
            stdout_path: stdout_path.clone(),
            stderr_path: stderr_path.clone(),
            spawn_after_ms: Some(spawn_after_ms),
        };
        // Durable evidence exists before the identity probe. If the daemon is
        // killed in the spawn -> identity window, RequestId recovery can still
        // find the physical process (or prove it is gone). A failure here is
        // not a clean start: the process exists, so the locator must travel
        // through the handle-hint channel and the runtime must record unknown
        // physical state.
        let hint = locator_before_identity.to_runtime_handle(ADAPTER_KIND);
        if let Err(err) = write_locator_record(&exec_dir, ADAPTER_KIND, &locator_before_identity) {
            return Err(err.with_handle_hint(hint));
        }
        if let Err(err) = require_deadline(
            deadline,
            "start deadline exhausted after spawn",
            Some(&hint),
        ) {
            kill_child(&mut child);
            let _ = child.try_wait();
            return Err(err);
        }
        let birth = match process_birth_from_child(&child, deadline) {
            Ok(b) => b,
            Err(err) => {
                kill_child(&mut child);
                let _ = child.try_wait();
                return Err(err.with_handle_hint(hint));
            }
        };
        let parsed = ParsedHandle {
            pid,
            birth,
            inst: token,
            request_id: request.request_id().as_str().to_string(),
            stdout_path: stdout_path.clone(),
            stderr_path: stderr_path.clone(),
        };
        let handle = encode_handle(&parsed);
        if let Err(err) = write_locator_record(
            &exec_dir,
            ADAPTER_KIND,
            &LocatorHandle {
                pid,
                birth: Some(birth),
                inst: parsed.inst.clone(),
                request_id: parsed.request_id.clone(),
                stdout_path: stdout_path.clone(),
                stderr_path: stderr_path.clone(),
                spawn_after_ms: Some(spawn_after_ms),
            },
        ) {
            // The pre-birth locator is still intact (atomic replacement), so
            // recovery is not lost; but the upgrade failure is not silently
            // ignored. Returning the exact pin keeps the process controllable
            // while the runtime records an unresolved start.
            return Err(err.with_handle_hint(handle));
        }
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
        if let Err(err) = write_stdin_deadline(stdin, b"", deadline) {
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
                self.live.lock().expect("live map").insert(
                    pid,
                    LiveProcess {
                        child,
                        birth,
                        inst: parsed.inst.clone(),
                    },
                );
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
        let Some(parsed) = self.control_parsed(handle, deadline)?.parsed else {
            // A durable locator proves spawn succeeded; no live birth identity
            // means the process is gone, not that the adapter may retry.
            return qualify_ok(
                deadline,
                "observe deadline exhausted",
                Some(handle),
                liveness_observation(InstancePresence::Ended),
            );
        };
        let mut live = self.live.lock().expect("live map");
        if let Some(proc) = live.get_mut(&parsed.pid) {
            if proc.birth != parsed.birth || proc.inst != parsed.inst {
                drop(live);
                let presence = instance_presence(
                    parsed.pid,
                    parsed.birth,
                    &parsed.inst,
                    &parsed.stdout_path,
                    deadline,
                )?;
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
        let presence = instance_presence(
            parsed.pid,
            parsed.birth,
            &parsed.inst,
            &parsed.stdout_path,
            deadline,
        )?;
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
        let Some(parsed) = self.control_parsed(handle, deadline)?.parsed else {
            return qualify_ok(
                deadline,
                "interrupt deadline exhausted after locator resolution",
                Some(handle),
                liveness_observation(InstancePresence::Ended),
            );
        };
        let trusted_live = {
            let live = self.live.lock().expect("live map");
            live.get(&parsed.pid)
                .is_some_and(|p| p.birth == parsed.birth && p.inst == parsed.inst)
        };
        let token = if trusted_live {
            None
        } else {
            Some(parsed.inst.as_str())
        };
        match pin::pin_instance_with_token_check(
            parsed.pid,
            parsed.birth,
            token,
            &parsed.stdout_path,
            deadline,
        )? {
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
        let Some(parsed) = self.control_parsed(handle, deadline)?.parsed else {
            return qualify_ok(
                deadline,
                "terminate deadline exhausted after locator resolution",
                Some(handle),
                liveness_observation(InstancePresence::Ended),
            );
        };
        let proc = {
            let mut live = self.live.lock().expect("live map");
            match live.remove(&parsed.pid) {
                Some(proc) if proc.birth == parsed.birth && proc.inst == parsed.inst => Some(proc),
                Some(proc) => {
                    live.insert(parsed.pid, proc);
                    None
                }
                None => None,
            }
        };
        if let Some(mut proc) = proc {
            match pin::pin_instance_with_token_check(
                parsed.pid,
                parsed.birth,
                None,
                &parsed.stdout_path,
                deadline,
            )? {
                pin::PinOutcome::Pinned(pinned) => pinned.kill(deadline)?,
                pin::PinOutcome::Gone => {
                    let _ = proc.child.try_wait();
                    return qualify_ok(
                        deadline,
                        "terminate deadline exhausted",
                        Some(handle),
                        liveness_observation(InstancePresence::Ended),
                    );
                }
                pin::PinOutcome::Mismatch => {
                    let _ = proc.child.try_wait();
                    return qualify_ok(
                        deadline,
                        "terminate deadline exhausted",
                        Some(handle),
                        liveness_observation(InstancePresence::Absent),
                    );
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
        match pin::pin_instance(
            parsed.pid,
            parsed.birth,
            &parsed.inst,
            &parsed.stdout_path,
            deadline,
        )? {
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
            pin::PinOutcome::Gone => qualify_ok(
                deadline,
                "terminate deadline exhausted",
                Some(handle),
                liveness_observation(InstancePresence::Ended),
            ),
            pin::PinOutcome::Mismatch => qualify_ok(
                deadline,
                "terminate deadline exhausted",
                Some(handle),
                liveness_observation(InstancePresence::Absent),
            ),
        }
    }

    fn collect_outcome(
        &self,
        handle: &RuntimeHandle,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<PhysicalExecutionOutcome> {
        require_deadline(deadline, "collect deadline already expired", None)?;
        let resolution = self.control_parsed(handle, deadline)?;
        let Some(parsed) = resolution.parsed else {
            return qualify_ok(
                deadline,
                "collect deadline exhausted after locator resolution",
                Some(handle),
                PhysicalExecutionOutcome {
                    physical_state: agentype_adapter_api::PhysicalState::Terminated,
                    exit_status: None,
                    artifact_refs: Some(json!({
                        "stdout": resolution.stdout_path.to_string_lossy(),
                        "stderr": resolution.stderr_path.to_string_lossy(),
                    })),
                    diagnostic: Some(
                        "durable locator witness: spawned process is absent; not a blind retry"
                            .into(),
                    ),
                },
            );
        };
        let proc = {
            let mut live = self.live.lock().expect("live map");
            match live.remove(&parsed.pid) {
                Some(proc) if proc.birth == parsed.birth && proc.inst == parsed.inst => Some(proc),
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
        } else if !wait_instance_exit(
            parsed.pid,
            parsed.birth,
            &parsed.inst,
            &parsed.stdout_path,
            deadline,
        )? {
            return Err(deadline_exceeded_hint(
                "collect deadline exhausted before process exit",
                handle.clone(),
            ));
        }
        require_deadline(
            deadline,
            "collect deadline exhausted after process exit",
            Some(handle),
        )?;
        Ok(PhysicalExecutionOutcome {
            physical_state: agentype_adapter_api::PhysicalState::Terminated,
            exit_status: None,
            artifact_refs: Some(json!({
                "stdout": parsed.stdout_path.to_string_lossy(),
                "stderr": parsed.stderr_path.to_string_lossy(),
            })),
            diagnostic: Some("physical process exited".into()),
        })
    }

    fn reconcile_start(
        &self,
        request_id: &RequestId,
        persisted_handle: Option<&RuntimeHandle>,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<StartObservation> {
        require_deadline(deadline, "reconcile deadline already expired", None)?;
        let control = match persisted_handle {
            Some(handle) => parse_control_handle(handle, ADAPTER_KIND)?,
            None => {
                // Durable RequestId physical query: a daemon killed after
                // spawn but before persisting a handle still leaves locator
                // evidence beside the execution's stdout/stderr.
                let exec_dir = legacy_exec_dir(request_id.as_str());
                match read_locator_record(&exec_dir, ADAPTER_KIND, request_id) {
                    Some(locator) => PhysicalControlHandle::Locator(locator),
                    None => {
                        return Ok(StartObservation {
                            state: ExecutionState::Unknown,
                            runtime_handle: RuntimeHandle::default(),
                            ambiguous: true,
                            detail: Some(
                                "no persisted handle and no durable locator; identity is request_id only"
                                    .into(),
                            ),
                            terminal_confirmed: false,
                            quiescent_confirmed: false,
                        });
                    }
                }
            }
        };
        let (parsed, observed_handle) = match control {
            PhysicalControlHandle::Pinned(parsed) => {
                if parsed.request_id != request_id.as_str() {
                    return Err(AdapterError::protocol(
                        "persisted handle request_id does not match reconcile identity",
                    ));
                }
                (parsed, None)
            }
            PhysicalControlHandle::Locator(locator) => {
                if locator.request_id != request_id.as_str() {
                    return Err(AdapterError::protocol(
                        "persisted locator request_id does not match reconcile identity",
                    ));
                }
                let locator_handle = locator.to_runtime_handle(ADAPTER_KIND);
                match resolve_locator_to_parsed(locator, deadline)? {
                    Some(parsed) => (parsed, Some(locator_handle)),
                    None => {
                        // Spawn succeeded (the locator is written after spawn)
                        // but the process is gone. This is a terminal physical
                        // candidate, never a blind retry.
                        let hint = locator_handle.clone();
                        return qualify_ok(
                            deadline,
                            "reconcile deadline exhausted after locator resolution",
                            Some(&hint),
                            ended_start_observation(locator_handle),
                        );
                    }
                }
            }
        };
        let presence = self.presence_for(&parsed, deadline)?;
        let full_handle = encode_handle(&parsed);
        let observation = match presence {
            InstancePresence::Running => StartObservation {
                state: ExecutionState::Running,
                // Always replace a locator with the exact pin once the live
                // birth identity has actually been observed.
                runtime_handle: full_handle,
                ambiguous: false,
                detail: None,
                terminal_confirmed: false,
                quiescent_confirmed: false,
            },
            InstancePresence::Ended => ended_start_observation(full_handle),
            InstancePresence::Absent => StartObservation {
                state: ExecutionState::Unknown,
                runtime_handle: observed_handle.unwrap_or(full_handle),
                ambiguous: true,
                detail: Some("process instance not found".into()),
                terminal_confirmed: false,
                quiescent_confirmed: false,
            },
        };
        let hint = observation.runtime_handle.clone();
        qualify_ok(
            deadline,
            "reconcile deadline exhausted after observation",
            Some(&hint),
            observation,
        )
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
            "inst": "t",
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
        let no_inst = RuntimeHandle(json!({
            "v": 1,
            "kind": ADAPTER_KIND,
            "pid": 1,
            "birth": 9,
            "request_id": "r",
            "stdout": "a",
            "stderr": "b"
        }));
        assert_eq!(
            parse_handle(&no_inst).unwrap_err().kind(),
            AdapterErrorKind::Protocol
        );
        let empty_req = RuntimeHandle(json!({
            "v": 1,
            "kind": ADAPTER_KIND,
            "pid": 1,
            "birth": 9,
            "inst": "t",
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
            inst: "abc".into(),
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
        assert_eq!(again.inst, "abc");
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
        let locator_root = Path::new("/var/tmp/agentype");
        for (boot, pid, mnt, root_dev, root_ino) in [
            ("", "pid:[1]", "mnt:[1]", "1", "2"),
            ("boot", "", "mnt:[1]", "1", "2"),
            ("boot", "pid:[1]", "", "1", "2"),
            ("boot", "pid:[1]", "mnt:[1]", "", "2"),
            ("boot", "pid:[1]", "mnt:[1]", "1", ""),
            ("unknown-boot", "pid:[1]", "mnt:[1]", "1", "2"),
            ("boot", "unknown-ns", "mnt:[1]", "1", "2"),
        ] {
            let err =
                linux_domain_key(boot, pid, mnt, root_dev, root_ino, locator_root).unwrap_err();
            assert_eq!(err.kind(), AdapterErrorKind::Unavailable);
        }
        let key =
            linux_domain_key("boot-id", "pid:[1]", "mnt:[2]", "8", "1", locator_root).unwrap();
        assert_eq!(
            key.as_str(),
            "linux:boot-id:pid:[1]:mnt:[2]:8:1:locroot=/var/tmp/agentype"
        );
        assert!(!key.as_str().contains("unknown"));
        let other =
            linux_domain_key("boot-id", "pid:[1]", "mnt:[2]", "8", "99", locator_root).unwrap();
        assert_ne!(
            key.as_str(),
            other.as_str(),
            "same ns with different root must not share a binding key"
        );
        let other_root = linux_domain_key(
            "boot-id",
            "pid:[1]",
            "mnt:[2]",
            "8",
            "1",
            Path::new("/other"),
        )
        .unwrap();
        assert_ne!(
            key.as_str(),
            other_root.as_str(),
            "a different locator root must not share a binding key"
        );
    }

    #[test]
    fn qualify_ok_rejects_expired_evidence() {
        let expired = AdapterDeadline::from_instant(
            std::time::Instant::now() - std::time::Duration::from_secs(1),
        );
        let err = qualify_ok(&expired, "evidence after deadline", None, "obs").unwrap_err();
        assert_eq!(err.kind(), AdapterErrorKind::DeadlineExceeded);
    }

    #[test]
    fn incomplete_locator_hint_is_protocol_not_control() {
        let hint = encode_locator_hint(
            9,
            None,
            "tok",
            "req-1",
            Path::new("stdout.txt"),
            Path::new("stderr.txt"),
        );
        let obj = hint.0.as_object().unwrap();
        assert_eq!(obj.get("pid").and_then(Value::as_u64), Some(9));
        assert_eq!(obj.get("inst").and_then(Value::as_str), Some("tok"));
        assert!(obj.get("birth").is_none());
        assert_eq!(
            obj.get("identity_complete").and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            obj.get("reconnectable").and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            parse_handle(&hint).unwrap_err().kind(),
            AdapterErrorKind::Protocol
        );
        let locator = parse_locator_with_kind(&hint, ADAPTER_KIND).unwrap();
        assert_eq!(locator.pid, 9);
        assert_eq!(locator.birth, None);
    }

    #[test]
    fn locator_record_round_trips_and_is_request_bound() {
        let root = std::env::temp_dir().join(format!("agentype-locator-record-{}", unix_ms()));
        fs::create_dir_all(&root).unwrap();
        let request_id = RequestId::new();
        let locator = LocatorHandle {
            pid: 4242,
            birth: Some(99),
            inst: "instance".into(),
            request_id: request_id.as_str().to_string(),
            stdout_path: root.join("stdout.txt"),
            stderr_path: root.join("stderr.txt"),
            spawn_after_ms: Some(1234),
        };
        write_locator_record(&root, ADAPTER_KIND, &locator).unwrap();
        let read = read_locator_record(&root, ADAPTER_KIND, &request_id).unwrap();
        assert_eq!(read, locator);
        assert!(
            read_locator_record(&root, ADAPTER_KIND, &RequestId::new()).is_none(),
            "a locator must never cross RequestId identities"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn interrupted_locator_rewrite_cannot_destroy_the_previous_record() {
        let root = std::env::temp_dir().join(format!("agentype-locator-atomic-{}", unix_ms()));
        fs::create_dir_all(&root).unwrap();
        let request_id = RequestId::new();
        let first = LocatorHandle {
            pid: 4242,
            birth: None,
            inst: "instance".into(),
            request_id: request_id.as_str().to_string(),
            stdout_path: root.join("stdout.txt"),
            stderr_path: root.join("stderr.txt"),
            spawn_after_ms: Some(1234),
        };
        write_locator_record(&root, ADAPTER_KIND, &first).unwrap();
        // Simulate a crash between temp-file creation/write and the atomic
        // rename: a truncated temp file exists beside the canonical record.
        fs::write(root.join(".locator.9999.0.tmp"), b"{\"v\":1,\"pid\":trunc").unwrap();
        assert_eq!(
            read_locator_record(&root, ADAPTER_KIND, &request_id),
            Some(first.clone()),
            "a partial temp file must never replace or corrupt the canonical record"
        );

        let second = LocatorHandle {
            birth: Some(99),
            ..first.clone()
        };
        write_locator_record(&root, ADAPTER_KIND, &second).unwrap();
        assert_eq!(
            read_locator_record(&root, ADAPTER_KIND, &request_id),
            Some(second),
            "a completed overwrite is a whole-record replacement"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn failed_locator_write_preserves_the_previous_record() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("agentype-locator-fail-{}", unix_ms()));
        fs::create_dir_all(&root).unwrap();
        let request_id = RequestId::new();
        let first = LocatorHandle {
            pid: 4242,
            birth: None,
            inst: "instance".into(),
            request_id: request_id.as_str().to_string(),
            stdout_path: root.join("stdout.txt"),
            stderr_path: root.join("stderr.txt"),
            spawn_after_ms: Some(1234),
        };
        write_locator_record(&root, ADAPTER_KIND, &first).unwrap();

        let mut perms = fs::metadata(&root).unwrap().permissions();
        perms.set_mode(0o555);
        fs::set_permissions(&root, perms).unwrap();
        let second = LocatorHandle {
            birth: Some(99),
            ..first.clone()
        };
        let err = write_locator_record(&root, ADAPTER_KIND, &second)
            .expect_err("a read-only directory must fail the atomic replace");
        assert_eq!(err.kind(), AdapterErrorKind::Unavailable);

        let mut perms = fs::metadata(&root).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&root, perms).unwrap();
        assert_eq!(
            read_locator_record(&root, ADAPTER_KIND, &request_id),
            Some(first),
            "a failed overwrite must leave the previous durable record intact"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn complete_pin_is_rejected_as_a_locator() {
        let handle = RuntimeHandle(json!({
            "v": HANDLE_VERSION,
            "kind": ADAPTER_KIND,
            "pid": 1,
            "birth": 2,
            "inst": "tok",
            "request_id": "req",
            "stdout": "a",
            "stderr": "b",
        }));
        assert_eq!(
            parse_locator_with_kind(&handle, ADAPTER_KIND)
                .unwrap_err()
                .kind(),
            AdapterErrorKind::Protocol
        );
    }
}
