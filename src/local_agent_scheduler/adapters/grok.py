from __future__ import annotations

import json
import os
import subprocess
import threading
import time
import uuid
from pathlib import Path
from typing import Any, Callable, Mapping

from ..enums import ExecutionState, FailureClass, WorkspaceMode
from ..errors import AdapterError
from ..models import (
    ExecutionObservation,
    ExecutionOutcome,
    ExecutionRequest,
    StartObservation,
)

GROK_SANDBOXES = {"off", "workspace", "read-only", "strict"}


class AcpSession:
    """JSON-RPC 2.0 stdio session for `grok agent stdio`."""

    def __init__(self, command: tuple[str, ...], process_cwd: str | None, timeout: float):
        if timeout <= 0:
            raise ValueError("timeout must be positive")
        initialization_deadline = time.monotonic() + timeout
        self.session_id = uuid.uuid4().hex
        self.timeout = timeout
        self.process = subprocess.Popen(
            command,
            cwd=process_cwd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
        )
        try:
            self._initialize(initialization_deadline)
        except BaseException:
            self._abandon_process(initialization_deadline)
            raise

    def _initialize(self, initialization_deadline: float) -> None:
        if not self.process.stdin or not self.process.stdout or not self.process.stderr:
            raise AdapterError("failed to open Grok ACP stdio")
        self._responses: dict[int, Mapping[str, Any]] = {}
        self._notifications: list[Mapping[str, Any]] = []
        self._stderr: list[str] = []
        self._condition = threading.Condition()
        self._write_lock = threading.Lock()
        self._next_id = 1
        self._reader = threading.Thread(target=self._read_stdout, daemon=True)
        self._stderr_reader = threading.Thread(target=self._read_stderr, daemon=True)
        self._reader.start()
        self._stderr_reader.start()
        self.request(
            "initialize",
            {
                "protocolVersion": 1,
                "clientInfo": {
                    "name": "local_agent_scheduler",
                    "title": "Local Agent Scheduler",
                    "version": "0.1.3",
                },
                # Empty client capabilities: do not advertise fs/terminal or the
                # agent will call client methods this adapter does not implement.
                "clientCapabilities": {},
            },
            timeout=self._remaining(initialization_deadline, "initialize"),
        )

    def _close_stdio(self) -> None:
        for stream in (self.process.stdin, self.process.stdout, self.process.stderr):
            if stream is None:
                continue
            try:
                stream.close()
            except OSError:
                pass

    def _abandon_process(self, deadline: float, *, terminate: bool = True) -> bool:
        try:
            if self.process.poll() is None:
                remaining = deadline - time.monotonic()
                try:
                    if terminate and remaining > 0:
                        self.process.terminate()
                        try:
                            self.process.wait(timeout=remaining)
                        except subprocess.TimeoutExpired:
                            remaining = deadline - time.monotonic()
                    if self.process.poll() is None:
                        self.process.kill()
                        remaining = deadline - time.monotonic()
                        if remaining > 0:
                            try:
                                self.process.wait(timeout=remaining)
                            except subprocess.TimeoutExpired:
                                pass
                except OSError:
                    pass
        finally:
            self._close_stdio()
        return self.process.poll() is not None

    @staticmethod
    def _remaining(deadline: float, operation: str) -> float:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError(f"Grok ACP operation timed out: {operation}")
        return remaining

    def _read_stdout(self) -> None:
        assert self.process.stdout is not None
        for line in self.process.stdout:
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                continue
            with self._condition:
                if "id" in message:
                    self._responses[int(message["id"])] = message
                else:
                    self._notifications.append(message)
                self._condition.notify_all()

    def _read_stderr(self) -> None:
        assert self.process.stderr is not None
        for line in self.process.stderr:
            self._stderr.append(line.rstrip())

    def _write(self, message: Mapping[str, Any]) -> None:
        if self.process.poll() is not None:
            raise AdapterError(
                f"Grok ACP exited with {self.process.returncode}: {' | '.join(self._stderr[-5:])}"
            )
        assert self.process.stdin is not None
        self.process.stdin.write(json.dumps(message, ensure_ascii=False, separators=(",", ":")) + "\n")
        self.process.stdin.flush()

    def _abort(self) -> None:
        if self.process.poll() is None:
            try:
                self.process.kill()
            except OSError:
                pass
        with self._condition:
            self._condition.notify_all()

    def _write_until(
        self, message: Mapping[str, Any], *, deadline: float, operation: str
    ) -> None:
        completed = threading.Event()
        errors: list[BaseException] = []

        def write() -> None:
            try:
                with self._write_lock:
                    self._write(message)
            except BaseException as exc:
                errors.append(exc)
            finally:
                completed.set()

        threading.Thread(
            target=write,
            name=f"grok-acp-write-{operation}",
            daemon=True,
        ).start()
        remaining = deadline - time.monotonic()
        if not completed.is_set() and (remaining <= 0 or not completed.wait(remaining)):
            self._abort()
            raise TimeoutError(f"Grok ACP request timed out while writing: {operation}")
        if errors:
            self._abort()
            raise errors[0]

    def request(
        self, method: str, params: Mapping[str, Any], *, timeout: float | None = None
    ) -> Mapping[str, Any]:
        operation_timeout = self.timeout if timeout is None else timeout
        deadline = time.monotonic() + operation_timeout
        with self._condition:
            request_id = self._next_id
            self._next_id += 1
        self._write_until(
            {"jsonrpc": "2.0", "method": method, "id": request_id, "params": params},
            deadline=deadline,
            operation=method,
        )
        try:
            with self._condition:
                while request_id not in self._responses:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise TimeoutError(f"Grok ACP request timed out: {method}")
                    self._condition.wait(min(remaining, 0.25))
                    if self.process.poll() is not None and request_id not in self._responses:
                        raise AdapterError(
                            f"Grok ACP exited during {method}: {' | '.join(self._stderr[-5:])}"
                        )
                response = self._responses.pop(request_id)
        except TimeoutError:
            self._abort()
            raise
        if response.get("error"):
            raise AdapterError(f"{method}: {response['error']}")
        result = response.get("result", {})
        return result if isinstance(result, Mapping) else {}

    def notifications(self) -> list[Mapping[str, Any]]:
        with self._condition:
            return list(self._notifications)

    def close(self, *, terminate: bool = True, timeout: float | None = None) -> bool:
        close_timeout = 0.0 if timeout is None else max(0.0, timeout)
        return self._abandon_process(
            time.monotonic() + close_timeout, terminate=terminate
        )


class GrokAcpAdapter:
    """Thin Grok frontend adapter using official ACP stdio (`grok agent stdio`)."""

    def __init__(
        self,
        *,
        command: tuple[str, ...] = ("grok", "agent", "--always-approve", "stdio"),
        process_cwd: str | None = None,
        sandbox: str = "workspace",
        request_timeout: float = 30.0,
        profile_options: Mapping[str, Mapping[str, Any]] | None = None,
        session_factory: Callable[..., AcpSession] = AcpSession,
        persisted_lookup: Callable[[str], Mapping[str, Any] | None] | None = None,
    ) -> None:
        if sandbox not in GROK_SANDBOXES:
            raise ValueError(
                "Grok adapter sandbox must be off, workspace, read-only, or strict"
            )
        if request_timeout <= 0:
            raise ValueError("request_timeout must be positive")
        self.command = command
        self.process_cwd = process_cwd
        self.sandbox = sandbox
        self.request_timeout = request_timeout
        self.profile_options = dict(profile_options or {})
        self.session_factory = session_factory
        self._persisted_lookup = persisted_lookup
        self._sessions: dict[str, AcpSession] = {}
        self._prompt_state: dict[str, dict[str, Any]] = {}

    def _remaining(self, deadline: float, operation: str) -> float:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError(f"Grok adapter method timed out: {operation}")
        return remaining

    def _close_budget(self, deadline: float) -> float:
        return max(0.0, deadline - time.monotonic())

    def _task_sandbox(self, workspace_mode: WorkspaceMode) -> str:
        if workspace_mode is WorkspaceMode.READ_ONLY:
            return "read-only"
        if self.sandbox in {"workspace", "off"}:
            return self.sandbox
        raise RuntimeError("write Task exceeds the configured Grok adapter sandbox ceiling")

    def _spawn_command(
        self, task_sandbox: str, *, model: str | None = None
    ) -> tuple[str, ...]:
        command = list(self.command)
        if "--sandbox" not in command:
            agent_index = command.index("agent") if "agent" in command else 1
            command[agent_index:agent_index] = ["--sandbox", task_sandbox]
        if model and "--model" not in command and "-m" not in command:
            mode_index = next(
                (
                    index
                    for index, token in enumerate(command)
                    if token in {"stdio", "serve", "headless"}
                ),
                len(command),
            )
            command[mode_index:mode_index] = ["--model", model]
        return tuple(command)

    def start_execution(self, request: ExecutionRequest) -> StartObservation:
        method_deadline = time.monotonic() + self.request_timeout
        session: AcpSession | None = None
        runtime_handle: dict[str, Any] = {
            "request_id": request.request_id,
            "execution_id": request.execution_id,
        }
        try:
            task_sandbox = self._task_sandbox(request.workspace_mode)
            options = self.profile_options.get(request.execution_profile, {})
            model = options.get("model") if isinstance(options.get("model"), str) else None
            session = self.session_factory(
                self._spawn_command(task_sandbox, model=model),
                self.process_cwd,
                self._remaining(method_deadline, "start_execution/session"),
            )
            runtime_handle["adapter_session_id"] = session.session_id
            new_params: dict[str, Any] = {
                "cwd": request.cwd,
                "mcpServers": [],
                "_meta": {"yoloMode": True, "sandbox": task_sandbox},
            }
            if "model" in options:
                new_params["_meta"]["model"] = options["model"]
            created = session.request(
                "session/new",
                new_params,
                timeout=self._remaining(method_deadline, "start_execution/session_new"),
            )
            acp_session_id = str(created.get("sessionId") or created.get("session_id") or "")
            if not acp_session_id:
                raise AdapterError("Grok ACP session/new did not return sessionId")
            runtime_handle["session_id"] = acp_session_id
            prompt_id = uuid.uuid4().hex
            runtime_handle["prompt_id"] = prompt_id
            self._sessions[session.session_id] = session
            self._start_prompt(session, acp_session_id, request.prompt, prompt_id)
            return StartObservation(ExecutionState.RUNNING, runtime_handle)
        except TimeoutError as exc:
            if session is not None:
                self._sessions[session.session_id] = session
            return StartObservation(
                ExecutionState.UNKNOWN,
                runtime_handle,
                ambiguous=True,
                failure_class=FailureClass.TIMEOUT,
                failure_code="GROK_ACP_START_TIMEOUT",
                detail=str(exc),
            )
        except Exception as exc:
            if session is not None:
                session.close(timeout=self._close_budget(method_deadline))
            return StartObservation(
                ExecutionState.FAILED,
                {"request_id": request.request_id},
                failure_class=self._classify_failure(str(exc)),
                failure_code="GROK_ACP_START_FAILED",
                detail=str(exc),
            )

    def _start_prompt(
        self, session: AcpSession, acp_session_id: str, prompt: str, prompt_id: str
    ) -> None:
        state: dict[str, Any] = {"done": False, "result": None, "error": None}
        self._prompt_state[session.session_id] = state

        def run() -> None:
            try:
                result = session.request(
                    "session/prompt",
                    {
                        "sessionId": acp_session_id,
                        "prompt": [{"type": "text", "text": prompt}],
                    },
                    timeout=max(session.timeout, 86_400.0),
                )
                state["result"] = result
            except Exception as exc:
                state["error"] = exc
            finally:
                state["done"] = True
                state["prompt_id"] = prompt_id

        threading.Thread(
            target=run, name=f"grok-acp-prompt-{prompt_id}", daemon=True
        ).start()

    def observe_execution(self, runtime_handle: Mapping[str, object]) -> ExecutionObservation:
        return self._observe_until(
            runtime_handle, time.monotonic() + self.request_timeout
        )

    def _live_session(self, runtime_handle: Mapping[str, object]) -> AcpSession | None:
        session_id = runtime_handle.get("adapter_session_id")
        return self._sessions.get(str(session_id)) if session_id else None

    def _close_live_session(
        self, runtime_handle: Mapping[str, object], *, timeout: float
    ) -> bool:
        session_id = runtime_handle.get("adapter_session_id")
        if not session_id:
            return False
        session = self._sessions.pop(str(session_id), None)
        if session is None:
            return False
        return session.close(timeout=timeout)

    def _prompt_observation(self, session: AcpSession) -> ExecutionObservation | None:
        state = self._prompt_state.get(session.session_id)
        if not state or not state.get("done"):
            return None
        if state.get("error") is not None:
            return ExecutionObservation(
                ExecutionState.FAILED,
                terminal_confirmed=True,
                quiescent_confirmed=session.process.poll() is not None,
                detail=str(state["error"]),
            )
        return ExecutionObservation(
            ExecutionState.SUCCEEDED,
            terminal_confirmed=True,
            quiescent_confirmed=session.process.poll() is not None,
        )

    def _observe_until(
        self, runtime_handle: Mapping[str, object], deadline: float
    ) -> ExecutionObservation:
        session = self._live_session(runtime_handle)
        process_detached = False
        if session:
            if not runtime_handle.get("session_id"):
                if session.process.poll() is not None:
                    self._close_live_session(
                        runtime_handle, timeout=self._close_budget(deadline)
                    )
                    return ExecutionObservation(
                        ExecutionState.LOST,
                        terminal_confirmed=True,
                        quiescent_confirmed=True,
                        detail="Grok ACP exited before session identity was established",
                    )
                return ExecutionObservation(
                    ExecutionState.UNKNOWN,
                    detail="live ambiguous start lacks ACP session identity",
                )
            finished = self._prompt_observation(session)
            if finished is not None:
                return finished
            if session.process.poll() is None:
                return ExecutionObservation(ExecutionState.RUNNING)
            self._close_live_session(
                runtime_handle, timeout=self._close_budget(deadline)
            )
            process_detached = True
        stored = self._read_persisted(runtime_handle)
        if stored is not None:
            return stored
        if process_detached:
            return ExecutionObservation(
                ExecutionState.LOST,
                terminal_confirmed=False,
                quiescent_confirmed=False,
                detail="Grok ACP process exited without a confirmed terminal prompt",
            )
        if runtime_handle.get("session_id"):
            return ExecutionObservation(
                ExecutionState.UNKNOWN,
                detail="stored Grok session could not be read",
            )
        return ExecutionObservation(ExecutionState.UNKNOWN, detail="no ACP session handle")

    def collect_outcome(self, runtime_handle: Mapping[str, object]) -> ExecutionOutcome:
        deadline = time.monotonic() + self.request_timeout
        session = self._live_session(runtime_handle)
        if session:
            finished = self._prompt_observation(session)
            if finished is None:
                return ExecutionOutcome(
                    ExecutionState.RUNNING,
                    terminal_confirmed=False,
                    quiescent_confirmed=False,
                )
            text = self._collect_agent_text(session.notifications())
            process_stopped = self._close_live_session(
                runtime_handle, timeout=self._close_budget(deadline)
            )
            if finished.state is ExecutionState.SUCCEEDED:
                payload = self._parse_payload(text)
                return ExecutionOutcome(
                    ExecutionState.SUCCEEDED,
                    payload=payload,
                    summary=text,
                    terminal_confirmed=True,
                    quiescent_confirmed=process_stopped,
                )
            return ExecutionOutcome(
                ExecutionState.FAILED,
                failure_class=FailureClass.EXECUTION_LOST,
                failure_code="GROK_ACP_PROMPT_FAILED",
                failure_signature=finished.detail,
                terminal_confirmed=True,
                quiescent_confirmed=process_stopped,
            )
        observation = self._observe_until(runtime_handle, deadline)
        if observation.state is ExecutionState.RUNNING:
            return ExecutionOutcome(
                ExecutionState.RUNNING,
                failure_class=FailureClass.EXECUTION_LOST,
                failure_code="GROK_SESSION_STILL_RUNNING",
                terminal_confirmed=False,
                quiescent_confirmed=False,
            )
        if observation.state is ExecutionState.SUCCEEDED:
            return ExecutionOutcome(
                ExecutionState.SUCCEEDED,
                payload={"recovered_session_id": runtime_handle.get("session_id")},
                terminal_confirmed=observation.terminal_confirmed,
                quiescent_confirmed=observation.quiescent_confirmed,
            )
        return ExecutionOutcome(
            observation.state,
            failure_class=FailureClass.EXECUTION_LOST,
            failure_code="GROK_ACP_NO_LIVE_SESSION",
            terminal_confirmed=observation.terminal_confirmed,
            quiescent_confirmed=observation.quiescent_confirmed,
        )

    def reconcile_start(
        self, request_id: str, runtime_handle: Mapping[str, object]
    ) -> StartObservation:
        deadline = time.monotonic() + self.request_timeout
        if runtime_handle.get("request_id") not in (None, request_id):
            return StartObservation(
                ExecutionState.UNKNOWN,
                runtime_handle,
                ambiguous=True,
                failure_class=FailureClass.ADAPTER_PROTOCOL_FAILURE,
                failure_code="REQUEST_ID_MISMATCH",
            )
        observation = self._observe_until(runtime_handle, deadline)
        return StartObservation(
            observation.state,
            runtime_handle,
            ambiguous=not observation.terminal_confirmed
            and observation.state
            not in {ExecutionState.RUNNING, ExecutionState.SUCCEEDED},
            failure_class=(
                FailureClass.EXECUTION_LOST
                if observation.state in {ExecutionState.LOST, ExecutionState.UNKNOWN}
                else None
            ),
            detail=observation.detail,
            terminal_confirmed=observation.terminal_confirmed,
            quiescent_confirmed=observation.quiescent_confirmed,
        )

    def interrupt_execution(self, runtime_handle: Mapping[str, object]) -> ExecutionObservation:
        return self._interrupt_until(
            runtime_handle, time.monotonic() + self.request_timeout
        )

    def _interrupt_until(
        self, runtime_handle: Mapping[str, object], deadline: float
    ) -> ExecutionObservation:
        session = self._live_session(runtime_handle)
        acp_session_id = runtime_handle.get("session_id")
        if not session or not acp_session_id:
            return ExecutionObservation(
                ExecutionState.UNKNOWN,
                terminal_confirmed=False,
                quiescent_confirmed=False,
                detail="cannot interrupt an unattached Grok ACP session",
            )
        try:
            session.request(
                "session/cancel",
                {"sessionId": str(acp_session_id)},
                timeout=self._remaining(deadline, "interrupt_execution/cancel"),
            )
        except Exception:
            pass
        process_stopped = self._close_live_session(
            runtime_handle, timeout=self._close_budget(deadline)
        )
        if process_stopped:
            return ExecutionObservation(
                ExecutionState.TERMINATED,
                terminal_confirmed=True,
                quiescent_confirmed=True,
            )
        return ExecutionObservation(
            ExecutionState.UNKNOWN,
            terminal_confirmed=False,
            quiescent_confirmed=False,
            detail="physical quiescence could not be confirmed",
        )

    def terminate_execution(self, runtime_handle: Mapping[str, object]) -> ExecutionObservation:
        deadline = time.monotonic() + self.request_timeout
        interrupted = self._interrupt_until(runtime_handle, deadline)
        if interrupted.quiescent_confirmed:
            return interrupted
        session = self._live_session(runtime_handle)
        if session:
            process_stopped = self._close_live_session(
                runtime_handle, timeout=self._close_budget(deadline)
            )
            if process_stopped:
                return ExecutionObservation(
                    ExecutionState.TERMINATED,
                    terminal_confirmed=True,
                    quiescent_confirmed=True,
                )
        return ExecutionObservation(
            ExecutionState.UNKNOWN,
            terminal_confirmed=interrupted.terminal_confirmed,
            quiescent_confirmed=False,
            detail="physical quiescence could not be confirmed",
        )

    def _read_persisted(
        self, runtime_handle: Mapping[str, object]
    ) -> ExecutionObservation | None:
        session_id = runtime_handle.get("session_id")
        if not session_id:
            return None
        document = None
        if self._persisted_lookup is not None:
            document = self._persisted_lookup(str(session_id))
        else:
            document = self._read_disk_session(str(session_id))
        if document is None:
            return None
        status = str(document.get("status") or document.get("state") or "")
        if status in {"inProgress", "in_progress", "active", "running"}:
            return ExecutionObservation(ExecutionState.RUNNING)
        if status in {"completed", "succeeded"}:
            return ExecutionObservation(
                ExecutionState.SUCCEEDED,
                terminal_confirmed=True,
                quiescent_confirmed=True,
            )
        if status in {"failed", "interrupted", "cancelled"}:
            return ExecutionObservation(
                ExecutionState.FAILED,
                terminal_confirmed=True,
                quiescent_confirmed=True,
            )
        return ExecutionObservation(
            ExecutionState.UNKNOWN,
            detail=f"stored Grok session has unknown status: {status}",
        )

    @staticmethod
    def _read_disk_session(session_id: str) -> Mapping[str, Any] | None:
        root = Path(os.environ.get("GROK_HOME", Path.home() / ".grok"))
        sessions = root / "sessions"
        if not sessions.is_dir():
            return None
        matches = [path for path in sessions.glob(f"*/{session_id}") if path.is_dir()]
        if not matches:
            return None
        summary_path = matches[0] / "summary.json"
        if not summary_path.is_file():
            return {"status": "inProgress"}
        try:
            payload = json.loads(summary_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return {"status": "inProgress"}
        if not isinstance(payload, Mapping):
            return {"status": "inProgress"}
        return payload

    @staticmethod
    def _collect_agent_text(messages: list[Mapping[str, Any]]) -> str:
        chunks: list[str] = []
        for message in messages:
            method = message.get("method")
            params = message.get("params", {})
            if not isinstance(params, Mapping):
                continue
            if method in {"session/update", "x.ai/session/update"}:
                update = params.get("update")
                body = update if isinstance(update, Mapping) else params
                kind = body.get("sessionUpdate") if isinstance(body, Mapping) else None
                if kind == "agent_message_chunk":
                    content = body.get("content", {}) if isinstance(body, Mapping) else {}
                    if isinstance(content, Mapping) and isinstance(content.get("text"), str):
                        chunks.append(str(content["text"]))
                    elif isinstance(body, Mapping) and isinstance(body.get("text"), str):
                        chunks.append(str(body["text"]))
        return "".join(chunks)

    @staticmethod
    def _parse_payload(text: str) -> Mapping[str, Any]:
        try:
            parsed = json.loads(text)
            return parsed if isinstance(parsed, dict) else {"value": parsed}
        except (json.JSONDecodeError, TypeError):
            return {"final_response": text}

    @staticmethod
    def _classify_failure(detail: str) -> FailureClass:
        lowered = detail.lower()
        if "timeout" in lowered or "timed out" in lowered:
            return FailureClass.TIMEOUT
        if "permission" in lowered or "denied" in lowered:
            return FailureClass.PERMISSION_FAILURE
        if "unavailable" in lowered or "overloaded" in lowered:
            return FailureClass.RESOURCE_UNAVAILABLE
        return FailureClass.ADAPTER_PROTOCOL_FAILURE
