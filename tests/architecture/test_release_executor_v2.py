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

    def test_v2_forward_guard_reuses_exact_common_ancestry(self):
        with patch.object(executor.ancestry, "verify_forward", return_value=C) as guard:
            self.assertEqual(executor.require_forward(B, A), C)
            guard.assert_called_once_with(executor.ancestry.public_get, B, A)
        with patch.object(executor.ancestry, "verify_forward",
                          side_effect=executor.ancestry.AncestryError("behind active")):
            with self.assertRaises(executor.ancestry.AncestryError):
                executor.require_forward(A, B)

    def test_v2_uses_forward_before_mutation_and_signed_shell_rechecks(self):
        source = (ROOT / "tools/release_executor_v2.py").read_text()
        self.assertEqual(source.count("require_forward(sha, self.current())"), 2)
        self.assertLess(source.index("require_forward(sha, app.current())"),
                        source.index("target = app_store.stage("))
        self.assertIn('["bash", str(path / "go-live-postgres-vps.sh"), sha, "--forward"]', source)
        live = (ROOT / "ops/go-live-postgres-vps.sh").read_text()
        self.assertIn("guard=/usr/local/lib/tor-release-v2/release_ancestry.py", live)
        self.assertIn('python3 -I "$guard" "$sha" --from-active', live)
        self.assertEqual(live.count("\nrequire_current_main_sha\n"), 2)
        self.assertIn("root:root:600:1", live)
        bootstrap = (ROOT / "ops/bootstrap-one-click-v2.sh").read_text()
        self.assertIn("install_release_v2.py release_ancestry.py; do", bootstrap)

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

    def test_outage_approval_is_exact_private_and_consumed_once(self):
        directory = self.root / "approval"
        directory.mkdir(mode=0o700)
        path = directory / "allow"
        path.write_bytes((A + "\n" + B + "\n").encode())
        path.chmod(0o600)
        executor.require_outage_approval(A, B, path=path)
        with self.assertRaises(executor.ExecutorError):
            executor.require_outage_approval(A, C, path=path, consume=True)
        self.assertTrue(path.exists())
        path.chmod(0o644)
        with self.assertRaises(executor.ExecutorError):
            executor.require_outage_approval(A, B, path=path, consume=True)
        path.chmod(0o600)
        alias = directory / "alias"
        os.link(path, alias)
        with self.assertRaises(executor.ExecutorError):
            executor.require_outage_approval(A, B, path=path, consume=True)
        alias.unlink()
        executor.require_outage_approval(A, B, path=path, consume=True)
        self.assertFalse(path.exists())
        with self.assertRaises(OSError):
            executor.require_outage_approval(A, B, path=path, consume=True)
        path.symlink_to(self.releases / A / "tor-web")
        with self.assertRaises(ValueError):
            executor.require_outage_approval(A, B, path=path, consume=True)

    def test_outage_requires_completed_v2_marker(self):
        path = self.root / "release-format"
        path.write_bytes(b"2\n")
        path.chmod(0o644)
        executor.require_v2_marker(path)
        for payload in (b"1\n", b"2", b"2\n\n"):
            path.write_bytes(payload)
            with self.assertRaises(executor.ExecutorError):
                executor.require_v2_marker(path)

    def test_outage_stopped_state_and_listener_are_checked(self):
        stopped = b"LoadState=loaded\nActiveState=inactive\nSubState=dead\nMainPID=0\nControlPID=0\n"
        def response(data):
            return subprocess.CompletedProcess([], 0, data)
        with patch.object(executor, "command", side_effect=[response(stopped), response(b"")]):
            executor.require_stopped_web()
        for status, listener in (
            (stopped.replace(b"inactive", b"active"), b""),
            (stopped.replace(b"MainPID=0", b"MainPID=123"), b""),
            (stopped + b"MainPID=0\n", b""),
            (stopped, b"LISTEN 127.0.0.1:3000"),
        ):
            with patch.object(executor, "command", side_effect=[response(status), response(listener)]):
                with self.assertRaises(executor.ExecutorError):
                    executor.require_stopped_web()

    def test_outage_verification_requires_real_offline_edge(self):
        with patch.object(executor, "trace_inactive"), \\
             patch.object(executor, "require_stopped_web") as stopped, \\
             patch.object(executor, "command", return_value=subprocess.CompletedProcess([], 0, b"502")):
            executor.Application(self.store, self.root / "signers", 9).verify_outage(A)
            self.assertEqual(stopped.call_count, 2)
        with patch.object(executor, "command", return_value=subprocess.CompletedProcess([], 0, b"200")):
            with self.assertRaisesRegex(executor.ExecutorError, "offline HTTPS"):
                executor.require_outage_edge()

    def test_outage_restore_stops_and_never_restarts_previous_binary(self):
        self.store.switch(B, A)
        app = executor.Application(self.store, self.root / "signers", 9)
        with patch.object(executor, "trace_inactive"), \\
             patch.object(executor, "require_stopped_web") as stopped, \\
             patch.object(executor, "command") as cmd:
            app.restore_outage(A, B)
            cmd.assert_called_once_with("systemctl", "stop", executor.APP, timeout=90)
            self.assertEqual(stopped.call_count, 2)
            self.assertEqual(self.store.current(), A)

    def test_outage_activation_uses_signed_recovery_shell_and_lock(self):
        app = executor.Application(self.store, self.root / "signers", 9)
        with patch.object(app, "verify_outage"), patch.object(app, "verify"), \\
             patch.object(executor, "require_forward"), patch.object(executor, "trace_inactive"), \\
             patch.object(executor.bundle, "verify"), \\
             patch.object(executor.subprocess, "Popen") as popen:
            popen.return_value.wait.return_value = 0
            app.activate_outage(B)
            popen.assert_called_once_with(
                ["bash", str(self.releases / B / "go-live-postgres-vps.sh"), B, "--recover", "--forward"],
                env=executor.SAFE_ENV, pass_fds=(9,))

    def test_root_outage_argument_is_separate_from_normal_wrapper(self):
        self.assertEqual(executor.release_request([A]), ("normal", A))
        self.assertEqual(executor.release_request(["--recover"]), ("journal", None))
        self.assertEqual(executor.release_request(["--repair-outage", B]), ("outage", B))
        for args in ([], ["--repair-outage"], ["--repair-outage", "bad"], [A, "--recover"]):
            with self.assertRaises(ValueError):
                executor.release_request(args)
        wrapper = ROOT / "ops/one-click-release-v2-wrapper.sh"
        result = subprocess.run(["bash", str(wrapper), "--repair-outage", B],
                                capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 2)

    def test_current_main_only(self):
        for payload in (b'{}', b'{"ref":"refs/heads/main","object":{"sha":"bad"}}'):
            with patch.object(executor, "command", return_value=subprocess.CompletedProcess([], 0, payload)):
                with self.assertRaises(executor.ExecutorError):
                    executor.require_main(A)


if __name__ == "__main__":
    unittest.main()
