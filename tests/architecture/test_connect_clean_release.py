"""Fresh helper connection refuses unrelated SSH/sudo rules and files."""
import base64
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import connect_clean_release as connect

KEY = b"ssh-ed25519 " + base64.b64encode(b"\x00\x00\x00\x0bssh-ed25519\x00\x00\x00\x20" + b"a" * 32)


class ConnectTests(unittest.TestCase):
    def test_signer_and_authorized_key_exact_match(self):
        self.assertEqual(connect.signer_key(b"tor-release " + KEY + b" comment\n"), KEY)
        connect.check_authorized(b"restrict " + KEY + b" comment\n", KEY)
        for line in (KEY, b"restrict " + KEY + b"\n" + KEY, b"restrict " + KEY + b"x",
                     b"command=\"sh\" " + KEY):
            with self.assertRaises(connect.Refused):
                connect.check_authorized(line, KEY)

    def test_invalid_signers_refused(self):
        for data in (b"other " + KEY, b"tor-release " + KEY + b"\ntor-release " + KEY,
                     b"tor-release ssh-ed25519 YQ=="):
            with self.assertRaises(connect.Refused):
                connect.signer_key(data)

    def test_exact_sudo_only(self):
        header = b"User tor-deploy may run the following commands on host:\n"
        rule = b"    (root) NOPASSWD: /usr/local/sbin/tor-one-click-release *\n"
        for data in (header + rule, header + rule + b"    (ALL) ALL\n", header + b"    (root) ALL\n"):
            with patch.object(connect.executor, "command", return_value=subprocess.CompletedProcess([], 0, data)):
                if data == header + rule:
                    connect.verify_sudo(installed=True)
                else:
                    with self.assertRaises(connect.Refused):
                        connect.verify_sudo(installed=True)

    def test_no_sudo_is_acceptable_only_before_install(self):
        with patch.object(connect.executor, "command", return_value=subprocess.CompletedProcess([], 1, b"")):
            connect.verify_sudo(installed=False)
            with self.assertRaises(connect.Refused):
                connect.verify_sudo(installed=True)

    def test_existing_helper_never_overwritten(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "wrapper"
            connect.ensure_file(path, b"original", 0o755)
            connect.ensure_file(path, b"original", 0o755)
            with self.assertRaises(connect.Refused):
                connect.ensure_file(path, b"changed", 0o755)
            self.assertEqual(path.read_bytes(), b"original")


if __name__ == "__main__":
    unittest.main()
