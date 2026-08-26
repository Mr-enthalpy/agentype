from __future__ import annotations

import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from local_agent_scheduler.adapters.codex import CodexAppServerAdapter
from local_agent_scheduler.adapters.grok import GrokAcpAdapter
from local_agent_scheduler.cli import _build_dispatcher
from local_agent_scheduler.config import ExecutionTargetConfig, load_config
from local_agent_scheduler.root_bridge import GrokAcpRootBridge
from local_agent_scheduler.core import Scheduler
from local_agent_scheduler.enums import ExecutionState, FailureClass, Retention, WorkspaceMode
from local_agent_scheduler.errors import ConfigurationError
from local_agent_scheduler.models import ExecutionRequest, PartitionSpec, RetryPolicy, TaskSpec
from local_agent_scheduler.runtime import Dispatcher
from local_agent_scheduler.storage import Database


class CapturingGrokSession:
    instances: list["CapturingGrokSession"] = []

    def __init__(self, command, _cwd, timeout):
        self.command = command
        self.timeout = timeout
        self.session_id = f"acp-{len(self.instances)}"
        self.calls: list[tuple[str, dict, float | None]] = []
        self.process = type("Process", (), {"poll": lambda self: None})()
        self.closed = False
        self.close_timeout: float | None = None
        self.instances.append(self)

    def request(self, method, params, *, timeout=None, **_kwargs):
        self.calls.append((method, dict(params), timeout))
        if method == "session/new":
            return {"sessionId": "grok-session"}
        if method == "session/prompt":
            return {"stopReason": "end_turn"}
        return {}

    def notifications(self):
        return [
            {
                "method": "session/update",
                "params": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"text": '{"ok": true}'},
                },
            }
        ]

    def close(self, *, timeout=None, **_kwargs):
        self.closed = True
        self.close_timeout = timeout
        return True


class ExitedGrokSession(CapturingGrokSession):
    def __init__(self, command, cwd, timeout):
        super().__init__(command, cwd, timeout)
        self.process = type("Process", (), {"poll": lambda self: 1})()


class RequestFailingGrokSession(CapturingGrokSession):
    clock = [0.0]

    def __init__(self, command, cwd, timeout):
        super().__init__(command, cwd, timeout)
        self.__class__.clock[0] += 0.4

    def request(self, method, params, *, timeout=None, **_kwargs):
        self.__class__.clock[0] += 0.5
        raise RuntimeError("session/new failed")


class GrokAdapterCase(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        CapturingGrokSession.instances = []
        ExitedGrokSession.instances = []
        RequestFailingGrokSession.instances = []
        RequestFailingGrokSession.clock = [0.0]

    def tearDown(self):
        self.temp.cleanup()

    def _request(self, workspace_mode=WorkspaceMode.READ_ONLY):
        return ExecutionRequest(
            "request",
            "execution",
            "task",
            "attempt",
            1,
            "agent",
            "inc",
            "local_grok",
            "default",
            self.temp.name,
            "inspect",
            workspace_mode,
            {},
            {},
        )

    def test_start_returns_running_with_session_identity(self):
        adapter = GrokAcpAdapter(session_factory=CapturingGrokSession)
        started = adapter.start_execution(self._request())
        self.assertEqual(started.state, ExecutionState.RUNNING)
        self.assertEqual(started.runtime_handle["session_id"], "grok-session")
        self.assertIn("prompt_id", started.runtime_handle)
        session = CapturingGrokSession.instances[0]
        self.assertEqual(session.calls[0][0], "session/new")
        for _ in range(50):
            if any(name == "session/prompt" for name, _params, _timeout in session.calls):
                break
            time.sleep(0.01)
        else:
            self.fail("background session/prompt was not started")

    def test_read_only_task_uses_read_only_sandbox(self):
        adapter = GrokAcpAdapter(
            sandbox="workspace", session_factory=CapturingGrokSession
        )
        adapter.start_execution(self._request(WorkspaceMode.READ_ONLY))
        command = CapturingGrokSession.instances[0].command
        agent_at = command.index("agent")
        self.assertEqual(
            command[agent_at - 2 : agent_at + 1], ("--sandbox", "read-only", "agent")
        )
        self.assertEqual(command[-1], "stdio")
        meta = CapturingGrokSession.instances[0].calls[0][1]["_meta"]
        self.assertEqual(meta["sandbox"], "read-only")

    def test_profile_model_is_inserted_before_stdio(self):
        adapter = GrokAcpAdapter(
            session_factory=CapturingGrokSession,
            profile_options={"default": {"model": "grok-build"}},
        )
        adapter.start_execution(self._request())
        command = CapturingGrokSession.instances[0].command
        agent_at = command.index("agent")
        stdio_at = command.index("stdio")
        self.assertEqual(
            command[agent_at - 2 : agent_at + 1], ("--sandbox", "read-only", "agent")
        )
        self.assertEqual(
            command[stdio_at - 2 : stdio_at + 1], ("--model", "grok-build", "stdio")
        )

    def test_collect_agent_text_accepts_nested_update_shape(self):
        nested = [
            {
                "method": "session/update",
                "params": {
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "content": {"text": '{"evidence":"v0.1.2-grok-live"}'},
                    }
                },
            }
        ]
        flat = [
            {
                "method": "session/update",
                "params": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"text": '{"status":"ok"}'},
                },
            }
        ]
        self.assertEqual(
            GrokAcpAdapter._collect_agent_text(nested),
            '{"evidence":"v0.1.2-grok-live"}',
        )
        self.assertEqual(GrokAcpAdapter._collect_agent_text(flat), '{"status":"ok"}')

    def test_start_deadline_covers_session_and_session_new(self):
        class AdvancingGrokSession(CapturingGrokSession):
            clock = [0.0]
            constructor_timeouts: list[float] = []

            def __init__(self, command, cwd, timeout):
                self.__class__.constructor_timeouts.append(timeout)
                self.__class__.clock[0] += 0.2
                super().__init__(command, cwd, timeout)

            def request(self, method, params, *, timeout=None, **_kwargs):
                self.__class__.clock[0] += 0.2
                return super().request(method, params, timeout=timeout)

        AdvancingGrokSession.clock = [0.0]
        AdvancingGrokSession.constructor_timeouts = []
        AdvancingGrokSession.instances = []
        adapter = GrokAcpAdapter(
            request_timeout=1.0, session_factory=AdvancingGrokSession
        )
        with patch(
            "local_agent_scheduler.adapters.grok.time.monotonic",
            side_effect=lambda: AdvancingGrokSession.clock[0],
        ):
            started = adapter.start_execution(self._request())
        self.assertEqual(started.state, ExecutionState.RUNNING)
        self.assertAlmostEqual(AdvancingGrokSession.constructor_timeouts[0], 1.0)
        session_new_timeout = AdvancingGrokSession.instances[0].calls[0][2]
        self.assertAlmostEqual(session_new_timeout, 0.8)

    def test_start_exception_cleanup_uses_remaining_deadline(self):
        adapter = GrokAcpAdapter(
            request_timeout=1.0, session_factory=RequestFailingGrokSession
        )
        with patch(
            "local_agent_scheduler.adapters.grok.time.monotonic",
            side_effect=lambda: RequestFailingGrokSession.clock[0],
        ):
            started = adapter.start_execution(self._request())
        self.assertEqual(started.state, ExecutionState.FAILED)
        session = RequestFailingGrokSession.instances[0]
        self.assertTrue(session.closed)
        self.assertIsNotNone(session.close_timeout)
        self.assertLessEqual(session.close_timeout, 0.1)

    def test_exited_process_with_in_progress_session_is_not_quiescent(self):
        lookup = {"called": None}

        def persisted(session_id):
            lookup["called"] = session_id
            return {"status": "inProgress"}

        adapter = GrokAcpAdapter(
            session_factory=ExitedGrokSession, persisted_lookup=persisted
        )
        session = ExitedGrokSession(("grok",), None, 1)
        adapter._sessions[session.session_id] = session
        handle = {
            "adapter_session_id": session.session_id,
            "session_id": "grok-session",
            "prompt_id": "prompt",
        }
        observation = adapter.observe_execution(handle)
        self.assertEqual(observation.state, ExecutionState.RUNNING)
        self.assertFalse(observation.quiescent_confirmed)
        self.assertTrue(session.closed)
        self.assertEqual(lookup["called"], "grok-session")
        outcome = adapter.collect_outcome(handle)
        self.assertEqual(outcome.state, ExecutionState.RUNNING)
        self.assertFalse(outcome.quiescent_confirmed)

    def test_in_progress_after_exit_does_not_replace_writer(self):
        db = Database(Path(self.temp.name) / "scheduler.db")
        scheduler = Scheduler(db, lease_seconds=2)
        scheduler.initialize()
        scheduler.upsert_partition(
            PartitionSpec("general", 1, Retention.RESIDENT, "local_grok", "default")
        )
        scheduler.reconcile_pool()
        policy = RetryPolicy(
            max_attempts=2,
            retry_classes=(FailureClass.EXECUTION_LOST,),
            base_backoff_seconds=0,
            max_backoff_seconds=0,
        )
        _batch, ids = scheduler.submit_batch(
            [
                TaskSpec(
                    "grok-writer",
                    {},
                    workspace_mode=WorkspaceMode.WRITE,
                    retry_policy=policy,
                    partition="general",
                )
            ]
        )
        claim = scheduler.claim_next(scheduler.list("logical_agents", state="READY")[0]["id"])
        execution_id, request_id = scheduler.create_execution(claim)
        adapter = GrokAcpAdapter(
            session_factory=ExitedGrokSession,
            persisted_lookup=lambda _sid: {"status": "inProgress"},
        )
        session = ExitedGrokSession(("grok",), None, 1)
        adapter._sessions[session.session_id] = session
        scheduler.record_start_ambiguity(
            claim.attempt_id,
            claim.lease_epoch,
            execution_id,
            runtime_handle={
                "request_id": request_id,
                "adapter_session_id": session.session_id,
                "session_id": "grok-session",
                "prompt_id": "prompt",
            },
        )
        dispatcher = Dispatcher(
            scheduler,
            adapters={"local_grok": adapter},
            targets={
                "local_grok": ExecutionTargetConfig(
                    "local_grok", "grok_acp", False, True
                )
            },
            execution_profiles={"default"},
            workspace_root=self.temp.name,
        )
        self.assertEqual(dispatcher.poll_executions(recovery=True), 1)
        execution = scheduler.get("executions", execution_id)
        task = scheduler.get("tasks", ids["grok-writer"])
        self.assertEqual(execution["state"], "RUNNING")
        self.assertEqual(execution["quiescent_confirmed"], 0)
        self.assertNotEqual(task["state"], "RETRY_WAIT")
        self.assertEqual(len(scheduler.list("attempts")), 1)

    def test_grok_example_config_loads(self):
        source = Path(__file__).resolve().parents[1] / "config" / "scheduler.grok.toml"
        config = load_config(source)
        self.assertEqual(config.execution_targets[0].adapter, "grok_acp")
        self.assertIsNotNone(config.grok_adapter)
        self.assertEqual(config.grok_adapter.sandbox, "workspace")
        self.assertEqual(config.partitions[0].execution_target, "local_grok")

    def test_unknown_adapter_is_rejected(self):
        path = Path(self.temp.name) / "bad.toml"
        path.write_text(
            """
schema_version = 1
[execution_profiles.default]
[[partitions]]
name = "general"
desired_capacity = 1
retention = "resident"
execution_target = "x"
execution_profile = "default"
[[execution_targets]]
name = "x"
adapter = "opencode"
attempt_isolation = false
termination_confirmation = true
""",
            encoding="utf-8",
        )
        with self.assertRaisesRegex(ConfigurationError, "unsupported adapter"):
            load_config(path)

    def test_cli_factory_keeps_codex_and_grok_instances_distinct(self):
        path = Path(self.temp.name) / "mixed.toml"
        path.write_text(
            """
schema_version = 1
[execution_profiles.default]
[[partitions]]
name = "general"
desired_capacity = 1
retention = "resident"
execution_target = "local_codex"
execution_profile = "default"
[[execution_targets]]
name = "local_codex"
adapter = "codex_app_server"
attempt_isolation = false
termination_confirmation = true
[[execution_targets]]
name = "local_grok"
adapter = "grok_acp"
attempt_isolation = false
termination_confirmation = true
[adapters.codex_app_server]
command = ["codex", "app-server"]
[adapters.grok_acp]
command = ["grok", "agent", "--always-approve", "stdio"]
[root_bridge]
kind = "filesystem"
inbox = ".events"
""",
            encoding="utf-8",
        )
        config = load_config(path)
        scheduler = Scheduler(Database(Path(self.temp.name) / "db.sqlite"))
        scheduler.initialize()
        dispatcher, _daemon = _build_dispatcher(scheduler, config)
        self.assertIsInstance(dispatcher.adapters["local_codex"], CodexAppServerAdapter)
        self.assertIsInstance(dispatcher.adapters["local_grok"], GrokAcpAdapter)
        self.assertIsNot(dispatcher.adapters["local_codex"], dispatcher.adapters["local_grok"])

    def test_grok_root_bridge_loads_existing_session_without_result_transport(self):
        class RootSession(CapturingGrokSession):
            def request(self, method, params, *, timeout=None, **_kwargs):
                self.calls.append((method, dict(params), timeout))
                if method == "session/load":
                    return {"sessionId": params["sessionId"]}
                if method == "session/prompt":
                    return {"stopReason": "end_turn"}
                if method == "session/new":
                    raise AssertionError("RootBridge must not create Root identity")
                return {}

        RootSession.instances = []
        bridge = GrokAcpRootBridge(
            root_session_id="root-session",
            session_factory=RootSession,
        )
        outcome = bridge.deliver(
            "event-1",
            "BATCH_RESULTS_READY",
            {"batch_id": "batch-1", "result_body": "must-not-be-transported"},
        )
        self.assertTrue(outcome.delivered)
        session = RootSession.instances[-1]
        methods = [name for name, _params, _timeout in session.calls]
        self.assertEqual(methods[0], "session/load")
        self.assertEqual(session.calls[0][1]["sessionId"], "root-session")
        self.assertIn("session/prompt", methods)
        self.assertNotIn("session/new", methods)
        notice = session.calls[1][1]["prompt"][0]["text"]
        self.assertIn("event-1", notice)
        self.assertIn("batch-1", notice)
        self.assertNotIn("must-not-be-transported", notice)
        self.assertTrue(session.closed)
        self.assertIn("--sandbox", session.command)
        self.assertEqual(session.command[session.command.index("agent") - 1], "read-only")

    def test_grok_root_bridge_failed_prompt_is_not_delivered(self):
        class FailingRootSession(CapturingGrokSession):
            def request(self, method, params, *, timeout=None, **_kwargs):
                self.calls.append((method, dict(params), timeout))
                if method == "session/load":
                    return {"sessionId": params["sessionId"]}
                raise TimeoutError("Grok ACP request timed out: session/prompt")

        FailingRootSession.instances = []
        bridge = GrokAcpRootBridge(
            root_session_id="root-session",
            session_factory=FailingRootSession,
            completion_timeout=0.05,
        )
        outcome = bridge.deliver("event-1", "BATCH_RESULTS_READY", {"batch_id": "batch-1"})
        self.assertFalse(outcome.delivered)
        self.assertTrue(FailingRootSession.instances[-1].closed)

    def test_grok_root_bridge_config_requires_session_id(self):
        path = Path(self.temp.name) / "grok-root-missing.toml"
        path.write_text(
            """
schema_version = 1
[execution_profiles.default]
[[partitions]]
name = "general"
desired_capacity = 1
retention = "resident"
execution_target = "local_grok"
execution_profile = "default"
[[execution_targets]]
name = "local_grok"
adapter = "grok_acp"
attempt_isolation = false
termination_confirmation = true
[adapters.grok_acp]
command = ["grok", "agent", "--always-approve", "stdio"]
[root_bridge]
kind = "grok_acp"
inbox = ".events"
""",
            encoding="utf-8",
        )
        with self.assertRaisesRegex(ConfigurationError, "root_session_id"):
            load_config(path)


if __name__ == "__main__":
    unittest.main()
