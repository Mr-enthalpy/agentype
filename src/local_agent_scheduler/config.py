from __future__ import annotations

import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping

from .enums import FailureClass, Retention
from .errors import ConfigurationError
from .models import PartitionSpec, RetryPolicy


@dataclass(frozen=True)
class ExecutionTargetConfig:
    name: str
    adapter: str
    attempt_isolation: bool
    termination_confirmation: bool


@dataclass(frozen=True)
class CodexAdapterConfig:
    command: tuple[str, ...]
    cwd: str
    approval_policy: str
    sandbox: str


@dataclass(frozen=True)
class GrokAdapterConfig:
    command: tuple[str, ...]
    cwd: str
    sandbox: str
    request_timeout: float


@dataclass(frozen=True)
class RootBridgeConfig:
    kind: str
    inbox: str
    root_thread_id: str | None
    root_session_id: str | None
    request_timeout: float
    completion_timeout: float


@dataclass(frozen=True)
class SchedulerConfig:
    schema_version: int
    database_path: str
    lease_seconds: float
    heartbeat_seconds: float
    continuity_max_bytes: int
    dispatcher_poll_seconds: float
    partitions: tuple[PartitionSpec, ...]
    execution_targets: tuple[ExecutionTargetConfig, ...]
    codex_adapter: CodexAdapterConfig
    grok_adapter: GrokAdapterConfig | None
    execution_profiles: Mapping[str, Mapping[str, Any]]
    retry_defaults: RetryPolicy
    root_bridge: RootBridgeConfig
    base_dir: Path

    def target(self, name: str) -> ExecutionTargetConfig:
        for target in self.execution_targets:
            if target.name == name:
                return target
        raise ConfigurationError(f"unknown execution target {name!r}")

    def resolve(self, value: str) -> str:
        path = Path(value)
        return str(path if path.is_absolute() else self.base_dir / path)


def _reject_unknown(mapping: Mapping[str, Any], allowed: set[str], context: str) -> None:
    unknown = set(mapping) - allowed
    if unknown:
        raise ConfigurationError(f"unknown keys in {context}: {sorted(unknown)}")


def _typed(value: Any, expected: type, context: str) -> Any:
    if expected is int and isinstance(value, bool):
        raise ConfigurationError(f"{context} must be an integer")
    if not isinstance(value, expected):
        raise ConfigurationError(f"{context} must be {expected.__name__}")
    return value


def _nonempty(value: Any, context: str) -> str:
    text = _typed(value, str, context)
    if not text.strip():
        raise ConfigurationError(f"{context} cannot be empty")
    return text


def load_config(path: str | Path) -> SchedulerConfig:
    config_path = Path(path).resolve()
    with config_path.open("rb") as handle:
        raw = tomllib.load(handle)
    _reject_unknown(
        raw,
        {
            "schema_version",
            "database_path",
            "lease_seconds",
            "heartbeat_seconds",
            "continuity_max_bytes",
            "dispatcher_poll_seconds",
            "partitions",
            "execution_targets",
            "execution_profiles",
            "retry_defaults",
            "adapters",
            "root_bridge",
        },
        "root",
    )
    if type(raw.get("schema_version")) is not int or raw.get("schema_version") != 1:
        raise ConfigurationError("schema_version must be 1")

    partitions: list[PartitionSpec] = []
    for index, item in enumerate(raw.get("partitions", [])):
        _reject_unknown(
            item,
            {
                "name",
                "desired_capacity",
                "retention",
                "execution_target",
                "execution_profile",
                "tags",
            },
            f"partitions[{index}]",
        )
        try:
            desired_capacity = _typed(
                item["desired_capacity"], int, f"partitions[{index}].desired_capacity"
            )
            if desired_capacity < 0:
                raise ConfigurationError(
                    f"partitions[{index}].desired_capacity must be non-negative"
                )
            partitions.append(
                PartitionSpec(
                    name=_nonempty(item["name"], f"partitions[{index}].name"),
                    desired_capacity=desired_capacity,
                    retention=Retention(item["retention"]),
                    execution_target=_nonempty(
                        item["execution_target"], f"partitions[{index}].execution_target"
                    ),
                    execution_profile=_nonempty(
                        item["execution_profile"], f"partitions[{index}].execution_profile"
                    ),
                    tags=tuple(
                        _nonempty(tag, f"partitions[{index}].tags")
                        for tag in _typed(item.get("tags", []), list, f"partitions[{index}].tags")
                    ),
                )
            )
        except (KeyError, TypeError, ValueError) as exc:
            raise ConfigurationError(f"invalid partitions[{index}]: {exc}") from exc

    targets: list[ExecutionTargetConfig] = []
    for index, item in enumerate(raw.get("execution_targets", [])):
        _reject_unknown(
            item,
            {"name", "adapter", "attempt_isolation", "termination_confirmation"},
            f"execution_targets[{index}]",
        )
        try:
            targets.append(
                ExecutionTargetConfig(
                    name=_nonempty(item["name"], f"execution_targets[{index}].name"),
                    adapter=_nonempty(item["adapter"], f"execution_targets[{index}].adapter"),
                    attempt_isolation=_typed(
                        item["attempt_isolation"], bool, f"execution_targets[{index}].attempt_isolation"
                    ),
                    termination_confirmation=_typed(
                        item["termination_confirmation"],
                        bool,
                        f"execution_targets[{index}].termination_confirmation",
                    ),
                )
            )
        except KeyError as exc:
            raise ConfigurationError(f"missing execution target key: {exc}") from exc

    adapters = raw.get("adapters", {})
    _reject_unknown(adapters, {"codex_app_server", "grok_acp"}, "adapters")
    codex = adapters.get("codex_app_server", {})
    _reject_unknown(codex, {"command", "cwd", "approval_policy", "sandbox"}, "adapters.codex_app_server")
    command_value = _typed(codex.get("command", ["codex", "app-server"]), list, "adapters.codex_app_server.command")
    command = tuple(_nonempty(part, "adapters.codex_app_server.command") for part in command_value)
    if not command:
        raise ConfigurationError("Codex adapter command cannot be empty")
    sandbox = _nonempty(
        codex.get("sandbox", "workspace-write"), "adapters.codex_app_server.sandbox"
    )
    if sandbox not in {"read-only", "workspace-write", "danger-full-access"}:
        raise ConfigurationError(
            "adapters.codex_app_server.sandbox must be read-only, workspace-write, "
            "or danger-full-access"
        )
    codex_config = CodexAdapterConfig(
        command=command,
        cwd=_nonempty(codex.get("cwd", "."), "adapters.codex_app_server.cwd"),
        approval_policy=_nonempty(
            codex.get("approval_policy", "never"), "adapters.codex_app_server.approval_policy"
        ),
        sandbox=sandbox,
    )
    grok_raw = adapters.get("grok_acp")
    grok_config: GrokAdapterConfig | None = None
    if grok_raw is not None:
        _typed(grok_raw, dict, "adapters.grok_acp")
        _reject_unknown(
            grok_raw, {"command", "cwd", "sandbox", "request_timeout"}, "adapters.grok_acp"
        )
        grok_command_value = _typed(
            grok_raw.get("command", ["grok", "agent", "--always-approve", "stdio"]),
            list,
            "adapters.grok_acp.command",
        )
        grok_command = tuple(
            _nonempty(part, "adapters.grok_acp.command") for part in grok_command_value
        )
        if not grok_command:
            raise ConfigurationError("Grok adapter command cannot be empty")
        grok_sandbox = _nonempty(
            grok_raw.get("sandbox", "workspace"), "adapters.grok_acp.sandbox"
        )
        if grok_sandbox not in {"off", "workspace", "read-only", "strict"}:
            raise ConfigurationError(
                "adapters.grok_acp.sandbox must be off, workspace, read-only, or strict"
            )
        grok_timeout = grok_raw.get("request_timeout", 30)
        if (
            not isinstance(grok_timeout, (int, float))
            or isinstance(grok_timeout, bool)
            or grok_timeout <= 0
        ):
            raise ConfigurationError("adapters.grok_acp.request_timeout must be a positive number")
        grok_config = GrokAdapterConfig(
            command=grok_command,
            cwd=_nonempty(grok_raw.get("cwd", "."), "adapters.grok_acp.cwd"),
            sandbox=grok_sandbox,
            request_timeout=float(grok_timeout),
        )

    raw_profiles = raw.get("execution_profiles", {})
    _typed(raw_profiles, dict, "execution_profiles")
    profiles: dict[str, Mapping[str, Any]] = {}
    for name, profile in raw_profiles.items():
        _nonempty(name, "execution_profiles name")
        _typed(profile, dict, f"execution_profiles.{name}")
        _reject_unknown(profile, {"model", "effort", "personality"}, f"execution_profiles.{name}")
        normalized: dict[str, Any] = {}
        for key, value in profile.items():
            normalized[key] = _nonempty(value, f"execution_profiles.{name}.{key}")
        profiles[name] = normalized

    retry_raw = raw.get("retry_defaults", {})
    _typed(retry_raw, dict, "retry_defaults")
    _reject_unknown(
        retry_raw,
        {"max_attempts", "retry_classes", "base_backoff_seconds", "max_backoff_seconds"},
        "retry_defaults",
    )
    max_attempts = _typed(retry_raw.get("max_attempts", 1), int, "retry_defaults.max_attempts")
    retry_classes_raw = _typed(
        retry_raw.get(
            "retry_classes",
            ["TRANSIENT_EXTERNAL", "TIMEOUT", "EXECUTION_LOST", "RESOURCE_UNAVAILABLE"],
        ),
        list,
        "retry_defaults.retry_classes",
    )
    base_backoff = retry_raw.get("base_backoff_seconds", 1.0)
    max_backoff = retry_raw.get("max_backoff_seconds", 60.0)
    if not isinstance(base_backoff, (int, float)) or isinstance(base_backoff, bool):
        raise ConfigurationError("retry_defaults.base_backoff_seconds must be numeric")
    if not isinstance(max_backoff, (int, float)) or isinstance(max_backoff, bool):
        raise ConfigurationError("retry_defaults.max_backoff_seconds must be numeric")
    if max_attempts < 1 or base_backoff < 0 or max_backoff < base_backoff:
        raise ConfigurationError("invalid retry_defaults range")
    try:
        retry_defaults = RetryPolicy(
            max_attempts=max_attempts,
            retry_classes=tuple(
                FailureClass(_nonempty(item, "retry_defaults.retry_classes"))
                for item in retry_classes_raw
            ),
            base_backoff_seconds=float(base_backoff),
            max_backoff_seconds=float(max_backoff),
        )
    except ValueError as exc:
        raise ConfigurationError(f"invalid retry failure class: {exc}") from exc

    root_bridge = raw.get("root_bridge", {})
    _reject_unknown(
        root_bridge,
        {
            "kind",
            "inbox",
            "root_thread_id",
            "root_session_id",
            "request_timeout",
            "completion_timeout",
        },
        "root_bridge",
    )
    bridge_kind = _nonempty(root_bridge.get("kind", "filesystem"), "root_bridge.kind")
    request_timeout = root_bridge.get("request_timeout", 30.0)
    completion_timeout = root_bridge.get("completion_timeout", 120.0)
    for value, name in (
        (request_timeout, "root_bridge.request_timeout"),
        (completion_timeout, "root_bridge.completion_timeout"),
    ):
        if not isinstance(value, (int, float)) or isinstance(value, bool) or value <= 0:
            raise ConfigurationError(f"{name} must be a positive number")
    raw_thread_id = root_bridge.get("root_thread_id")
    raw_session_id = root_bridge.get("root_session_id")
    bridge_config = RootBridgeConfig(
        kind=bridge_kind,
        inbox=_nonempty(root_bridge.get("inbox", ".scheduler-root-events"), "root_bridge.inbox"),
        root_thread_id=(
            _nonempty(raw_thread_id, "root_bridge.root_thread_id")
            if raw_thread_id is not None
            else None
        ),
        root_session_id=(
            _nonempty(raw_session_id, "root_bridge.root_session_id")
            if raw_session_id is not None
            else None
        ),
        request_timeout=float(request_timeout),
        completion_timeout=float(completion_timeout),
    )
    if bridge_config.kind not in {"filesystem", "codex_app_server", "grok_acp"}:
        raise ConfigurationError(
            "root_bridge.kind must be filesystem, codex_app_server, or grok_acp"
        )
    if bridge_config.kind == "codex_app_server" and bridge_config.root_thread_id is None:
        raise ConfigurationError("Codex RootBridge requires root_thread_id")
    if bridge_config.kind == "grok_acp" and bridge_config.root_session_id is None:
        raise ConfigurationError("Grok RootBridge requires root_session_id")

    target_names = {target.name for target in targets}
    if len(target_names) != len(targets):
        raise ConfigurationError("execution target names must be unique")
    if len({partition.name for partition in partitions}) != len(partitions):
        raise ConfigurationError("partition names must be unique")
    for partition in partitions:
        if partition.execution_target not in target_names:
            raise ConfigurationError(
                f"partition {partition.name!r} references unknown target {partition.execution_target!r}"
            )
        if partition.execution_profile not in profiles:
            raise ConfigurationError(
                f"partition {partition.name!r} references unknown profile {partition.execution_profile!r}"
            )
    for target in targets:
        if target.adapter not in {"codex_app_server", "grok_acp"}:
            raise ConfigurationError(f"unsupported adapter {target.adapter!r}")
        if target.adapter == "grok_acp" and grok_config is None:
            grok_config = GrokAdapterConfig(
                command=("grok", "agent", "--always-approve", "stdio"),
                cwd=".",
                sandbox="workspace",
                request_timeout=30.0,
            )

    lease_seconds = raw.get("lease_seconds", 120)
    heartbeat_seconds = raw.get("heartbeat_seconds", 30)
    dispatcher_poll_seconds = raw.get("dispatcher_poll_seconds", 1.0)
    for value, name in (
        (lease_seconds, "lease_seconds"),
        (heartbeat_seconds, "heartbeat_seconds"),
        (dispatcher_poll_seconds, "dispatcher_poll_seconds"),
    ):
        if not isinstance(value, (int, float)) or isinstance(value, bool) or value <= 0:
            raise ConfigurationError(f"{name} must be a positive number")
    if float(heartbeat_seconds) >= float(lease_seconds):
        raise ConfigurationError("heartbeat_seconds must be less than lease_seconds")
    if float(dispatcher_poll_seconds) > float(heartbeat_seconds):
        raise ConfigurationError(
            "dispatcher_poll_seconds must not exceed heartbeat_seconds"
        )
    continuity_max_bytes = _typed(
        raw.get("continuity_max_bytes", 16_384), int, "continuity_max_bytes"
    )
    if continuity_max_bytes <= 0:
        raise ConfigurationError("continuity_max_bytes must be positive")

    return SchedulerConfig(
        schema_version=1,
        database_path=_nonempty(raw.get("database_path", ".local-agent-scheduler.db"), "database_path"),
        lease_seconds=float(lease_seconds),
        heartbeat_seconds=float(heartbeat_seconds),
        continuity_max_bytes=continuity_max_bytes,
        dispatcher_poll_seconds=float(dispatcher_poll_seconds),
        partitions=tuple(partitions),
        execution_targets=tuple(targets),
        codex_adapter=codex_config,
        grok_adapter=grok_config,
        execution_profiles=profiles,
        retry_defaults=retry_defaults,
        root_bridge=bridge_config,
        base_dir=config_path.parent,
    )
