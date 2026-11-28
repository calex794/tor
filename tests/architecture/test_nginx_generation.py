"""Reload acknowledgement is not proof of a new worker generation."""
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from managed_edge_release import EdgeError, await_worker_generation

MASTER = (100, "12345")
OLD = frozenset({101, 102})
NEW = frozenset({103, 104})


class GenerationTests(unittest.TestCase):
    def run_trace(self, samples, masters=None):
        workers = iter(samples)
        identities = iter(masters) if masters is not None else None
        delays = []
        await_worker_generation(
            MASTER, OLD, lambda: next(identities) if identities else MASTER,
            lambda pid: next(workers), attempts=len(samples), wait=delays.append,
        )
        return delays

    def test_waits_until_old_workers_stop_accepting(self):
        delays = self.run_trace([OLD, OLD | NEW, NEW, NEW, NEW])
        self.assertEqual(len(delays), 4)

    def test_reappearance_resets_stability_window(self):
        delays = self.run_trace([NEW, OLD | NEW, NEW, NEW, NEW])
        self.assertEqual(len(delays), 4)

    def test_no_new_workers_is_not_success(self):
        with self.assertRaisesRegex(EdgeError, "did not converge"):
            self.run_trace([OLD, frozenset(), OLD, OLD])

    def test_master_restart_or_pid_reuse_refused(self):
        with self.assertRaisesRegex(EdgeError, "master changed"):
            self.run_trace([NEW, NEW, NEW], [MASTER, (100, "12346"), MASTER])


if __name__ == "__main__":
    unittest.main()
