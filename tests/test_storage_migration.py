from __future__ import annotations

import sys
import tempfile
import unittest
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from local_agent_scheduler.core import Scheduler
from local_agent_scheduler.enums import AgentState, Retention
from local_agent_scheduler.models import PartitionSpec, TaskSpec
from local_agent_scheduler.storage import Database, SCHEMA_VERSION


class IncarnationMigrationCase(unittest.TestCase):
    @staticmethod
    def _downgrade_schema_markers(database: Database) -> None:
        with database.transaction() as conn:
            conn.execute("DROP INDEX one_execution_per_incarnation")
            conn.execute("DROP INDEX one_active_incarnation_per_agent")
            conn.execute("DELETE FROM schema_migrations WHERE version=2")

    def test_v1_reused_incarnation_is_split_without_losing_history(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            database = Database(Path(temporary) / "scheduler.db")
            scheduler = Scheduler(database)
            scheduler.initialize()
            scheduler.upsert_partition(
                PartitionSpec("general", 1, Retention.RESIDENT, "local", "default")
            )
            scheduler.reconcile_pool()
            agent_id = scheduler.list("logical_agents", state=AgentState.READY.value)[0]["id"]

            _batch, _ids = scheduler.submit_batch([TaskSpec("first", {})])
            first = scheduler.claim_next(agent_id)
            first_execution, _ = scheduler.create_execution(first)
            scheduler.confirm_execution_running(
                first.attempt_id,
                first.lease_epoch,
                first_execution,
                runtime_handle={"physical": "one"},
            )
            scheduler.ack_success(
                first.attempt_id,
                first.lease_epoch,
                execution_id=first_execution,
                payload={"ok": 1},
            )

            _batch, _ids = scheduler.submit_batch([TaskSpec("second", {})])
            second = scheduler.claim_next(agent_id)
            second_execution, _ = scheduler.create_execution(second)

            # Reconstruct the V0.1 defect: both Executions and Attempts point
            # at one WARM Incarnation, and the database reports schema v1.
            with database.transaction() as conn:
                conn.execute("DROP INDEX one_execution_per_incarnation")
                conn.execute("DROP INDEX one_active_incarnation_per_agent")
                conn.execute(
                    "UPDATE executions SET incarnation_id=? WHERE id=?",
                    (first.incarnation_id, second_execution),
                )
                conn.execute(
                    "UPDATE attempts SET incarnation_id=? WHERE id=?",
                    (first.incarnation_id, second.attempt_id),
                )
                conn.execute("DELETE FROM incarnations WHERE id=?", (second.incarnation_id,))
                conn.execute(
                    "UPDATE incarnations SET state='WARM',ended_at=NULL WHERE id=?",
                    (first.incarnation_id,),
                )
                conn.execute("DELETE FROM schema_migrations WHERE version=2")

            database.initialize()

            self.assertEqual(SCHEMA_VERSION, 2)
            self.assertEqual(
                database.fetch_one(
                    "SELECT MAX(version) AS version FROM schema_migrations"
                )["version"],
                2,
            )
            migrated_first = database.fetch_one(
                "SELECT incarnation_id FROM executions WHERE id=?", (first_execution,)
            )
            migrated_second = database.fetch_one(
                "SELECT incarnation_id FROM executions WHERE id=?", (second_execution,)
            )
            self.assertNotEqual(
                migrated_first["incarnation_id"], migrated_second["incarnation_id"]
            )
            self.assertEqual(
                database.fetch_one(
                    "SELECT state FROM incarnations WHERE id=?",
                    (migrated_first["incarnation_id"],),
                )["state"],
                "TERMINATED",
            )
            self.assertEqual(
                database.fetch_one(
                    "SELECT state FROM incarnations WHERE id=?",
                    (migrated_second["incarnation_id"],),
                )["state"],
                "LOST",
            )
            self.assertEqual(database.integrity_check(), "ok")

    def test_v1_single_unknown_execution_is_conservatively_lost(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            database = Database(Path(temporary) / "scheduler.db")
            scheduler = Scheduler(database)
            scheduler.initialize()
            scheduler.upsert_partition(
                PartitionSpec("general", 1, Retention.RESIDENT, "local", "default")
            )
            scheduler.reconcile_pool()
            agent_id = scheduler.list("logical_agents", state=AgentState.READY.value)[0]["id"]
            _batch, _ids = scheduler.submit_batch([TaskSpec("unknown", {})])
            claim = scheduler.claim_next(agent_id)
            execution_id, request_id = scheduler.create_execution(claim)
            scheduler.record_start_ambiguity(
                claim.attempt_id,
                claim.lease_epoch,
                execution_id,
                runtime_handle={"request_id": request_id},
            )
            self._downgrade_schema_markers(database)

            database.initialize()

            self.assertEqual(
                database.fetch_one(
                    "SELECT state FROM incarnations WHERE id=?", (claim.incarnation_id,)
                )["state"],
                "LOST",
            )
            self.assertEqual(
                database.fetch_one(
                    "SELECT state FROM attempts WHERE id=?", (claim.attempt_id,)
                )["state"],
                "ACTIVE",
            )

    def test_concurrent_first_initialize_is_idempotent(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "concurrent.db"
            with ThreadPoolExecutor(max_workers=4) as pool:
                errors = list(pool.map(lambda _index: Database(path).initialize(), range(4)))
            self.assertEqual(errors, [None, None, None, None])
            database = Database(path)
            self.assertEqual(
                database.fetch_one(
                    "SELECT MAX(version) AS version FROM schema_migrations"
                )["version"],
                SCHEMA_VERSION,
            )
            self.assertEqual(database.integrity_check(), "ok")


if __name__ == "__main__":
    unittest.main()
