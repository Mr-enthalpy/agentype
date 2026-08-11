from __future__ import annotations

import json
import signal
import time
from pathlib import Path
from typing import Mapping

from .adapters.base import ExecutionAdapter
from .config import ExecutionTargetConfig
from .core import Scheduler
from .enums import AgentState, ExecutionState, FailureClass, WorkspaceMode
from .errors import StaleAuthority
from .models import ExecutionObservation, ExecutionRequest
from .root_bridge import OutboxDispatcher
from .storage import json_loads


class Dispatcher:
    def __init__(
        self,
        scheduler: Scheduler,
        *,
        adapters: Mapping[str, ExecutionAdapter],
        targets: Mapping[str, ExecutionTargetConfig],
        workspace_root: str | Path,
        outbox: OutboxDispatcher | None = None,
    ) -> None:
        self.scheduler = scheduler
        self.adapters = dict(adapters)
        self.targets = dict(targets)
        self.workspace_root = str(Path(workspace_root).resolve())
        self.outbox = outbox

    def recover(self) -> dict[str, int]:
        self.scheduler.set_lifecycle("RECOVERY")
        isolated = {name for name, target in self.targets.items() if target.attempt_isolation}
        lease_result = self.scheduler.expire_leases(attempt_isolated_targets=isolated)
        observed = self.poll_executions(recovery=True)
        self.scheduler.promote_retry_wait()
        pool_result = self.scheduler.reconcile_pool()
        revived = self.scheduler.revive_eligible_agents()
        self.scheduler.set_lifecycle("READY")
        return {
            "observed": observed,
            "retried": lease_result["retried"],
            "suspended": lease_result["suspended"],
            "born": pool_result["born"],
            "retired": pool_result["retired"],
            "revived": revived,
        }

    def tick(self) -> dict[str, int]:
        self.scheduler.promote_retry_wait()
        isolated = {name for name, target in self.targets.items() if target.attempt_isolation}
        expired = self.scheduler.expire_leases(attempt_isolated_targets=isolated)
        pool = self.scheduler.reconcile_pool()
        revived = self.scheduler.revive_eligible_agents()
        affinity_births = self.scheduler.ensure_task_consumers()
        observed = self.poll_executions()
        dispatched = self.dispatch_ready()
        delivered = self.outbox.deliver_pending() if self.outbox else 0
        return {
            "dispatched": dispatched,
            "observed": observed,
            "retried": expired["retried"],
            "suspended": expired["suspended"],
            "born": pool["born"],
            "retired": pool["retired"],
            "revived": revived,
            "affinity_births": affinity_births,
            "notifications": delivered,
        }

    def dispatch_ready(self) -> int:
        dispatched = 0
        agents = self.scheduler.list("logical_agents", state=AgentState.READY.value)
        for agent in agents:
            claim = self.scheduler.claim_next(agent["id"])
            if claim is None:
                continue
            execution_id, request_id = self.scheduler.create_execution(claim)
            adapter = self.adapters[claim.execution_target]
            incarnation = self.scheduler.get("incarnations", claim.incarnation_id)
            request = ExecutionRequest(
                request_id=request_id,
                execution_id=execution_id,
                task_id=claim.task_id,
                attempt_id=claim.attempt_id,
                lease_epoch=claim.lease_epoch,
                logical_agent_id=claim.logical_agent_id,
                incarnation_id=claim.incarnation_id,
                execution_target=claim.execution_target,
                execution_profile=claim.execution_profile,
                cwd=self.workspace_root,
                prompt=self._render_prompt(claim, json_loads(agent["continuity_json"], {})),
                workspace_mode=claim.workspace_mode,
                continuity=json_loads(agent["continuity_json"], {}),
                incarnation_runtime_handle=json_loads(incarnation["runtime_handle_json"], {}),
            )
            start = adapter.start_execution(request)
            if start.state == ExecutionState.RUNNING:
                self.scheduler.confirm_execution_running(
                    claim.attempt_id,
                    claim.lease_epoch,
                    execution_id,
                    runtime_handle=start.runtime_handle,
                )
                dispatched += 1
            elif start.ambiguous or start.state == ExecutionState.UNKNOWN:
                self.scheduler.record_start_ambiguity(
                    claim.attempt_id,
                    claim.lease_epoch,
                    execution_id,
                    runtime_handle=start.runtime_handle,
                    detail=start.detail,
                )
            else:
                self.scheduler.nack(
                    claim.attempt_id,
                    claim.lease_epoch,
                    failure_class=start.failure_class or FailureClass.START_FAILURE,
                    execution_id=execution_id,
                    failure_code=start.failure_code,
                    detail=start.detail,
                    terminal_confirmed=True,
                    quiescent_confirmed=True,
                    attempt_isolation=self.targets[claim.execution_target].attempt_isolation,
                )
        return dispatched

    def poll_executions(self, *, recovery: bool = False) -> int:
        count = 0
        executions = []
        for state in (ExecutionState.RUNNING, ExecutionState.STARTING, ExecutionState.UNKNOWN):
            executions.extend(self.scheduler.list("executions", state=state.value))
        for execution in executions:
            adapter = self.adapters.get(execution["execution_target"])
            if adapter is None:
                continue
            handle = json_loads(execution["runtime_handle_json"], {})
            if execution["state"] in (ExecutionState.STARTING.value, ExecutionState.UNKNOWN.value):
                start = adapter.reconcile_start(execution["request_id"], handle)
                if start.state == ExecutionState.RUNNING:
                    try:
                        attempt = self.scheduler.get("attempts", execution["attempt_id"])
                        self.scheduler.confirm_execution_running(
                            execution["attempt_id"],
                            int(attempt["lease_epoch"]),
                            execution["id"],
                            runtime_handle=start.runtime_handle,
                        )
                    except StaleAuthority:
                        self.scheduler.record_physical_outcome(
                            execution["id"], state=ExecutionState.RUNNING
                        )
                    count += 1
                    continue
                if start.ambiguous:
                    continue
                if start.state in {
                    ExecutionState.SUCCEEDED,
                    ExecutionState.FAILED,
                    ExecutionState.LOST,
                    ExecutionState.TERMINATED,
                }:
                    observation = ExecutionObservation(
                        start.state,
                        terminal_confirmed=True,
                        quiescent_confirmed=True,
                        detail=start.detail,
                    )
                else:
                    observation = adapter.observe_execution(handle)
            else:
                observation = adapter.observe_execution(handle)
            if observation.state == ExecutionState.RUNNING:
                try:
                    attempt = self.scheduler.get("attempts", execution["attempt_id"])
                    self.scheduler.heartbeat(
                        execution["attempt_id"], int(attempt["lease_epoch"])
                    )
                except StaleAuthority:
                    pass
                count += 1
                continue
            if observation.state in (ExecutionState.UNKNOWN, ExecutionState.STARTING):
                continue
            outcome = adapter.collect_outcome(handle)
            try:
                attempt = self.scheduler.get("attempts", execution["attempt_id"])
                epoch = int(attempt["lease_epoch"])
                if outcome.state == ExecutionState.SUCCEEDED:
                    self.scheduler.ack_success(
                        execution["attempt_id"],
                        epoch,
                        execution_id=execution["id"],
                        payload=outcome.payload or {},
                        summary=outcome.summary,
                        continuity_capsule=outcome.checkpoint,
                    )
                else:
                    target = self.targets[execution["execution_target"]]
                    self.scheduler.nack(
                        execution["attempt_id"],
                        epoch,
                        failure_class=outcome.failure_class or FailureClass.UNKNOWN,
                        execution_id=execution["id"],
                        failure_code=outcome.failure_code,
                        failure_signature=outcome.failure_signature,
                        terminal_confirmed=outcome.terminal_confirmed,
                        quiescent_confirmed=outcome.quiescent_confirmed,
                        attempt_isolation=target.attempt_isolation,
                    )
            except StaleAuthority:
                self.scheduler.record_physical_outcome(
                    execution["id"],
                    state=outcome.state,
                    payload=outcome.payload,
                    failure_class=outcome.failure_class,
                    failure_code=outcome.failure_code,
                    failure_signature=outcome.failure_signature,
                    terminal_confirmed=outcome.terminal_confirmed,
                    quiescent_confirmed=outcome.quiescent_confirmed,
                )
            count += 1
        return count

    def interrupt_execution(self, execution_id: str, *, terminate: bool = False) -> dict[str, object]:
        execution = self.scheduler.get("executions", execution_id)
        adapter = self.adapters[execution["execution_target"]]
        handle = json_loads(execution["runtime_handle_json"], {})
        observation = (
            adapter.terminate_execution(handle)
            if terminate
            else adapter.interrupt_execution(handle)
        )
        self.scheduler.record_physical_outcome(
            execution_id,
            state=(
                ExecutionState.LOST
                if observation.state == ExecutionState.UNKNOWN
                else observation.state
            ),
            failure_class=FailureClass.EXECUTION_LOST,
            failure_code="ROOT_TERMINATION",
            terminal_confirmed=observation.terminal_confirmed,
            quiescent_confirmed=observation.quiescent_confirmed,
        )
        self.scheduler.cancel_task(
            execution["task_id"],
            quiescence_confirmed=observation.quiescent_confirmed,
            attempt_isolation=self.targets[execution["execution_target"]].attempt_isolation,
        )
        return {
            "execution_id": execution_id,
            "state": observation.state.value,
            "terminal_confirmed": observation.terminal_confirmed,
            "quiescent_confirmed": observation.quiescent_confirmed,
            "detail": observation.detail,
        }

    @staticmethod
    def _render_prompt(claim, continuity: Mapping[str, object]) -> str:
        sections = [
            "LOCAL AGENT SCHEDULER TASK",
            f"TASK_ID\n{claim.task_id}",
            f"ATTEMPT_ID\n{claim.attempt_id}",
            f"LEASE_EPOCH\n{claim.lease_epoch}",
            f"WORKSTREAM\n{claim.workstream_id or 'none'}",
            "OBJECTIVE\n" + json.dumps(claim.payload, ensure_ascii=False, sort_keys=True),
            "ACCEPTANCE\n" + json.dumps(claim.acceptance, ensure_ascii=False, sort_keys=True),
            "COMMITTED CONTINUITY\n" + json.dumps(continuity, ensure_ascii=False, sort_keys=True),
        ]
        if claim.workspace_mode is WorkspaceMode.WRITE:
            sections.append(
                "WRITER RECOVERY RULES\n"
                "The current workspace is authoritative. Inspect assignment-scoped state and diff "
                "before writing; continue idempotently; do not revert unrelated work."
            )
        sections.append(
            "RETURN\nReturn the authoritative result only when acceptance is satisfied. "
            "Do not claim Scheduler ACK; the Scheduler validates the current lease separately."
        )
        return "\n\n".join(sections)


class SchedulerDaemon:
    def __init__(self, dispatcher: Dispatcher, *, poll_seconds: float = 1.0):
        self.dispatcher = dispatcher
        self.poll_seconds = poll_seconds
        self._stopping = False

    def stop(self, *_args) -> None:
        self._stopping = True

    def run(self) -> None:
        signal.signal(signal.SIGINT, self.stop)
        if hasattr(signal, "SIGTERM"):
            signal.signal(signal.SIGTERM, self.stop)
        self.dispatcher.recover()
        while not self._stopping:
            self.dispatcher.tick()
            time.sleep(self.poll_seconds)
