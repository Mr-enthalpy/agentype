from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


REPOSITORY = Path(__file__).resolve().parents[1]
SOURCE = REPOSITORY / "src"


class ProcessRestartIntegrationCase(unittest.TestCase):
    def test_claim_survives_process_exit_and_is_reconciled_by_new_process(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            database = Path(temporary) / "restart.db"
            environment = dict(os.environ)
            environment["PYTHONPATH"] = str(SOURCE)
            first = """
from local_agent_scheduler.core import Scheduler
from local_agent_scheduler.enums import FailureClass, Retention
from local_agent_scheduler.models import PartitionSpec, RetryPolicy, TaskSpec
from local_agent_scheduler.storage import Database
import sys
s = Scheduler(Database(sys.argv[1]), lease_seconds=10)
s.initialize()
s.upsert_partition(PartitionSpec('general', 1, Retention.RESIDENT, 'local', 'default'))
s.reconcile_pool()
policy = RetryPolicy(max_attempts=2, retry_classes=(FailureClass.EXECUTION_LOST,), base_backoff_seconds=0, max_backoff_seconds=0)
s.submit_batch([TaskSpec('restart', {}, retry_policy=policy)])
agent = s.list('logical_agents', state='READY')[0]
claim = s.claim_next(agent['id'], now=100)
print(claim.task_id)
"""
            claimed = subprocess.run(
                [sys.executable, "-c", first, str(database)],
                check=True,
                capture_output=True,
                text=True,
                env=environment,
            ).stdout.strip()
            second = """
from local_agent_scheduler.core import Scheduler
from local_agent_scheduler.storage import Database
import sys
s = Scheduler(Database(sys.argv[1]), lease_seconds=10)
s.initialize()
result = s.expire_leases(now=111)
s.promote_retry_wait(now=111)
print(result['retried'], s.get('tasks', sys.argv[2])['state'], s.db.integrity_check())
"""
            recovered = subprocess.run(
                [sys.executable, "-c", second, str(database), claimed],
                check=True,
                capture_output=True,
                text=True,
                env=environment,
            ).stdout.strip()
            self.assertEqual(recovered, "1 QUEUED ok")


if __name__ == "__main__":
    unittest.main()
