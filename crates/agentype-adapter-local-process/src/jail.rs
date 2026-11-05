//! Workspace-jailing Execution Environment Adapter.
//!
//! Unlike [`LocalProcessAgentAdapter`](super::LocalProcessAgentAdapter), this
//! adapter is **provisioning-capable**: it consumes the exact source-private
//! launch descriptor committed with a typed Execution and physically enforces
//! the workspace mode carried by the M5 start request.
//!
//! Enforcement is mechanical, not declared:
//!
//! * Windows: a restricted token (`WRITE_RESTRICTED` plus the `RESTRICTED` SID)
//!   is created and passed to `CreateProcessAsUserW`. Write access therefore
//!   requires both the normal access check **and** a write ACE for `S-1-5-12`.
//!   `ReadOnly` grants no such ACE; `Write` grants modify access on the
//!   per-execution jail root only.
//! * Linux: Landlock is installed in the child via `pre_exec`. The ruleset
//!   handles every write-class filesystem right; `ReadOnly` adds no path rule,
//!   `Write` adds one rule for the per-execution jail root.
//!
//! Network enforcement is `Enabled` only. `attempt_isolation` is not
//! enforceable and is reported as `false`, so an isolating Task is ineligible
//! rather than weakly provisioned.
//!
//! Control (`observe`/`interrupt`/`terminate`/`collect`/`reconcile`) is
//! deliberately shared with the local-process adapter's pinned-identity model:
//! no action is ever taken on a bare numeric PID.

#![allow(unsafe_code)]

#[cfg(unix)]
use super::wait_child_exit;
use super::{
    deadline_exceeded_hint, instance_presence, instance_token, liveness_observation,
    parse_handle_with_kind, qualify_ok, require_deadline, terminated_observation,
    wait_instance_exit, InstancePresence, ParsedHandle,
};
use agentype_adapter_api::{
    AdapterDeadline, AdapterError, AdapterResult, EnvironmentStartRequest, ExecutionAdapter,
    ExecutionObservation, ImportableAdapter, PhysicalExecutionOutcome, PhysicalState,
    RuntimeHandle, StartObservation,
};
use agentype_core::{ExecutionState, RequestId, WorkspaceMode};
use agentype_execution_config::{AdapterBindingKey, NetworkEnforcement};
use serde_json::{Map, Value};
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
#[cfg(unix)]
use std::sync::Mutex;

/// Frozen `adapter_kind` for the jailing driver family.
pub const JAIL_ADAPTER_KIND: &str = "jailed_process";

/// The only descriptor grammar this adapter accepts. The source integration
/// that prepares descriptors must advertise exactly this protocol identity.
pub const JAIL_PROVISIONING_PROTOCOL: &str = "agentype.workspace-jail/v1";

const DESCRIPTOR_PROTOCOL_FIELD: &str = "protocol";
const DESCRIPTOR_COMMAND_FIELD: &str = "command";
const DESCRIPTOR_ARGS_FIELD: &str = "args";
const DESCRIPTOR_ROOT_FIELD: &str = "jail_root";

/// Strict, secret-free launch descriptor produced by a source integration and
/// consumed only by [`JailedProcessAgentAdapter`].
///
/// There is deliberately no environment, credential, or token field: the
/// adapter injects its own instance token, and B.5 credentials remain out of
/// scope. Unknown fields are rejected, so a producer cannot smuggle an
/// undeclared capability through this grammar.
///
/// `jail_root` is a base directory. The adapter creates a fresh
/// `agentype-jail-<request_id>` child for each Execution and applies
/// enforcement there, so two Executions never share a writable root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JailLaunchDescriptor {
    command: String,
    args: Vec<String>,
    jail_root: PathBuf,
}

impl JailLaunchDescriptor {
    pub fn new(
        command: impl Into<String>,
        args: Vec<String>,
        jail_root: impl Into<PathBuf>,
    ) -> AdapterResult<Self> {
        let command = command.into();
        let jail_root = jail_root.into();
        validate_command(&command)?;
        validate_root(&jail_root)?;
        for arg in &args {
            if arg.contains('\0') {
                return Err(AdapterError::protocol(
                    "workspace jail descriptor argument contains NUL",
                ));
            }
        }
        Ok(Self {
            command,
            args,
            jail_root,
        })
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }

    pub fn jail_root(&self) -> &Path {
        &self.jail_root
    }

    /// Canonical, deterministic JSON. Field order is fixed so a producer's
    /// output is stable across runs.
    pub fn encode(&self) -> String {
        let mut map = Map::new();
        map.insert(
            DESCRIPTOR_PROTOCOL_FIELD.to_string(),
            Value::String(JAIL_PROVISIONING_PROTOCOL.to_string()),
        );
        map.insert(
            DESCRIPTOR_COMMAND_FIELD.to_string(),
            Value::String(self.command.clone()),
        );
        map.insert(
            DESCRIPTOR_ARGS_FIELD.to_string(),
            Value::Array(self.args.iter().cloned().map(Value::String).collect()),
        );
        map.insert(
            DESCRIPTOR_ROOT_FIELD.to_string(),
            Value::String(self.jail_root.to_string_lossy().into_owned()),
        );
        Value::Object(map).to_string()
    }

    /// Strict inverse of [`Self::encode`]. Any unknown field, missing field,
    /// wrong type, wrong protocol, or non-absolute path is fail-closed.
    pub fn parse(raw: &str) -> AdapterResult<Self> {
        let value: Value = serde_json::from_str(raw)
            .map_err(|_| AdapterError::protocol("workspace jail descriptor is not valid JSON"))?;
        let object = value.as_object().ok_or_else(|| {
            AdapterError::protocol("workspace jail descriptor must be a JSON object")
        })?;
        for key in object.keys() {
            if !matches!(
                key.as_str(),
                DESCRIPTOR_PROTOCOL_FIELD
                    | DESCRIPTOR_COMMAND_FIELD
                    | DESCRIPTOR_ARGS_FIELD
                    | DESCRIPTOR_ROOT_FIELD
            ) {
                return Err(AdapterError::protocol(
                    "workspace jail descriptor contains an unknown field",
                ));
            }
        }
        let protocol = object
            .get(DESCRIPTOR_PROTOCOL_FIELD)
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AdapterError::protocol("workspace jail descriptor protocol is missing")
            })?;
        if protocol != JAIL_PROVISIONING_PROTOCOL {
            return Err(AdapterError::protocol(
                "workspace jail descriptor protocol is not accepted here",
            ));
        }
        let command = object
            .get(DESCRIPTOR_COMMAND_FIELD)
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AdapterError::protocol("workspace jail descriptor command is missing")
            })?;
        let args = match object.get(DESCRIPTOR_ARGS_FIELD) {
            Some(Value::Array(items)) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    let Some(arg) = item.as_str() else {
                        return Err(AdapterError::protocol(
                            "workspace jail descriptor args must be strings",
                        ));
                    };
                    out.push(arg.to_string());
                }
                out
            }
            _ => {
                return Err(AdapterError::protocol(
                    "workspace jail descriptor args must be an array",
                ))
            }
        };
        let root = object
            .get(DESCRIPTOR_ROOT_FIELD)
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AdapterError::protocol("workspace jail descriptor jail_root is missing")
            })?;
        Self::new(command, args, root)
    }
}

fn validate_command(command: &str) -> AdapterResult<()> {
    let trimmed = command.trim();
    if trimmed.is_empty() || trimmed != command {
        return Err(AdapterError::protocol(
            "workspace jail descriptor command is blank or padded",
        ));
    }
    if command.contains('\0') {
        return Err(AdapterError::protocol(
            "workspace jail descriptor command contains NUL",
        ));
    }
    if !Path::new(command).is_absolute() {
        return Err(AdapterError::protocol(
            "workspace jail descriptor command must be absolute",
        ));
    }
    Ok(())
}

fn validate_root(root: &Path) -> AdapterResult<()> {
    if !root.is_absolute() {
        return Err(AdapterError::protocol(
            "workspace jail descriptor jail_root must be absolute",
        ));
    }
    if root
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(AdapterError::protocol(
            "workspace jail descriptor jail_root may not traverse parents",
        ));
    }
    Ok(())
}

/// One live, adapter-owned jailed process. Only the Unix implementation keeps
/// the child so it can be reaped; Windows control re-pins by birth identity.
#[cfg(unix)]
struct JailLive {
    birth: u64,
    inst: String,
    child: std::process::Child,
}

/// Provenance-preserving workspace jail for typed Executions.
pub struct JailedProcessAgentAdapter {
    #[cfg(unix)]
    live: Mutex<std::collections::HashMap<u32, JailLive>>,
    binding_key: AdapterBindingKey,
}

impl JailedProcessAgentAdapter {
    pub fn try_new() -> AdapterResult<Self> {
        Ok(Self {
            #[cfg(unix)]
            live: Mutex::new(std::collections::HashMap::new()),
            binding_key: super::capture_domain_key()?,
        })
    }

    pub fn binding_key(&self) -> &AdapterBindingKey {
        &self.binding_key
    }
}

#[cfg(unix)]
impl Drop for JailedProcessAgentAdapter {
    fn drop(&mut self) {
        // Dropping adapter ownership is not terminate_execution: reap children
        // that already exited, never kill live ones.
        if let Ok(mut live) = self.live.lock() {
            for (_, mut process) in live.drain() {
                let _ = process.child.try_wait();
            }
        }
    }
}

impl ImportableAdapter for JailedProcessAgentAdapter {
    fn import_kind(&self) -> &str {
        JAIL_ADAPTER_KIND
    }

    fn import_binding_key(&self) -> AdapterBindingKey {
        self.binding_key.clone()
    }

    fn import_attempt_isolation(&self) -> bool {
        // A per-execution workspace is not attempt isolation. Never overclaim.
        false
    }

    fn import_provisioning_protocol(&self) -> Option<&str> {
        Some(JAIL_PROVISIONING_PROTOCOL)
    }

    fn import_enforceable_workspace(&self) -> Vec<WorkspaceMode> {
        #[cfg(any(windows, target_os = "linux"))]
        {
            vec![WorkspaceMode::ReadOnly, WorkspaceMode::Write]
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            Vec::new()
        }
    }

    fn import_enforceable_network(&self) -> Vec<NetworkEnforcement> {
        #[cfg(any(windows, target_os = "linux"))]
        {
            vec![NetworkEnforcement::Enabled]
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            Vec::new()
        }
    }
}

struct Spawned {
    pid: u32,
    birth: u64,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    running: bool,
    #[cfg(unix)]
    child: std::process::Child,
}

fn safe_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn prepare_jail(
    descriptor: &JailLaunchDescriptor,
    request_id: &RequestId,
    mode: WorkspaceMode,
) -> AdapterResult<(PathBuf, PathBuf)> {
    fs::create_dir_all(descriptor.jail_root())
        .map_err(|_| AdapterError::unavailable("cannot create workspace jail base"))?;
    let root = descriptor.jail_root().join(format!(
        "agentype-jail-{}",
        safe_component(request_id.as_str())
    ));
    fs::create_dir(&root).map_err(|_| {
        AdapterError::unavailable("workspace jail root already exists or cannot be created")
    })?;
    let workspace = root.join("workspace");
    fs::create_dir(&workspace)
        .map_err(|_| AdapterError::unavailable("cannot create workspace jail workspace"))?;
    #[cfg(windows)]
    {
        if mode == WorkspaceMode::Write {
            grant_restricted_write(&root)?;
        }
    }
    let _ = mode;
    Ok((root, workspace))
}

impl ExecutionAdapter for JailedProcessAgentAdapter {
    fn start_execution(
        &self,
        request: &EnvironmentStartRequest,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<StartObservation> {
        require_deadline(deadline, "start deadline already expired", None)?;
        if request.attempt_isolation() {
            return Err(AdapterError::unavailable(
                "workspace jail does not enforce attempt isolation",
            ));
        }
        if request.network_policy() != NetworkEnforcement::Enabled {
            return Err(AdapterError::unavailable(
                "workspace jail enforces only enabled network",
            ));
        }
        let raw = request.launch_descriptor().trim();
        if raw.is_empty() {
            return Err(AdapterError::unavailable(
                "workspace jail requires a committed launch descriptor",
            ));
        }
        let descriptor = JailLaunchDescriptor::parse(raw)?;
        let (root, workspace) =
            prepare_jail(&descriptor, request.request_id(), request.workspace_mode())?;
        let inst = instance_token();
        let spawned = spawn_jailed(&descriptor, &root, &workspace, request, &inst, deadline)?;
        let parsed = ParsedHandle {
            pid: spawned.pid,
            birth: spawned.birth,
            inst,
            request_id: request.request_id().as_str().to_string(),
            stdout_path: spawned.stdout_path.clone(),
            stderr_path: spawned.stderr_path.clone(),
        };
        let handle = super::encode_handle_with_kind(&parsed, JAIL_ADAPTER_KIND);
        #[cfg(unix)]
        {
            // Always retain the child, even when it already exited, so the
            // adapter can reap it instead of leaking a zombie.
            self.live.lock().expect("jail live map").insert(
                parsed.pid,
                JailLive {
                    birth: parsed.birth,
                    inst: parsed.inst.clone(),
                    child: spawned.child,
                },
            );
        }
        if !spawned.running {
            return Ok(super::ended_start_observation(handle));
        }
        Ok(StartObservation {
            state: ExecutionState::Running,
            runtime_handle: handle,
            ambiguous: false,
            detail: None,
            terminal_confirmed: false,
            quiescent_confirmed: false,
        })
    }

    fn observe_execution(
        &self,
        handle: &RuntimeHandle,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<ExecutionObservation> {
        require_deadline(deadline, "observe deadline already expired", None)?;
        let parsed = parse_handle_with_kind(handle, JAIL_ADAPTER_KIND)?;
        #[cfg(unix)]
        {
            let mut live = self.live.lock().expect("jail live map");
            if let Some(process) = live.get_mut(&parsed.pid) {
                if process.birth == parsed.birth && process.inst == parsed.inst {
                    match process.child.try_wait() {
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
            }
        }
        let presence = instance_presence(
            parsed.pid,
            parsed.birth,
            &parsed.inst,
            &parsed.stdout_path,
            deadline,
        )?;
        require_deadline(deadline, "observe deadline exhausted", Some(handle))?;
        Ok(liveness_observation(presence))
    }

    fn interrupt_execution(
        &self,
        handle: &RuntimeHandle,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<ExecutionObservation> {
        require_deadline(deadline, "interrupt deadline already expired", None)?;
        let parsed = parse_handle_with_kind(handle, JAIL_ADAPTER_KIND)?;
        match super::pin::pin_instance(
            parsed.pid,
            parsed.birth,
            &parsed.inst,
            &parsed.stdout_path,
            deadline,
        )? {
            super::pin::PinOutcome::Pinned(pinned) => pinned.interrupt(deadline)?,
            super::pin::PinOutcome::Gone | super::pin::PinOutcome::Mismatch => {}
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
        let parsed = parse_handle_with_kind(handle, JAIL_ADAPTER_KIND)?;
        #[cfg(unix)]
        {
            let process = {
                let mut live = self.live.lock().expect("jail live map");
                match live.remove(&parsed.pid) {
                    Some(process)
                        if process.birth == parsed.birth && process.inst == parsed.inst =>
                    {
                        Some(process)
                    }
                    Some(process) => {
                        live.insert(parsed.pid, process);
                        None
                    }
                    None => None,
                }
            };
            if let Some(mut process) = process {
                let _ = process.child.kill();
                if wait_child_exit(&mut process.child, deadline)? {
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
        }
        match super::pin::pin_instance(
            parsed.pid,
            parsed.birth,
            &parsed.inst,
            &parsed.stdout_path,
            deadline,
        )? {
            super::pin::PinOutcome::Pinned(pinned) => {
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
            super::pin::PinOutcome::Gone => qualify_ok(
                deadline,
                "terminate deadline exhausted",
                Some(handle),
                liveness_observation(InstancePresence::Ended),
            ),
            super::pin::PinOutcome::Mismatch => qualify_ok(
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
        let parsed = parse_handle_with_kind(handle, JAIL_ADAPTER_KIND)?;
        #[cfg(unix)]
        {
            let process = {
                let mut live = self.live.lock().expect("jail live map");
                match live.remove(&parsed.pid) {
                    Some(process)
                        if process.birth == parsed.birth && process.inst == parsed.inst =>
                    {
                        Some(process)
                    }
                    Some(process) => {
                        live.insert(parsed.pid, process);
                        None
                    }
                    None => None,
                }
            };
            match process {
                Some(mut process) => {
                    if !wait_child_exit(&mut process.child, deadline)? {
                        self.live
                            .lock()
                            .expect("jail live map")
                            .insert(parsed.pid, process);
                        return Err(deadline_exceeded_hint(
                            "collect deadline exhausted before process exit",
                            handle.clone(),
                        ));
                    }
                }
                None => {
                    if !wait_instance_exit(
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
                }
            }
        }
        #[cfg(windows)]
        {
            if !wait_instance_exit(
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
        }
        require_deadline(
            deadline,
            "collect deadline exhausted after process exit",
            Some(handle),
        )?;
        Ok(PhysicalExecutionOutcome {
            physical_state: PhysicalState::Terminated,
            exit_status: None,
            artifact_refs: Some(serde_json::json!({
                "stdout": parsed.stdout_path.to_string_lossy(),
                "stderr": parsed.stderr_path.to_string_lossy(),
            })),
            diagnostic: Some("jailed physical process exited".into()),
        })
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
        let parsed = match parse_handle_with_kind(handle, JAIL_ADAPTER_KIND) {
            Ok(parsed) => parsed,
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
            #[cfg(unix)]
            {
                let mut live = self.live.lock().expect("jail live map");
                match live.get_mut(&parsed.pid) {
                    Some(process)
                        if process.birth == parsed.birth && process.inst == parsed.inst =>
                    {
                        match process.child.try_wait() {
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
                    _ => {
                        drop(live);
                        instance_presence(
                            parsed.pid,
                            parsed.birth,
                            &parsed.inst,
                            &parsed.stdout_path,
                            deadline,
                        )?
                    }
                }
            }
            #[cfg(windows)]
            {
                instance_presence(
                    parsed.pid,
                    parsed.birth,
                    &parsed.inst,
                    &parsed.stdout_path,
                    deadline,
                )?
            }
        };
        let observation = match presence {
            InstancePresence::Running => StartObservation {
                state: ExecutionState::Running,
                runtime_handle: handle.clone(),
                ambiguous: false,
                detail: None,
                terminal_confirmed: false,
                quiescent_confirmed: false,
            },
            InstancePresence::Ended => super::ended_start_observation(handle.clone()),
            InstancePresence::Absent => StartObservation {
                state: ExecutionState::Unknown,
                runtime_handle: handle.clone(),
                ambiguous: true,
                detail: Some("process instance not found".into()),
                terminal_confirmed: false,
                quiescent_confirmed: false,
            },
        };
        qualify_ok(
            deadline,
            "reconcile deadline exhausted after observation",
            Some(handle),
            observation,
        )
    }
}

// =============================================================================
// Windows: restricted-token spawn
// =============================================================================

#[cfg(windows)]
#[allow(unsafe_code)]
fn spawn_jailed(
    descriptor: &JailLaunchDescriptor,
    _root: &Path,
    workspace: &Path,
    request: &EnvironmentStartRequest,
    _inst: &str,
    _deadline: &AdapterDeadline,
) -> AdapterResult<Spawned> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;

    // A restricted token can only initialize DLLs when the child inherits an
    // existing console; CREATE_NO_WINDOW/CREATE_NEW_CONSOLE children die with
    // STATUS_DLL_INIT_FAILED. Fail closed when this host has no console rather
    // than spawning a process that can never run.
    // SAFETY: pure query.
    if unsafe { GetConsoleCP() } == 0 {
        return Err(AdapterError::unavailable(
            "restricted-token workspace jail requires an inherited Windows console",
        ));
    }

    type Handle = *mut core::ffi::c_void;
    type Dword = u32;

    type Bool = i32;

    const TOKEN_ASSIGN_PRIMARY: Dword = 0x0001;
    const TOKEN_DUPLICATE: Dword = 0x0002;
    const TOKEN_QUERY: Dword = 0x0008;
    const WRITE_RESTRICTED: Dword = 0x0008;
    const HANDLE_FLAG_INHERIT: Dword = 0x0000_0001;
    const STILL_ACTIVE: Dword = 259;
    const STARTF_USESTDHANDLES: Dword = 0x0000_0100;
    const EXTENDED_STARTUPINFO_PRESENT: Dword = 0x0008_0000;
    const PROC_THREAD_ATTRIBUTE_HANDLE_LIST: usize = 0x0002_0002;

    #[repr(C)]
    struct SidAndAttributes {
        sid: *mut core::ffi::c_void,
        attributes: Dword,
    }

    #[repr(C)]
    struct StartupInfoW {
        cb: Dword,
        reserved: *mut u16,
        desktop: *mut u16,
        title: *mut u16,
        x: Dword,
        y: Dword,
        x_size: Dword,
        y_size: Dword,
        x_count_chars: Dword,
        y_count_chars: Dword,
        fill_attribute: Dword,
        flags: Dword,
        show_window: u16,
        reserved2: u16,
        reserved2_ptr: *mut u8,
        std_input: Handle,
        std_output: Handle,
        std_error: Handle,
    }

    #[repr(C)]
    struct StartupInfoExW {
        startup_info: StartupInfoW,
        attribute_list: *mut core::ffi::c_void,
    }

    #[repr(C)]
    struct ProcessInformation {
        process: Handle,
        thread: Handle,
        process_id: Dword,
        thread_id: Dword,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> Handle;
        fn GetConsoleCP() -> Dword;
        fn GetExitCodeProcess(process: Handle, code: *mut Dword) -> Bool;
        fn CloseHandle(handle: Handle) -> Bool;
        fn LocalFree(mem: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
        fn SetHandleInformation(handle: Handle, mask: Dword, flags: Dword) -> Bool;
        fn InitializeProcThreadAttributeList(
            list: *mut core::ffi::c_void,
            count: Dword,
            flags: Dword,
            size: *mut usize,
        ) -> Bool;
        fn UpdateProcThreadAttribute(
            list: *mut core::ffi::c_void,
            flags: Dword,
            attribute: usize,
            value: *const core::ffi::c_void,
            size: usize,
            previous: *mut core::ffi::c_void,
            return_size: *mut usize,
        ) -> Bool;
        fn DeleteProcThreadAttributeList(list: *mut core::ffi::c_void);
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn OpenProcessToken(process: Handle, access: Dword, token: *mut Handle) -> Bool;
        fn CreateRestrictedToken(
            existing: Handle,
            flags: Dword,
            disable_count: Dword,
            disable: *const SidAndAttributes,
            delete_count: Dword,
            delete: *const core::ffi::c_void,
            restrict_count: Dword,
            restrict: *const SidAndAttributes,
            new_token: *mut Handle,
        ) -> Bool;
        fn ConvertStringSidToSidW(string_sid: *const u16, sid: *mut *mut core::ffi::c_void)
            -> Bool;
        #[allow(clippy::too_many_arguments)]
        fn CreateProcessAsUserW(
            token: Handle,
            application: *const u16,
            command_line: *mut u16,
            process_attributes: *mut core::ffi::c_void,
            thread_attributes: *mut core::ffi::c_void,
            inherit_handles: Bool,
            creation_flags: Dword,
            environment: *mut core::ffi::c_void,
            current_directory: *const u16,
            startup_info: *const StartupInfoW,
            process_information: *mut ProcessInformation,
        ) -> Bool;
    }

    fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn quote_arg(arg: &str) -> String {
        if !arg.is_empty() && !arg.chars().any(|ch| ch == ' ' || ch == '\t' || ch == '"') {
            return arg.to_string();
        }
        let mut out = String::with_capacity(arg.len() + 2);
        out.push('"');
        let mut backslashes = 0usize;
        for ch in arg.chars() {
            match ch {
                '\\' => backslashes += 1,
                '"' => {
                    out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                    out.push('"');
                    backslashes = 0;
                }
                _ => {
                    out.extend(std::iter::repeat_n('\\', backslashes));
                    backslashes = 0;
                    out.push(ch);
                }
            }
        }
        out.extend(std::iter::repeat_n('\\', backslashes * 2));
        out.push('"');
        out
    }

    let exec_dir = std::env::temp_dir().join(format!(
        "agentype-jail-exec-{}",
        request.request_id().as_str()
    ));
    fs::create_dir_all(&exec_dir)
        .map_err(|_| AdapterError::unavailable("cannot create jailed execution dir"))?;
    let stdout_path = exec_dir.join("stdout.txt");
    let stderr_path = exec_dir.join("stderr.txt");
    let stdout = File::create(&stdout_path)
        .map_err(|_| AdapterError::unavailable("cannot create stdout"))?;
    let stderr = File::create(&stderr_path)
        .map_err(|_| AdapterError::unavailable("cannot create stderr"))?;
    let stdin =
        File::open("NUL").map_err(|_| AdapterError::unavailable("cannot open null input"))?;

    let raw_handles = [
        stdin.as_raw_handle() as Handle,
        stdout.as_raw_handle() as Handle,
        stderr.as_raw_handle() as Handle,
    ];
    for handle in raw_handles {
        // SAFETY: each handle is owned by a live File for the whole call.
        if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } == 0 {
            return Err(AdapterError::unavailable(
                "cannot mark jailed stdio handles inheritable",
            ));
        }
    }

    let mut base_token: Handle = std::ptr::null_mut();
    // SAFETY: GetCurrentProcess returns a pseudo-handle; base_token is an out
    // parameter owned by this frame and closed below.
    if unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ASSIGN_PRIMARY | TOKEN_DUPLICATE | TOKEN_QUERY,
            &mut base_token,
        )
    } == 0
    {
        return Err(AdapterError::unavailable(
            "cannot open the current process token",
        ));
    }

    let restricted_sid_string = wide(std::ffi::OsStr::new("S-1-5-12"));
    let mut restricted_sid: *mut core::ffi::c_void = std::ptr::null_mut();
    let sid_ok =
        unsafe { ConvertStringSidToSidW(restricted_sid_string.as_ptr(), &mut restricted_sid) };
    if sid_ok == 0 {
        unsafe {
            CloseHandle(base_token);
        }
        return Err(AdapterError::unavailable(
            "cannot resolve the restricted SID",
        ));
    }
    let restrict = SidAndAttributes {
        sid: restricted_sid,
        attributes: 0,
    };
    let mut restricted_token: Handle = std::ptr::null_mut();
    // SAFETY: base_token and restricted_sid are valid; the new token is an out
    // parameter. CreateRestrictedToken copies the SID.
    let token_ok = unsafe {
        CreateRestrictedToken(
            base_token,
            WRITE_RESTRICTED,
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            1,
            &restrict,
            &mut restricted_token,
        )
    };
    unsafe {
        CloseHandle(base_token);
        LocalFree(restricted_sid);
    }
    if token_ok == 0 {
        return Err(AdapterError::unavailable(
            "cannot create a restricted process token",
        ));
    }

    let application = wide(std::ffi::OsStr::new(descriptor.command()));
    let mut command_line = quote_arg(descriptor.command());
    for arg in descriptor.args() {
        command_line.push(' ');
        command_line.push_str(&quote_arg(arg));
    }
    let mut command_line: Vec<u16> = command_line
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let current_directory = wide(workspace.as_os_str());

    // Restrict inheritance to exactly the three stdio handles.
    let mut list_size = 0usize;
    // SAFETY: first call is the documented size probe; it fails by design.
    unsafe {
        InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut list_size);
    }
    let mut list_words = vec![0u64; list_size.div_ceil(std::mem::size_of::<u64>())];
    let list = list_words.as_mut_ptr() as *mut core::ffi::c_void;
    // SAFETY: buffer is at least list_size bytes and suitably aligned.
    if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut list_size) } == 0 {
        unsafe {
            CloseHandle(restricted_token);
        }
        return Err(AdapterError::unavailable(
            "cannot initialize the process attribute list",
        ));
    }
    let handles = raw_handles;
    // SAFETY: the handle array outlives CreateProcessAsUserW; the attribute
    // list buffer owns its own state.
    let attr_ok = unsafe {
        UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            handles.as_ptr() as *const core::ffi::c_void,
            std::mem::size_of_val(&handles),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if attr_ok == 0 {
        unsafe {
            DeleteProcThreadAttributeList(list);
            CloseHandle(restricted_token);
        }
        return Err(AdapterError::unavailable(
            "cannot restrict inherited handles",
        ));
    }

    let mut startup: StartupInfoExW = unsafe { std::mem::zeroed() };
    startup.startup_info.cb = std::mem::size_of::<StartupInfoExW>() as Dword;
    startup.startup_info.flags = STARTF_USESTDHANDLES;
    startup.startup_info.std_input = raw_handles[0];
    startup.startup_info.std_output = raw_handles[1];
    startup.startup_info.std_error = raw_handles[2];
    startup.attribute_list = list;

    let mut process_info: ProcessInformation = unsafe { std::mem::zeroed() };
    // SAFETY: all pointers reference live, correctly typed values for the
    // duration of the call.
    let created = unsafe {
        CreateProcessAsUserW(
            restricted_token,
            application.as_ptr(),
            command_line.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            1,
            EXTENDED_STARTUPINFO_PRESENT,
            std::ptr::null_mut(),
            current_directory.as_ptr(),
            &startup.startup_info,
            &mut process_info,
        )
    };
    unsafe {
        DeleteProcThreadAttributeList(list);
        CloseHandle(restricted_token);
    }
    for handle in raw_handles {
        // SAFETY: the handle is still owned by its File; clearing the inherit
        // flag only affects future spawns.
        unsafe {
            SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
        }
    }
    if created == 0 {
        return Err(AdapterError::unavailable(
            "CreateProcessAsUser failed for the workspace jail",
        ));
    }
    unsafe {
        CloseHandle(process_info.thread);
    }

    let birth = match super::pin::creation_from_handle(process_info.process) {
        Ok(birth) => birth,
        Err(err) => {
            unsafe {
                CloseHandle(process_info.process);
            }
            return Err(err);
        }
    };
    let mut code = STILL_ACTIVE;
    // SAFETY: process_info.process is a valid process handle.
    let query_ok = unsafe { GetExitCodeProcess(process_info.process, &mut code) } != 0;
    unsafe {
        CloseHandle(process_info.process);
    }
    if !query_ok {
        return Err(AdapterError::other("GetExitCodeProcess failed after spawn"));
    }

    Ok(Spawned {
        pid: process_info.process_id,
        birth,
        stdout_path,
        stderr_path,
        running: code == STILL_ACTIVE,
    })
}

#[cfg(windows)]
fn grant_restricted_write(root: &Path) -> AdapterResult<()> {
    use std::process::{Command, Stdio};
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Windows"));
    let icacls = system_root.join("System32").join("icacls.exe");
    let status = Command::new(icacls)
        .arg(root)
        .arg("/grant")
        .arg("*S-1-5-12:(OI)(CI)(M)")
        .arg("/T")
        .arg("/C")
        .arg("/Q")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| AdapterError::unavailable("workspace jail ACL tool unavailable"))?;
    if !status.success() {
        return Err(AdapterError::unavailable("workspace jail ACL grant failed"));
    }
    Ok(())
}

// =============================================================================
// Linux: Landlock spawn
// =============================================================================

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn spawn_jailed(
    descriptor: &JailLaunchDescriptor,
    root: &Path,
    workspace: &Path,
    request: &EnvironmentStartRequest,
    inst: &str,
    deadline: &AdapterDeadline,
) -> AdapterResult<Spawned> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let exec_dir = std::env::temp_dir().join(format!(
        "agentype-jail-exec-{}",
        request.request_id().as_str()
    ));
    fs::create_dir_all(&exec_dir)
        .map_err(|_| AdapterError::unavailable("cannot create jailed execution dir"))?;
    let stdout_path = exec_dir.join("stdout.txt");
    let stderr_path = exec_dir.join("stderr.txt");
    let stdout = File::create(&stdout_path)
        .map_err(|_| AdapterError::unavailable("cannot create stdout"))?;
    let stderr = File::create(&stderr_path)
        .map_err(|_| AdapterError::unavailable("cannot create stderr"))?;

    let jail_root = CString::new(root.as_os_str().as_bytes())
        .map_err(|_| AdapterError::protocol("workspace jail root contains NUL"))?;
    let mode = request.workspace_mode();
    let mut command = Command::new(descriptor.command());
    command
        .args(descriptor.args())
        .current_dir(workspace)
        .env("AGENTYPE_INSTANCE", inst)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    // SAFETY: the closure only performs async-signal-safe syscalls and moves
    // pre-allocated data into the forked child.
    unsafe {
        command.pre_exec(move || apply_landlock(&jail_root, mode));
    }
    let mut child = command
        .spawn()
        .map_err(|_| AdapterError::unavailable("workspace jail spawn failed"))?;
    let pid = child.id();
    let birth = match super::process_birth(pid, deadline) {
        Ok(birth) => birth,
        Err(err) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err);
        }
    };
    let running = match child.try_wait() {
        Ok(Some(_)) => false,
        Ok(None) => true,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AdapterError::other("child wait failed after spawn"));
        }
    };
    Ok(Spawned {
        pid,
        birth,
        stdout_path,
        stderr_path,
        running,
        child,
    })
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn apply_landlock(root: &std::ffi::CString, mode: WorkspaceMode) -> std::io::Result<()> {
    const SYS_LANDLOCK_CREATE_RULESET: libc::c_long = 444;
    const SYS_LANDLOCK_ADD_RULE: libc::c_long = 445;
    const SYS_LANDLOCK_RESTRICT_SELF: libc::c_long = 446;
    const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
    const LANDLOCK_RULE_PATH_BENEATH: u32 = 1;
    const PR_SET_NO_NEW_PRIVS: libc::c_int = 38;

    const ACCESS_WRITE_FILE: u64 = 1 << 1;
    const ACCESS_REMOVE_DIR: u64 = 1 << 4;
    const ACCESS_REMOVE_FILE: u64 = 1 << 5;
    const ACCESS_MAKE_CHAR: u64 = 1 << 6;
    const ACCESS_MAKE_DIR: u64 = 1 << 7;
    const ACCESS_MAKE_REG: u64 = 1 << 8;
    const ACCESS_MAKE_SOCK: u64 = 1 << 9;
    const ACCESS_MAKE_FIFO: u64 = 1 << 10;
    const ACCESS_MAKE_BLOCK: u64 = 1 << 11;
    const ACCESS_MAKE_SYM: u64 = 1 << 12;
    const ACCESS_REFER: u64 = 1 << 13;
    const ACCESS_TRUNCATE: u64 = 1 << 14;

    #[repr(C)]
    struct RulesetAttr {
        handled_access_fs: u64,
    }

    #[repr(C, packed)]
    struct PathBeneathAttr {
        allowed_access: u64,
        parent_fd: i32,
    }

    fn last_error() -> std::io::Error {
        std::io::Error::last_os_error()
    }

    // SAFETY: prctl with PR_SET_NO_NEW_PRIVS is always safe to call.
    let rc = unsafe { libc::prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if rc != 0 {
        return Err(last_error());
    }

    // ABI probe: a NULL ruleset with VERSION returns the supported ABI.
    // SAFETY: documented probe call; no memory is written.
    let abi = unsafe {
        libc::syscall(
            SYS_LANDLOCK_CREATE_RULESET,
            std::ptr::null::<u8>(),
            0usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };
    if abi < 0 {
        return Err(last_error());
    }
    let abi = abi as u64;

    let mut handled = ACCESS_WRITE_FILE
        | ACCESS_REMOVE_DIR
        | ACCESS_REMOVE_FILE
        | ACCESS_MAKE_CHAR
        | ACCESS_MAKE_DIR
        | ACCESS_MAKE_REG
        | ACCESS_MAKE_SOCK
        | ACCESS_MAKE_FIFO
        | ACCESS_MAKE_BLOCK
        | ACCESS_MAKE_SYM;
    if abi >= 2 {
        handled |= ACCESS_REFER;
    }
    if abi >= 3 {
        handled |= ACCESS_TRUNCATE;
    }

    let attr = RulesetAttr {
        handled_access_fs: handled,
    };
    // SAFETY: attr is a valid ruleset attribute for the probed ABI.
    let ruleset = unsafe {
        libc::syscall(
            SYS_LANDLOCK_CREATE_RULESET,
            &attr as *const RulesetAttr as *const libc::c_void,
            std::mem::size_of::<RulesetAttr>(),
            0u32,
        )
    };
    if ruleset < 0 {
        return Err(last_error());
    }
    let ruleset = ruleset as i32;

    if mode == WorkspaceMode::Write {
        // SAFETY: root is a NUL-terminated path; O_PATH yields a rule fd.
        let fd = unsafe { libc::open(root.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
        if fd < 0 {
            let err = last_error();
            unsafe {
                libc::close(ruleset);
            }
            return Err(err);
        }
        let rule = PathBeneathAttr {
            allowed_access: handled,
            parent_fd: fd,
        };
        // SAFETY: rule points at a valid packed rule attribute and fd.
        let added = unsafe {
            libc::syscall(
                SYS_LANDLOCK_ADD_RULE,
                ruleset,
                LANDLOCK_RULE_PATH_BENEATH,
                &rule as *const PathBeneathAttr as *const libc::c_void,
                0u32,
            )
        };
        unsafe {
            libc::close(fd);
        }
        if added < 0 {
            let err = last_error();
            unsafe {
                libc::close(ruleset);
            }
            return Err(err);
        }
    }

    // SAFETY: ruleset is a valid fd; restrict_self applies it to this thread
    // (the child) and does not return on success until exec.
    let restricted = unsafe { libc::syscall(SYS_LANDLOCK_RESTRICT_SELF, ruleset, 0u32) };
    unsafe {
        libc::close(ruleset);
    }
    if restricted < 0 {
        return Err(last_error());
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "linux")))]
fn spawn_jailed(
    _descriptor: &JailLaunchDescriptor,
    _root: &Path,
    _workspace: &Path,
    _request: &EnvironmentStartRequest,
    _inst: &str,
    _deadline: &AdapterDeadline,
) -> AdapterResult<Spawned> {
    // `import_enforceable_workspace` is empty on these hosts, so typed
    // acquisition must already have rejected the adapter. Keep the physical
    // start fail-closed rather than launching unjailed.
    Err(AdapterError::unavailable(
        "workspace jail enforcement exists only on Windows and Linux",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor() -> JailLaunchDescriptor {
        JailLaunchDescriptor::new(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/bin/fake-agent.rs")
                .to_string_lossy()
                .into_owned(),
            vec!["--sleep-ms".into(), "1".into()],
            std::env::temp_dir(),
        )
        .unwrap()
    }

    #[test]
    fn descriptor_round_trips_and_is_strict() {
        let descriptor = descriptor();
        let encoded = descriptor.encode();
        assert_eq!(JailLaunchDescriptor::parse(&encoded).unwrap(), descriptor);

        let mut value: Value = serde_json::from_str(&encoded).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("env".into(), Value::Object(Map::new()));
        assert!(JailLaunchDescriptor::parse(&value.to_string()).is_err());

        let mut value: Value = serde_json::from_str(&encoded).unwrap();
        value.as_object_mut().unwrap().insert(
            "protocol".into(),
            Value::String("agentype.workspace-jail/v2".into()),
        );
        assert!(JailLaunchDescriptor::parse(&value.to_string()).is_err());
    }

    #[test]
    fn descriptor_rejects_relative_command_and_traversal() {
        assert!(JailLaunchDescriptor::new("relative.exe", vec![], std::env::temp_dir()).is_err());
        let traversal = std::env::temp_dir().join("..").join("escape");
        assert!(JailLaunchDescriptor::new("C:\\x.exe", vec![], traversal).is_err());
    }

    #[test]
    fn descriptor_is_secret_free() {
        let encoded = descriptor().encode();
        let lower = encoded.to_ascii_lowercase();
        for forbidden in ["secret", "token", "password", "apikey", "api_key", "env"] {
            assert!(
                !lower.contains(&format!("\"{forbidden}\"")),
                "descriptor must not contain {forbidden}"
            );
        }
    }
}
