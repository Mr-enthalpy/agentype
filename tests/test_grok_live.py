from __future__ import annotations

import os
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from local_agent_scheduler.adapters.grok import AcpSession, GrokAcpAdapter
from local_agent_scheduler.config import ExecutionTargetConfig
from local_agent_scheduler.core import Scheduler
from local_agent_scheduler.enums import Retention, WorkspaceMode
from local_agent_scheduler.models import PartitionSpec, RetryPolicy, TaskSpec
from local_agent_scheduler.root_bridge import (
    FilesystemRootBridge,
    GrokAcpRootBridge,
    OutboxDispatcher,
)
from local_agent_scheduler.runtime import Dispatcher, SchedulerDaemon
from local_agent_scheduler.storage import Database, json_loads


def _grok_executable() -> str | None:
    override = os.environ.get("AGENTYPE_GROK_BIN")
    if override:
        path = Path(override)
        return str(path) if path.is_file() else None
    found = shutil.which("grok")
    if found:
        return found
    fallback = Path.home() / ".grok" / "bin" / "grok.exe"
    return str(fallback) if fallback.is_file() else None


@unittest.skipUnless(
    os.environ.get("AGENTYPE_GROK_LIVE") == "1",
    "set AGENTYPE_GROK_LIVE=1 to run the real Grok worker round",
)
class GrokLiveRoundCase(unittest.TestCase):
    def setUp(self):
        executable = _grok_executable()
        if executable is None:
            self.skipTest("grok executable not found")
        self.temp = tempfile.TemporaryDirectory(prefix="grok-live-round-")
        root = Path(self.temp.name)
        self.scheduler = Scheduler(Database(root / "scheduler.db"), lease_seconds=120)
        self.scheduler.initialize()
        self.scheduler.upsert_partition(
            PartitionSpec("general", 1, Retention.EPHEMERAL, "local_grok", "default")
        )
        self.scheduler.reconcile_pool()
        inbox = root / "events"
        inbox.mkdir()
        adapter = GrokAcpAdapter(
            command=(executable, "agent", "--always-approve", "stdio"),
            process_cwd=str(root),
            sandbox="workspace",
            request_timeout=90.0,
            profile_options={"default": {"model": "grok-build"}},
        )
        dispatcher = Dispatcher(
            self.scheduler,
            adapters={"local_grok": adapter},
            targets={
                "local_grok": ExecutionTargetConfig(
                    "local_grok", "grok_acp", False, True
                )
            },
            execution_profiles={"default"},
            workspace_root=str(root),
            outbox=OutboxDispatcher(self.scheduler.db, FilesystemRootBridge(inbox)),
        )
        self.daemon = SchedulerDaemon(
            dispatcher, poll_seconds=0.5, heartbeat_seconds=15
        )

    def tearDown(self):
        self.temp.cleanup()

    def test_read_only_worker_completes_one_scheduler_round(self):
        _batch, ids = self.scheduler.submit_batch(
            [
                TaskSpec(
                    "ping",
                    {
                        "objective": (
                            "Do not use tools. Reply with only this JSON object "
                            "and no other text: "
                            '{"status":"ok","evidence":"v0.1.3-grok-live"}'
                        )
                    },
                    acceptance={"requires": ["status", "evidence"]},
                    workspace_mode=WorkspaceMode.READ_ONLY,
                    retry_policy=RetryPolicy(max_attempts=1, retry_classes=()),
                    partition="general",
                )
            ]
        )
        totals = self.daemon.run_until_idle(max_wait_seconds=120)
        self.assertGreaterEqual(totals.get("dispatched", 0), 1)
        task = self.scheduler.get("tasks", ids["ping"])
        self.assertEqual(task["state"], "COMPLETED")
        results = self.scheduler.list("results")
        self.assertEqual(len(results), 1)
        payload = json_loads(results[0]["payload_json"], {})
        self.assertEqual(payload.get("status"), "ok")
        self.assertEqual(payload.get("evidence"), "v0.1.3-grok-live")
        self.assertEqual(results[0]["state"], "AVAILABLE")
        batches = self.scheduler.list("batches")
        self.assertEqual(batches[0]["state"], "COMPLETED")
        executions = self.scheduler.list("executions")
        self.assertEqual(executions[0]["state"], "SUCCEEDED")
        outbox = self.scheduler.list("notification_outbox")
        self.assertEqual(len(outbox), 1)
        self.assertEqual(outbox[0]["event_type"], "BATCH_RESULTS_READY")
        self.assertEqual(outbox[0]["state"], "DELIVERED")
        self.assertEqual(self.scheduler.status()["counts"]["active_leases"], 0)
        self.assertEqual(self.scheduler.list("escalations", state="OPEN"), [])
        self.assertEqual(self.scheduler.status()["counts"]["integrity"], "ok")
        self.scheduler.ack_result(results[0]["id"], consumer_ref="live-round")
        self.assertEqual(
            self.scheduler.get("results", results[0]["id"])["state"], "ACKED"
        )


@unittest.skipUnless(
    os.environ.get("AGENTYPE_GROK_LIVE") == "1",
    "set AGENTYPE_GROK_LIVE=1 to run the real Grok dormant Root round",
)
class GrokDormantRootLiveCase(unittest.TestCase):
    def setUp(self):
        executable = _grok_executable()
        if executable is None:
            self.skipTest("grok executable not found")
        self.executable = executable
        self.temp = tempfile.TemporaryDirectory(prefix="grok-dormant-root-")
        self.root = Path(self.temp.name)
        self.bridge_methods: list[str] = []

    def tearDown(self):
        self.temp.cleanup()

    def _bootstrap_dormant_root(self) -> str:
        command = (
            self.executable,
            "--sandbox",
            "read-only",
            "agent",
            "--always-approve",
            "stdio",
        )
        session = AcpSession(command, str(self.root), 90.0)
        try:
            created = session.request(
                "session/new",
                {
                    "cwd": str(self.root),
                    "mcpServers": [],
                    "_meta": {"yoloMode": True},
                },
                timeout=90.0,
            )
            session_id = str(created.get("sessionId") or "")
            if not session_id:
                self.fail("root fixture session/new did not return sessionId")
            session.request(
                "session/prompt",
                {
                    "sessionId": session_id,
                    "prompt": [
                        {
                            "type": "text",
                            "text": (
                                "You are the Local Agent Scheduler Root. Do not use tools. "
                                "Reply with only: dormant-ready. Later notification turns "
                                "will identify EVENT_ID; acknowledge that id only."
                            ),
                        }
                    ],
                },
                timeout=90.0,
            )
            return session_id
        finally:
            session.close(timeout=2.0)

    def _recording_factory(self, command, process_cwd, timeout):
        session = AcpSession(command, process_cwd, timeout)
        original = session.request

        def request(method, params, **kwargs):
            self.bridge_methods.append(method)
            return original(method, params, **kwargs)

        session.request = request  # type: ignore[method-assign]
        return session

    def test_dormant_root_wakeup_delivers_batch_results_ready(self):
        root_session_id = self._bootstrap_dormant_root()
        scheduler = Scheduler(Database(self.root / "scheduler.db"), lease_seconds=180)
        scheduler.initialize()
        scheduler.upsert_partition(
            PartitionSpec("general", 1, Retention.EPHEMERAL, "local_grok", "default")
        )
        scheduler.reconcile_pool()
        worker = GrokAcpAdapter(
            command=(self.executable, "agent", "--always-approve", "stdio"),
            process_cwd=str(self.root),
            sandbox="workspace",
            request_timeout=90.0,
            profile_options={"default": {"model": "grok-build"}},
        )
        bridge = GrokAcpRootBridge(
            root_session_id=root_session_id,
            command=(self.executable, "agent", "--always-approve", "stdio"),
            process_cwd=str(self.root),
            request_timeout=90.0,
            completion_timeout=120.0,
            session_factory=self._recording_factory,
        )
        dispatcher = Dispatcher(
            scheduler,
            adapters={"local_grok": worker},
            targets={
                "local_grok": ExecutionTargetConfig(
                    "local_grok", "grok_acp", False, True
                )
            },
            execution_profiles={"default"},
            workspace_root=str(self.root),
            outbox=OutboxDispatcher(scheduler.db, bridge),
        )
        daemon = SchedulerDaemon(dispatcher, poll_seconds=0.5, heartbeat_seconds=15)
        _batch, ids = scheduler.submit_batch(
            [
                TaskSpec(
                    "ping",
                    {
                        "objective": (
                            "Do not use tools. Reply with only this JSON object "
                            "and no other text: "
                            '{"status":"ok","evidence":"v0.1.3-grok-live"}'
                        )
                    },
                    acceptance={"requires": ["status", "evidence"]},
                    workspace_mode=WorkspaceMode.READ_ONLY,
                    retry_policy=RetryPolicy(max_attempts=1, retry_classes=()),
                    partition="general",
                )
            ]
        )
        daemon.run_until_idle(max_wait_seconds=180)
        self.assertEqual(scheduler.get("tasks", ids["ping"])["state"], "COMPLETED")
        results = scheduler.list("results")
        self.assertEqual(len(results), 1)
        payload = json_loads(results[0]["payload_json"], {})
        self.assertEqual(payload.get("evidence"), "v0.1.3-grok-live")
        outbox = scheduler.list("notification_outbox")
        self.assertEqual(len(outbox), 1)
        self.assertEqual(outbox[0]["event_type"], "BATCH_RESULTS_READY")
        self.assertEqual(outbox[0]["state"], "DELIVERED")
        self.assertIn("session/load", self.bridge_methods)
        self.assertIn("session/prompt", self.bridge_methods)
        self.assertNotIn("session/new", self.bridge_methods)
        self.assertEqual(scheduler.status()["counts"]["active_leases"], 0)
        self.assertEqual(scheduler.list("escalations", state="OPEN"), [])


if __name__ == "__main__":
    unittest.main()
