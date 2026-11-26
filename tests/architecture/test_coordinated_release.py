"""DEPLOY-05: durable real filesystem journal, component fault injection."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import os

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import coordinated_release as release
import managed_systemd_release as unit

A, B = "a" * 40, "b" * 40
POLICY = (ROOT / "ops/systemd/40-tor-release.conf").read_bytes()
OLD = POLICY.replace(b"RestartSec=2s", b"RestartSec=5s")


class Crash(BaseException):
    """Model SIGKILL between a component mutation and the next journal write."""


class World:
    def __init__(self, root):
        self.root = root
        self.journal = release.Journal(root / "transactions")
        self.directory = root / "dropins"
        self.directory.mkdir()
        self.path = self.directory / unit.NAME
        self.path.write_bytes(OLD)
        self.path.chmod(0o644)
        self.releases = root / "edge"
        self.releases.mkdir()
        self.edge = self.live_edge = self.app = self.live_app = A
        self.baseline = {name: "unchanged" for name in unit.PROPERTIES}
        self.baseline.update(User="tor-deploy", Group="tor-deploy",
                             WorkingDirectory="/opt/tor")
        self.baseline.update(unit.policy(OLD))
        self.props = self.baseline.copy()
        self.fail = None
        self.crash = None
        self.restore_fail = None
        self.events = []

    def step(self, name, phase):
        record = self.journal.load()
        assert record.phase == phase and record.state == "active"
        self.events.append(name)
        if self.crash == name:
            raise Crash(name)
        if self.fail == name:
            raise RuntimeError(name)

    # edge store and nginx boundary
    def active_sha(self):
        return self.edge

    def prepare(self, *args):
        pass

    def test(self):
        pass

    def probe(self, sha, directory):
        assert sha == self.live_edge

    def activate(self, sha, nginx):
        self.edge = self.live_edge = sha
        self.step("edge", "edge")

    def recover(self, previous, candidate, nginx):
        assert self.edge in (previous, candidate)
        if self.restore_fail == "edge":
            raise RuntimeError("edge")
        self.edge = self.live_edge = previous
        self.events.append("restore_edge")

    # UnitPolicy + systemd boundary
    def plan(self, *args):
        return unit.Receipt(OLD, POLICY, self.baseline.copy())

    def snapshot(self):
        return self.props.copy()

    def apply_receipt(self, receipt, systemd):
        self.path.write_bytes(receipt.candidate)
        self.props.update(unit.policy(receipt.candidate))
        self.step("unit", "unit")

    def restore(self, receipt, systemd):
        if self.restore_fail == "unit":
            raise RuntimeError("unit")
        self.path.write_bytes(receipt.previous)
        self.props = receipt.baseline.copy()
        self.events.append("restore_unit")


class App:
    def __init__(self, world):
        self.world = world

    def current(self):
        return self.world.app

    def preflight(self, sha):
        assert sha == B

    def verify(self, sha):
        assert self.world.app == self.world.live_app == sha
        if sha == B:
            self.world.step("verify", "verify")

    def activate(self, sha):
        self.world.app = self.world.live_app = sha
        self.world.step("app", "app")

    def restore(self, previous, candidate):
        w = self.world
        assert w.app in (previous, candidate)
        if w.restore_fail == "app":
            raise RuntimeError("app")
        # Verify the old unit has already been restored, before restart.
        assert w.path.read_bytes() == OLD
        w.app = w.live_app = previous
        w.events.append("restore_app")


class CoordinatorTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.world = World(self.root)

    def coordinator(self):
        w = self.world
        # Re-open disk journal (no in-memory receipt survives interruption).
        return release.Coordinator(release.Journal(w.journal.directory),
                                   w, w, w, w, App(w))

    def run_release(self):
        return self.coordinator().run(self.root, B, self.root / "signers")

    def assert_old(self):
        w = self.world
        self.assertEqual((w.app, w.live_app, w.edge, w.live_edge), (A, A, A, A))
        self.assertEqual(w.path.read_bytes(), OLD)
        self.assertEqual(w.props, w.baseline)
        self.assertEqual(w.journal.load().state, "rolled_back")

    def test_commit_only_after_final_verification(self):
        record = self.run_release()
        self.assertEqual(record.state, "committed")
        self.assertEqual(self.world.journal.load(), record)
        self.assertEqual(self.world.events, ["edge", "unit", "app", "verify"])
        self.assertEqual(self.world.journal.path.stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.world.journal.directory.stat().st_mode & 0o777, 0o700)
        self.assertEqual(self.coordinator().recover(), record)

    def test_failure_at_each_mutation_recovers(self):
        for i, phase in enumerate(("edge", "unit", "app", "verify")):
            with self.subTest(phase=phase):
                root = self.root / str(i)
                root.mkdir()
                self.world = World(root)
                self.world.fail = phase
                with self.assertRaisesRegex(release.ReleaseError, "TOR_ROLLBACK_VERIFIED"):
                    self.run_release()
                self.assert_old()
                if phase in ("edge", "unit"):
                    self.assertNotIn("restore_app", self.world.events)

    def test_crash_at_each_mutation_resumes_from_disk(self):
        for i, phase in enumerate(("edge", "unit", "app", "verify")):
            with self.subTest(phase=phase):
                root = self.root / str(i)
                root.mkdir()
                self.world = World(root)
                self.world.crash = phase
                with self.assertRaises(Crash):
                    self.run_release()
                self.assertEqual(self.world.journal.load().phase, phase)
                with self.assertRaisesRegex(release.ReleaseError, "unfinished"):
                    self.run_release()
                self.world.crash = None
                self.coordinator().recover()
                self.assert_old()

    def test_rollback_failure_preserves_journal_and_retry_recovers(self):
        for i, component in enumerate(("edge", "unit", "app")):
            with self.subTest(component=component):
                root = self.root / str(i)
                root.mkdir()
                self.world = World(root)
                self.world.fail = "app"
                self.world.restore_fail = component
                with self.assertRaisesRegex(release.ReleaseError, "TOR_ROLLBACK_UNVERIFIED"):
                    self.run_release()
                self.assertEqual(self.world.journal.load().state, "rollback_pending")
                if component == "unit":
                    self.assertNotIn("restore_app", self.world.events)
                self.world.restore_fail = None
                self.coordinator().recover()
                self.assert_old()

    def test_recovery_reconciles_disk_and_live_edge_drift(self):
        self.world.crash = "edge"
        with self.assertRaises(Crash):
            self.run_release()
        self.world.live_edge = A
        self.world.crash = None
        self.coordinator().recover()
        self.assert_old()

    def test_corrupt_or_unsafe_journal_refused(self):
        self.world.journal.ensure()
        path = self.world.journal.path
        path.write_text('{"version":1,"version":1}')
        path.chmod(0o600)
        with self.assertRaises(release.ReleaseError):
            self.run_release()
        path.unlink()
        path.symlink_to(self.world.path)
        with self.assertRaises(ValueError):
            self.coordinator().recover()
        self.assertEqual(self.world.path.read_bytes(), OLD)

    def test_journal_never_serializes_environment_values(self):
        self.run_release()
        raw = self.world.journal.path.read_bytes()
        self.assertNotIn(b'"Environment":', raw)
        self.assertNotIn(b'"DATABASE_URL"', raw)
        self.assertEqual(release.Record.decode(raw), self.world.journal.load())


if __name__ == "__main__":
    unittest.main()
