"""Real filesystem/lock tests and injected application boundary checks."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import release_executor_v2 as executor

A, B, C = "a" * 40, "b" * 40, "c" * 40


class ExecutorTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.releases = self.root / "releases"
        self.releases.mkdir()
        for sha in (A, B, C):
            path = self.releases / sha
            path.mkdir()
            binary = path / "tor-web"
            binary.write_bytes(sha.encode())
            binary.chmod(0o755)
        self.store = executor.AppStore(self.root)
        self.store.link.symlink_to(self.releases / A)

    def test_atomic_restore_and_idempotent_retry(self):
        self.store.switch(B, A)
        self.assertEqual(self.store.current(), B)
        self.store.switch(A, B)
        self.store.switch(A, B)
        self.assertEqual(self.store.current(), A)

    def test_unrelated_release_refuses_restore(self):
        self.store.link.unlink()
        self.store.link.symlink_to(self.releases / C)
        with self.assertRaises(executor.ExecutorError):
            self.store.switch(A, B)
        self.assertEqual(self.store.current(), C)

    def test_foreign_symlink_refused(self):
        self.store.link.unlink()
        self.store.link.symlink_to(self.root)
        with self.assertRaises(executor.ExecutorError):
            self.store.current()

    def test_lock_inheritance_blocks_other_process(self):
        lock = self.root / "release.lock"
        with executor.release_lock(lock) as fd:
            with self.assertRaises(BlockingIOError):
                with executor.release_lock(lock):
                    pass
            child = subprocess.Popen(
                [sys.executable, "-c", "import sys; sys.stdin.read()"],
                stdin=subprocess.PIPE, pass_fds=(fd,),
            )
        try:
            with self.assertRaises(BlockingIOError):
                with executor.release_lock(lock):
                    pass
        finally:
            child.communicate(timeout=5)
        with executor.release_lock(lock):
            pass

    def test_live_executable_inode_is_checked(self):
        self.assertTrue(executor.same_executable(os.getpid(), Path(sys.executable)))
        self.assertFalse(executor.same_executable(os.getpid(), self.releases / A / "tor-web"))

    def test_old_process_rejected_even_when_pointer_is_new(self):
        app = executor.Application(self.store, self.root / "signers", 9)
        self.store.switch(B, A)
        with patch.object(executor, "trace_inactive"), \
             patch.object(executor, "service_pid", return_value=123), \
             patch.object(executor, "same_executable", return_value=False), \
             patch.object(executor, "readiness") as ready:
            with self.assertRaisesRegex(executor.ExecutorError, "live Rust"):
                app.verify(B)
            ready.assert_not_called()

    def test_process_restart_during_readiness_is_refused(self):
        app = executor.Application(self.store, self.root / "signers", 9)
        with patch.object(executor, "trace_inactive"), \
             patch.object(executor, "service_pid", side_effect=[123, 124]), \
             patch.object(executor, "same_executable", return_value=True), \
             patch.object(executor, "readiness"):
            with self.assertRaisesRegex(executor.ExecutorError, "changed"):
                app.verify(A)

    def test_active_trace_store_blocks_update(self):
        result = subprocess.CompletedProcess([], 0, b"LoadState=loaded\nActiveState=active\nUnitFileState=enabled\n")
        with patch.object(executor, "command", return_value=result):
            with self.assertRaisesRegex(executor.ExecutorError, "Trace Store"):
                executor.trace_inactive()

    def test_absent_trace_store_allowed(self):
        result = subprocess.CompletedProcess([], 1, b"LoadState=not-found\nActiveState=inactive\nUnitFileState=\n")
        with patch.object(executor, "command", return_value=result):
            executor.trace_inactive()

    def test_both_readiness_endpoints_required(self):
        responses = [subprocess.CompletedProcess([], 0, b"ok\n"),
                     subprocess.CompletedProcess([], 0, b"not ready\n")]
        with patch.object(executor, "command", side_effect=responses) as cmd:
            with self.assertRaises(executor.ExecutorError):
                executor.readiness()
            self.assertEqual(cmd.call_count, 2)
            self.assertIn("https://pandaolate.ru/readyz", cmd.call_args.args)


    def test_database_policy_shared_guard_refuses_missing_activation_or_config(self):
        valid = b"Environment=TOR_DEMO_MODE=0\nEnvironment=TOR_AUTH_ENABLED=1\nEnvironment=TOR_PUBLIC_ORIGIN=https://pandaolate.ru\nEnvironmentFile=/etc/tor/tor-app.env\n"
        with patch.object(executor, "trace_inactive"), \
             patch.object(executor, "command"), \
             patch.object(executor.edge, "read_small", side_effect=[b"PG_ACTIVATED\n", valid]):
            executor.require_database_policy()
        for data in ([b"not activated"], [b"PG_ACTIVATED\n", b""]):
            with patch.object(executor, "trace_inactive"), \
                 patch.object(executor, "command"), \
                 patch.object(executor.edge, "read_small", side_effect=data):
                with self.assertRaises(executor.ExecutorError):
                    executor.require_database_policy()

    def test_current_main_only(self):
        for payload in (b'{}', b'{"ref":"refs/heads/main","object":{"sha":"bad"}}'):
            with patch.object(executor, "command", return_value=subprocess.CompletedProcess([], 0, payload)):
                with self.assertRaises(executor.ExecutorError):
                    executor.require_main(A)


if __name__ == "__main__":
    unittest.main()
