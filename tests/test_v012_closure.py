from __future__ import annotations

import sys
import tempfile
import time
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from local_agent_scheduler.adapters.codex import CodexAppServerAdapter
from local_agent_scheduler.config import ExecutionTargetConfig
from local_agent_scheduler.core import Scheduler
from local_agent_scheduler.enums import (
    AgentState,
    ContinuityPreference,
    ExecutionState,
    FailureClass,
    Retention,
    WorkspaceMode,
)
from local_agent_scheduler.errors import InvalidTransition
from local_agent_scheduler.models import (
    ExecutionRequest,
    PartitionSpec,
    RetryPolicy,
    TaskSpec,
)
from local_agent_scheduler.runtime import Dispatcher, SchedulerDaemon
from local_agent_scheduler.root_bridge import OutboxDispatcher
from local_agent_scheduler.storage import Database


class CapturingSession:
    instances: list["CapturingSession"] = []

    def __init__(self, *_args, **_kwargs):
        self.session_id = f"session-{len(self.instances)}"
        self.calls: list[tuple[str, dict]] = []
        self.process = type("Process", (), {"poll": lambda self: None})()
        self.instances.append(self)

    def request(self, method, params):
        self.calls.append((method, params))
        if method == "thread/start":
            return {"thread": {"id": "thread"}}
        return {"turn": {"id": "turn"}}

    def notifications(self):
        return []

    def close(self):
        return True


class SlowBridge:
    def __init__(self, delay: float):
        self.delay = delay

    def deliver(self, *_args):
        time.sleep(self.delay)
        return type("Delivery", (), {"delivered": True, "detail": None})()


class ClosureCase(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.db = Database(Path(self.temp.name) / "scheduler.db")
        self.scheduler = Scheduler(self.db, lease_seconds=2)
        self.scheduler.initialize()
        self.scheduler.upsert_partition(
            PartitionSpec("general", 1, Retention.RESIDENT, "codex", "default")
        )
        self.scheduler.reconcile_pool()

    def tearDown(self):
        self.temp.cleanup()

    def agent(self):
        return self.scheduler.list("logical_agents", state="READY")[0]["id"]

    def test_batch_suspension_is_transactional_claim_barrier(self):
        policy = RetryPolicy(max_attempts=1, retry_classes=())
        batch, _ids = self.scheduler.submit_batch(
            [TaskSpec("bad", {}, retry_policy=policy), TaskSpec("sibling", {})]
        )
        claim = self.scheduler.claim_next(self.agent())
        self.scheduler.nack(
            claim.attempt_id, claim.lease_epoch, failure_class=FailureClass.UNKNOWN
        )
        self.assertEqual(self.scheduler.get("batches", batch)["state"], "SUSPENDED")
        self.assertIsNone(self.scheduler.claim_next(self.agent()))
        self.assertIsNone(self.scheduler.claim_next_available())

    def test_writer_success_without_quiescence_suspends_without_result(self):
        _batch, ids = self.scheduler.submit_batch(
            [TaskSpec("writer", {}, workspace_mode=WorkspaceMode.WRITE)]
        )
        claim = self.scheduler.claim_next(self.agent())
        execution, _ = self.scheduler.create_execution(claim)
        result = self.scheduler.ack_success(
            claim.attempt_id,
            claim.lease_epoch,
            execution_id=execution,
            payload={"reported": "success"},
            quiescent_confirmed=False,
        )
        self.assertIsNone(result)
        self.assertEqual(self.scheduler.get("tasks", ids["writer"])["state"], "SUSPENDED")
        self.assertEqual(self.scheduler.list("results"), [])
        self.assertEqual(
            self.scheduler.get("logical_agents", claim.logical_agent_id)["state"],
            "SUSPENDED",
        )

    def test_writer_claim_before_execution_is_safe_to_retry(self):
        policy = RetryPolicy(
            max_attempts=2,
            retry_classes=(FailureClass.EXECUTION_LOST,),
            base_backoff_seconds=0,
            max_backoff_seconds=0,
        )
        _batch, ids = self.scheduler.submit_batch(
            [TaskSpec("writer", {}, workspace_mode=WorkspaceMode.WRITE, retry_policy=policy)]
        )
        claim = self.scheduler.claim_next(self.agent(), now=10)
        self.assertIsNone(claim.incarnation_id)
        expired = self.scheduler.expire_leases(now=13)
        self.assertEqual(expired["retried"], 1)
        self.scheduler.promote_retry_wait(now=13)
        self.assertEqual(self.scheduler.get("tasks", ids["writer"])["state"], "QUEUED")

    def test_cancelled_writer_keeps_safety_escalation_open(self):
        _batch, ids = self.scheduler.submit_batch(
            [TaskSpec("writer", {}, workspace_mode=WorkspaceMode.WRITE)]
        )
        claim = self.scheduler.claim_next(self.agent())
        self.scheduler.create_execution(claim)
        self.scheduler.cancel_task(ids["writer"])
        escalation = self.scheduler.list("escalations", state="OPEN")[0]
        self.scheduler.resolve_escalation(escalation["id"], operation="cancel_task")
        self.assertEqual(
            self.scheduler.get("escalations", escalation["id"])["state"], "OPEN"
        )
        with self.assertRaises(InvalidTransition):
            self.scheduler.revive_agent(claim.logical_agent_id, "codex")

    def test_suspended_member_does_not_block_capacity_birth(self):
        old = self.agent()
        with self.db.transaction() as conn:
            conn.execute("UPDATE logical_agents SET state='SUSPENDED' WHERE id=?", (old,))
        result = self.scheduler.reconcile_pool()
        self.assertEqual(result["born"], 1)
        self.assertNotEqual(self.agent(), old)

    def test_preferred_workstream_agent_wins_global_match(self):
        ws = self.scheduler.create_workstream("same")
        generic = self.agent()
        preferred = self.scheduler.birth_agent("general", workstream_id=ws)
        _batch, _ids = self.scheduler.submit_batch(
            [
                TaskSpec(
                    "affinity",
                    {},
                    workstream_id=ws,
                    continuity=ContinuityPreference.PREFERRED,
                )
            ]
        )
        claim = self.scheduler.claim_next_available()
        self.assertEqual(claim.logical_agent_id, preferred)
        self.assertNotEqual(claim.logical_agent_id, generic)

    def test_idle_ready_agent_has_no_fake_incarnation_and_sequential_reuse_allowed(self):
        agent = self.agent()
        self.assertEqual(self.scheduler.list("incarnations"), [])
        _batch, _ids = self.scheduler.submit_batch([TaskSpec("one", {})])
        first = self.scheduler.claim_next(agent)
        first_execution, _ = self.scheduler.create_execution(first)
        incarnation = self.scheduler.get("executions", first_execution)["incarnation_id"]
        self.scheduler.ack_success(
            first.attempt_id,
            first.lease_epoch,
            execution_id=first_execution,
            payload={},
            incarnation_reusable=True,
        )
        _batch, _ids = self.scheduler.submit_batch([TaskSpec("two", {})])
        second = self.scheduler.claim_next(agent)
        second_execution, _ = self.scheduler.create_execution(second)
        self.assertEqual(
            self.scheduler.get("executions", second_execution)["incarnation_id"],
            incarnation,
        )

    def test_read_only_sandbox_and_live_ambiguous_identity(self):
        CapturingSession.instances.clear()
        adapter = CodexAppServerAdapter(session_factory=CapturingSession)
        request = ExecutionRequest(
            "request", "execution", "task", "attempt", 1, "agent", "inc", "codex",
            "default", self.temp.name, "inspect", WorkspaceMode.READ_ONLY, {}, {}
        )
        started = adapter.start_execution(request)
        self.assertEqual(started.state, ExecutionState.RUNNING)
        self.assertEqual(CapturingSession.instances[0].calls[0][1]["sandbox"], "read-only")
        incomplete = {
            "adapter_session_id": CapturingSession.instances[0].session_id,
            "thread_id": "thread",
        }
        self.assertEqual(adapter.observe_execution(incomplete).state, ExecutionState.UNKNOWN)

    def test_topology_bootstrap_does_not_resurrect_retired_partition(self):
        with tempfile.TemporaryDirectory() as temporary:
            database = Database(Path(temporary) / "topology.db")
            first = Scheduler(database)
            first.initialize()
            spec = PartitionSpec(
                "obsolete", 1, Retention.RESIDENT, "codex", "default"
            )
            self.assertTrue(first.bootstrap_partitions([spec]))
            first.retire_partition("obsolete")
            restarted = Scheduler(database)
            restarted.initialize()
            self.assertFalse(restarted.bootstrap_partitions([spec]))
            partition = restarted.list("pool_partitions")[0]
            self.assertEqual(partition["active"], 0)
            self.assertEqual(partition["desired_capacity"], 0)

    def test_cross_target_move_waits_for_physical_presence_boundary(self):
        self.scheduler.upsert_partition(
            PartitionSpec("other", 0, Retention.RESIDENT, "other", "default")
        )
        agent = self.agent()
        with self.db.transaction() as conn:
            incarnation = self.scheduler._ensure_incarnation(conn, agent, "codex", time.time())
            conn.execute("UPDATE incarnations SET state='WARM' WHERE id=?", (incarnation,))
        self.scheduler.move_agent(agent, "other")
        draining = self.scheduler.get("logical_agents", agent)
        self.assertEqual(draining["state"], "DRAINING")
        self.assertEqual(draining["partition_name"], "general")
        self.scheduler.mark_incarnation_lost(incarnation)
        moved = self.scheduler.get("logical_agents", agent)
        self.assertEqual(moved["state"], "READY")
        self.assertEqual(moved["partition_name"], "other")

    def test_background_heartbeat_survives_slow_bridge_io(self):
        _batch, _ids = self.scheduler.submit_batch([TaskSpec("leased", {})])
        claim = self.scheduler.claim_next(self.agent())
        with self.db.transaction() as conn:
            conn.execute(
                "UPDATE leases SET expires_at=? WHERE id=?",
                (time.time() + 0.1, claim.lease_id),
            )
        dispatcher = Dispatcher(
            self.scheduler,
            adapters={},
            targets={},
            workspace_root=self.temp.name,
        )
        daemon = SchedulerDaemon(
            dispatcher, poll_seconds=0.01, heartbeat_seconds=0.03
        )
        daemon._start_supervision()
        try:
            SlowBridge(0.4).deliver("event", "CONTROL", {})
        finally:
            daemon._stop_supervision()
        self.assertEqual(self.scheduler.expire_leases(now=time.time())["retried"], 0)
        self.assertEqual(
            self.scheduler.get("leases", claim.lease_id)["state"], "ACTIVE"
        )


if __name__ == "__main__":
    unittest.main()
