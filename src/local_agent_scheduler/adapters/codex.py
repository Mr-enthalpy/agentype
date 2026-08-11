from __future__ import annotations

import json
import queue
import subprocess
import threading
import time
import uuid
from pathlib import Path
from typing import Any, Callable, Mapping

from ..enums import ExecutionState, FailureClass
from ..errors import AdapterError
from ..models import (
    ExecutionObservation,
    ExecutionOutcome,
    ExecutionRequest,
    StartObservation,
)


class AppServerSession:
    def __init__(self, command: tuple[str, ...], process_cwd: str | None, timeout: float):
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
        if not self.process.stdin or not self.process.stdout or not self.process.stderr:
            raise AdapterError("failed to open Codex app-server stdio")
        self._responses: dict[int, Mapping[str, Any]] = {}
        self._notifications: list[Mapping[str, Any]] = []
        self._stderr: list[str] = []
        self._condition = threading.Condition()
        self._next_id = 1
        self._reader = threading.Thread(target=self._read_stdout, daemon=True)
        self._stderr_reader = threading.Thread(target=self._read_stderr, daemon=True)
        self._reader.start()
        self._stderr_reader.start()
        self.request(
            "initialize",
            {
                "clientInfo": {
                    "name": "local_agent_scheduler",
                    "title": "Local Agent Scheduler",
                    "version": "0.1.0",
                }
            },
        )
        self.notify("initialized", {})

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
                f"Codex app-server exited with {self.process.returncode}: {' | '.join(self._stderr[-5:])}"
            )
        assert self.process.stdin is not None
        self.process.stdin.write(json.dumps(message, ensure_ascii=False, separators=(",", ":")) + "\n")
        self.process.stdin.flush()

    def request(
        self, method: str, params: Mapping[str, Any], *, timeout: float | None = None
    ) -> Mapping[str, Any]:
        with self._condition:
            request_id = self._next_id
            self._next_id += 1
        self._write({"method": method, "id": request_id, "params": params})
        deadline = time.monotonic() + (self.timeout if timeout is None else timeout)
        with self._condition:
            while request_id not in self._responses:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError(f"Codex app-server request timed out: {method}")
                self._condition.wait(min(remaining, 0.25))
                if self.process.poll() is not None and request_id not in self._responses:
                    raise AdapterError(
                        f"Codex app-server exited during {method}: {' | '.join(self._stderr[-5:])}"
                    )
            response = self._responses.pop(request_id)
        if response.get("error"):
            raise AdapterError(f"{method}: {response['error']}")
        return response.get("result", {})

    def notify(self, method: str, params: Mapping[str, Any]) -> None:
        self._write({"method": method, "params": params})

    def notifications(self) -> list[Mapping[str, Any]]:
        with self._condition:
            return list(self._notifications)

    def close(self, *, terminate: bool = True) -> bool:
        if self.process.poll() is None and terminate:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        return self.process.poll() is not None


class CodexAppServerAdapter:
    """Thin Codex frontend adapter using the official app-server stdio protocol."""

    def __init__(
        self,
        *,
        command: tuple[str, ...] = ("codex", "app-server"),
        process_cwd: str | None = None,
        approval_policy: str = "never",
        sandbox: str = "workspaceWrite",
        request_timeout: float = 30.0,
        profile_options: Mapping[str, Mapping[str, Any]] | None = None,
        session_factory: Callable[..., AppServerSession] = AppServerSession,
    ) -> None:
        self.command = command
        self.process_cwd = process_cwd
        self.approval_policy = approval_policy
        self.sandbox = sandbox
        self.request_timeout = request_timeout
        self.profile_options = dict(profile_options or {})
        self.session_factory = session_factory
        self._sessions: dict[str, AppServerSession] = {}

    def start_execution(self, request: ExecutionRequest) -> StartObservation:
        session: AppServerSession | None = None
        runtime_handle: dict[str, Any] = {
            "request_id": request.request_id,
            "execution_id": request.execution_id,
        }
        try:
            session = self.session_factory(self.command, self.process_cwd, self.request_timeout)
            runtime_handle["adapter_session_id"] = session.session_id
            thread_params: dict[str, Any] = {
                "cwd": request.cwd,
                "approvalPolicy": self.approval_policy,
                "sandbox": self.sandbox,
                "serviceName": "local_agent_scheduler",
            }
            options = self.profile_options.get(request.execution_profile, {})
            for key in ("model", "personality"):
                if key in options:
                    thread_params[key] = options[key]
            thread_result = session.request("thread/start", thread_params)
            thread_id = thread_result["thread"]["id"]
            runtime_handle["thread_id"] = thread_id
            turn_params: dict[str, Any] = {
                "threadId": thread_id,
                "input": [{"type": "text", "text": request.prompt}],
                "cwd": request.cwd,
                "approvalPolicy": self.approval_policy,
            }
            if "effort" in options:
                turn_params["effort"] = options["effort"]
            turn_result = session.request("turn/start", turn_params)
            turn_id = turn_result["turn"]["id"]
            runtime_handle["turn_id"] = turn_id
            self._sessions[session.session_id] = session
            return StartObservation(
                ExecutionState.RUNNING,
                runtime_handle,
            )
        except TimeoutError as exc:
            if session is not None:
                self._sessions[session.session_id] = session
            return StartObservation(
                ExecutionState.UNKNOWN,
                runtime_handle,
                ambiguous=True,
                failure_class=FailureClass.TIMEOUT,
                failure_code="APP_SERVER_START_TIMEOUT",
                detail=str(exc),
            )
        except Exception as exc:
            if session is not None:
                session.close()
            failure_class = self._classify_failure(str(exc))
            return StartObservation(
                ExecutionState.FAILED,
                {"request_id": request.request_id},
                failure_class=failure_class,
                failure_code="APP_SERVER_START_FAILED",
                detail=str(exc),
            )

    def observe_execution(self, runtime_handle: Mapping[str, object]) -> ExecutionObservation:
        session = self._live_session(runtime_handle)
        if session:
            terminal = self._terminal_notification(session, runtime_handle)
            if terminal:
                status = self._turn_status(terminal)
                state = ExecutionState.SUCCEEDED if status == "completed" else ExecutionState.FAILED
                return ExecutionObservation(state, terminal_confirmed=True, quiescent_confirmed=True)
            if session.process.poll() is not None:
                return ExecutionObservation(
                    ExecutionState.LOST,
                    terminal_confirmed=True,
                    quiescent_confirmed=True,
                    detail="app-server process exited without a turn completion event",
                )
            return ExecutionObservation(ExecutionState.RUNNING)
        thread_id = runtime_handle.get("thread_id")
        if not thread_id:
            return ExecutionObservation(ExecutionState.UNKNOWN, detail="no thread handle")
        turn_id = runtime_handle.get("turn_id")
        return self._read_stored_thread(
            str(thread_id), str(turn_id) if turn_id is not None else None
        )

    def collect_outcome(self, runtime_handle: Mapping[str, object]) -> ExecutionOutcome:
        session = self._live_session(runtime_handle)
        if not session:
            thread_id = runtime_handle.get("thread_id")
            turn_id = runtime_handle.get("turn_id")
            recovered = (
                self._read_stored_outcome(
                    str(thread_id), str(turn_id) if turn_id is not None else None
                )
                if thread_id
                else None
            )
            if recovered is not None:
                return recovered
            observation = self.observe_execution(runtime_handle)
            return ExecutionOutcome(
                observation.state,
                failure_class=FailureClass.EXECUTION_LOST,
                failure_code="NO_LIVE_SESSION",
                failure_signature="CODEX_SESSION_NOT_ATTACHED",
                terminal_confirmed=observation.terminal_confirmed,
                quiescent_confirmed=observation.quiescent_confirmed,
            )
        terminal = self._terminal_notification(session, runtime_handle)
        if not terminal:
            return ExecutionOutcome(
                ExecutionState.RUNNING,
                terminal_confirmed=False,
                quiescent_confirmed=False,
            )
        status = self._turn_status(terminal)
        text = self._collect_agent_text(session.notifications())
        self._close_live_session(runtime_handle)
        if status == "completed":
            payload: Mapping[str, Any]
            try:
                parsed = json.loads(text)
                payload = parsed if isinstance(parsed, dict) else {"value": parsed}
            except (json.JSONDecodeError, TypeError):
                payload = {"final_response": text}
            return ExecutionOutcome(
                ExecutionState.SUCCEEDED,
                payload=payload,
                summary=text,
                terminal_confirmed=True,
                quiescent_confirmed=True,
            )
        detail = json.dumps(terminal, ensure_ascii=False, sort_keys=True)
        return ExecutionOutcome(
            ExecutionState.FAILED,
            failure_class=self._classify_failure(detail),
            failure_code="CODEX_TURN_FAILED",
            failure_signature=self._normalized_signature(detail),
            terminal_confirmed=True,
            quiescent_confirmed=True,
        )

    def reconcile_start(
        self, request_id: str, runtime_handle: Mapping[str, object]
    ) -> StartObservation:
        if runtime_handle.get("request_id") not in (None, request_id):
            return StartObservation(
                ExecutionState.UNKNOWN,
                runtime_handle,
                ambiguous=True,
                failure_class=FailureClass.ADAPTER_PROTOCOL_FAILURE,
                failure_code="REQUEST_ID_MISMATCH",
            )
        observation = self.observe_execution(runtime_handle)
        if observation.state in (ExecutionState.RUNNING, ExecutionState.SUCCEEDED):
            return StartObservation(observation.state, runtime_handle)
        return StartObservation(
            observation.state,
            runtime_handle,
            ambiguous=not observation.terminal_confirmed,
            failure_class=(
                FailureClass.EXECUTION_LOST
                if observation.state in (ExecutionState.LOST, ExecutionState.UNKNOWN)
                else None
            ),
            detail=observation.detail,
        )

    def interrupt_execution(self, runtime_handle: Mapping[str, object]) -> ExecutionObservation:
        session = self._live_session(runtime_handle)
        if not session:
            return ExecutionObservation(
                ExecutionState.UNKNOWN,
                terminal_confirmed=False,
                quiescent_confirmed=False,
                detail="cannot interrupt an unattached Codex session",
            )
        thread_id = runtime_handle.get("thread_id")
        turn_id = runtime_handle.get("turn_id")
        if not thread_id or not turn_id:
            return ExecutionObservation(ExecutionState.UNKNOWN, detail="missing thread/turn handle")
        try:
            session.request(
                "turn/interrupt",
                {"threadId": str(thread_id), "turnId": str(turn_id)},
                timeout=10,
            )
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                terminal = self._terminal_notification(session, runtime_handle)
                if terminal:
                    return ExecutionObservation(
                        ExecutionState.TERMINATED,
                        terminal_confirmed=True,
                        quiescent_confirmed=True,
                    )
                time.sleep(0.05)
            return ExecutionObservation(
                ExecutionState.UNKNOWN,
                terminal_confirmed=False,
                quiescent_confirmed=False,
                detail="interrupt accepted but terminal event not observed",
            )
        except Exception as exc:
            return ExecutionObservation(ExecutionState.UNKNOWN, detail=str(exc))

    def terminate_execution(self, runtime_handle: Mapping[str, object]) -> ExecutionObservation:
        interrupted = self.interrupt_execution(runtime_handle)
        session = self._live_session(runtime_handle)
        process_stopped = session.close() if session else False
        if interrupted.terminal_confirmed and process_stopped:
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

    def _live_session(self, runtime_handle: Mapping[str, object]) -> AppServerSession | None:
        session_id = runtime_handle.get("adapter_session_id")
        return self._sessions.get(str(session_id)) if session_id else None

    def _close_live_session(self, runtime_handle: Mapping[str, object]) -> None:
        session_id = runtime_handle.get("adapter_session_id")
        if not session_id:
            return
        session = self._sessions.pop(str(session_id), None)
        if session is not None:
            session.close()

    @staticmethod
    def _turn_status(notification: Mapping[str, Any]) -> str:
        params = notification.get("params", {})
        turn = params.get("turn", {}) if isinstance(params, Mapping) else {}
        return str(turn.get("status", "failed"))

    @staticmethod
    def _terminal_notification(
        session: AppServerSession, runtime_handle: Mapping[str, object]
    ) -> Mapping[str, Any] | None:
        expected_turn = str(runtime_handle.get("turn_id", ""))
        for message in reversed(session.notifications()):
            if message.get("method") != "turn/completed":
                continue
            params = message.get("params", {})
            turn = params.get("turn", {}) if isinstance(params, Mapping) else {}
            if not expected_turn or str(turn.get("id", "")) == expected_turn:
                return message
        return None

    @staticmethod
    def _collect_agent_text(messages: list[Mapping[str, Any]]) -> str:
        deltas: list[str] = []
        completed_text: str | None = None
        for message in messages:
            method = message.get("method")
            params = message.get("params", {})
            if not isinstance(params, Mapping):
                continue
            if method == "item/agentMessage/delta" and isinstance(params.get("delta"), str):
                deltas.append(str(params["delta"]))
            if method == "item/completed":
                item = params.get("item", {})
                if isinstance(item, Mapping) and item.get("type") == "agentMessage":
                    for key in ("text", "content"):
                        if isinstance(item.get(key), str):
                            completed_text = str(item[key])
        return completed_text if completed_text is not None else "".join(deltas)

    def _read_stored_thread(
        self, thread_id: str, turn_id: str | None
    ) -> ExecutionObservation:
        document = self._read_stored_document(thread_id)
        if document is None:
            return ExecutionObservation(
                ExecutionState.UNKNOWN,
                terminal_confirmed=False,
                quiescent_confirmed=False,
                detail="stored Codex thread could not be read",
            )
        turn = self._stored_turn(document, turn_id)
        if turn is None:
            return ExecutionObservation(
                ExecutionState.UNKNOWN,
                terminal_confirmed=False,
                quiescent_confirmed=False,
                detail="expected Codex turn is absent or ambiguous in stored thread",
            )
        status = str(turn.get("status", ""))
        if status == "inProgress":
            return ExecutionObservation(ExecutionState.RUNNING)
        if status == "completed":
            return ExecutionObservation(
                ExecutionState.SUCCEEDED, terminal_confirmed=True, quiescent_confirmed=True
            )
        if status in {"failed", "interrupted"}:
            return ExecutionObservation(
                ExecutionState.FAILED, terminal_confirmed=True, quiescent_confirmed=True
            )
        return ExecutionObservation(
            ExecutionState.UNKNOWN,
            terminal_confirmed=False,
            quiescent_confirmed=False,
            detail=f"stored turn has unknown status: {status}",
        )

    def _read_stored_document(self, thread_id: str) -> Mapping[str, Any] | None:
        session: AppServerSession | None = None
        try:
            session = self.session_factory(self.command, self.process_cwd, self.request_timeout)
            return session.request("thread/read", {"threadId": thread_id, "includeTurns": True})
        except Exception:
            return None
        finally:
            if session is not None:
                session.close()

    def _read_stored_outcome(
        self, thread_id: str, turn_id: str | None
    ) -> ExecutionOutcome | None:
        document = self._read_stored_document(thread_id)
        if document is None:
            return None
        turn = self._stored_turn(document, turn_id)
        if turn is None:
            return None
        status = str(turn.get("status", ""))
        if status in {"failed", "interrupted"}:
            detail = json.dumps(turn.get("error", {}), ensure_ascii=False, sort_keys=True)
            return ExecutionOutcome(
                ExecutionState.FAILED,
                failure_class=self._classify_failure(detail),
                failure_code="CODEX_STORED_TURN_FAILED",
                failure_signature=self._normalized_signature(detail),
                terminal_confirmed=True,
                quiescent_confirmed=True,
            )
        if status != "completed":
            return None
        text = self._find_last_agent_text(turn) or ""
        try:
            parsed = json.loads(text)
            payload = parsed if isinstance(parsed, dict) else {"value": parsed}
        except (json.JSONDecodeError, TypeError):
            payload = {"final_response": text, "recovered_thread_id": thread_id}
        return ExecutionOutcome(
            ExecutionState.SUCCEEDED,
            payload=payload,
            summary=text,
            terminal_confirmed=True,
            quiescent_confirmed=True,
        )

    @staticmethod
    def _stored_turn(
        document: Mapping[str, Any], turn_id: str | None
    ) -> Mapping[str, Any] | None:
        thread = document.get("thread", {})
        turns = thread.get("turns", []) if isinstance(thread, Mapping) else []
        candidates = [turn for turn in turns if isinstance(turn, Mapping)]
        if turn_id is not None:
            return next(
                (turn for turn in candidates if str(turn.get("id", "")) == turn_id),
                None,
            )
        # A newly-created execution owns a dedicated Codex thread in V0.1. If
        # turn/start replied ambiguously, exactly one stored turn is still safe
        # to reconcile; multiple turns are not guessed.
        return candidates[0] if len(candidates) == 1 else None

    @classmethod
    def _find_last_agent_text(cls, value: Any) -> str | None:
        found: list[str] = []

        def walk(item: Any) -> None:
            if isinstance(item, Mapping):
                if item.get("type") == "agentMessage":
                    for key in ("text", "content"):
                        if isinstance(item.get(key), str):
                            found.append(str(item[key]))
                for child in item.values():
                    walk(child)
            elif isinstance(item, list):
                for child in item:
                    walk(child)

        walk(value)
        return found[-1] if found else None

    @staticmethod
    def _classify_failure(detail: str) -> FailureClass:
        lowered = detail.lower()
        if (
            "429" in lowered
            or "rate limit" in lowered
            or "temporar" in lowered
            or "connection reset" in lowered
            or "connection closed" in lowered
            or "stream failure" in lowered
        ):
            return FailureClass.TRANSIENT_EXTERNAL
        if "timeout" in lowered or "timed out" in lowered:
            return FailureClass.TIMEOUT
        if (
            "unavailable" in lowered
            or "overloaded" in lowered
            or any(code in lowered for code in ("502", "503", "504"))
        ):
            return FailureClass.RESOURCE_UNAVAILABLE
        if "permission" in lowered or "approval" in lowered or "denied" in lowered:
            return FailureClass.PERMISSION_FAILURE
        return FailureClass.ADAPTER_PROTOCOL_FAILURE

    @staticmethod
    def _normalized_signature(detail: str) -> str:
        compact = " ".join(detail.split())
        return compact[:240]
