from __future__ import annotations

from typing import Mapping, Protocol

from ..models import (
    ExecutionObservation,
    ExecutionOutcome,
    ExecutionRequest,
    StartObservation,
)


class ExecutionAdapter(Protocol):
    def start_execution(self, request: ExecutionRequest) -> StartObservation: ...

    def observe_execution(self, runtime_handle: Mapping[str, object]) -> ExecutionObservation: ...

    def interrupt_execution(self, runtime_handle: Mapping[str, object]) -> ExecutionObservation: ...

    def terminate_execution(self, runtime_handle: Mapping[str, object]) -> ExecutionObservation: ...

    def collect_outcome(self, runtime_handle: Mapping[str, object]) -> ExecutionOutcome: ...

    def reconcile_start(
        self, request_id: str, runtime_handle: Mapping[str, object]
    ) -> StartObservation: ...
