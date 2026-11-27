"""Exercise actual release build shell blocks with an isolated Git fixture.

Cargo is a controlled stub here. Real compilation is a separate CI proof.
"""

import hashlib
import os
from pathlib import Path
import re
import subprocess
import tempfile
import textwrap
import unittest

ROOT = Path(__file__).resolve().parents[2]
PROFILES = {
    "production": (
        "tor-one-click-release.yml",
        "Собрать сайт, миграции и писатель трасс на GitHub",
        [
            "build --locked --release -p tor-web --features s3-covers,s3-free,s3-paid,s3-chapter-import",
            "build --release --locked -p tor-migrations --bin tor-migrate",
            "build --release --locked -p tor-observability --bin tor-trace-store",
        ],
    ),
    "s3-free": (
        "tor-s3-candidate-build.yml",
        "Compile S3-enabled TOR without paid or runtime opt-in",
        [
            "build --release --locked -p tor-web --features s3-free,s3-chapter-import,s3-covers",
        ],
    ),
}
LOCK = b"# fixture lockfile\nversion = 4\n"


def build_block(workflow, name):
    body = (ROOT / ".github/workflows" / workflow).read_text()
    anchor = f'      - name: "{name}"'
    if anchor not in body:
        anchor = f"      - name: {name}"
    step = body.split(anchor + "\n", 1)[1].split("\n      - name:", 1)[0]
    return textwrap.dedent(step.split("        run: |\n", 1)[1])


class LockedReleaseBuildTests(unittest.TestCase):
    def exercise(self, profile, mode):
        workflow, name, expected = PROFILES[profile]
        with tempfile.TemporaryDirectory(prefix="tor-lock-release-") as directory:
            root = Path(directory)
            repo = root / "repo"
            repo.mkdir()
            temp = root / "runner"
            temp.mkdir()
            bin_dir = root / "bin"
            bin_dir.mkdir()
            calls = root / "cargo.calls"

            def git(*args):
                subprocess.run(
                    ["git", "-C", str(repo), *args],
                    capture_output=True, check=True,
                )

            git("init", "-q")
            git("config", "user.name", "Lock fixture")
            git("config", "user.email", "fixture@example.invalid")
            (repo / "tools").mkdir()
            (repo / "tools/check_feature_layout.py").write_text("pass\n")
            (repo / "tools/build_tor_production_web.sh").write_bytes(
                (ROOT / "tools/build_tor_production_web.sh").read_bytes())
            lock = repo / "Cargo.lock"
            if mode != "untracked":
                lock.write_bytes(b"" if mode == "empty" else LOCK)
            git("add", ".")
            git("commit", "-qm", "fixture")
            if mode == "missing":
                lock.unlink()
            elif mode == "untracked":
                lock.write_bytes(LOCK)
            elif mode in ("dirty", "staged"):
                lock.write_bytes(LOCK + b"# modified\n")
                if mode == "staged":
                    git("add", "Cargo.lock")

            cargo = bin_dir / "cargo"
            cargo.write_text(
                '#!/bin/bash\n'
                'set -eu\n'
                'printf "%s\\n" "$*" >> "$BUILD_CALLS"\n'
                'test "$1" = build && { [[ "$2:$3" == --release:--locked ]] || '
                '[[ "$2:$3" == --locked:--release ]]; }\n'
                'if [[ "$LOCK_MODE" == stale ]]; then exit 101; fi\n'
                'case "$5" in\n'
                '  tor-web) name=tor-web ;;\n'
                '  tor-migrations) name=tor-migrate ;;\n'
                '  tor-observability) name=tor-trace-store ;;\n'
                '  *) exit 2 ;;\n'
                'esac\n'
                'mkdir -p target/release\n'
                'printf "%s\\n" "#!/bin/bash" '
                '"echo TOR_RELEASE_CAPABILITIES s3-covers=true s3-free=true s3-paid=true s3-chapter-import=true" '
                '> "target/release/$name"\n'
                'chmod +x "target/release/$name"\n'
                'if [[ "$LOCK_MODE" == tamper ]]; then printf "# tampered\\n" >> Cargo.lock; fi\n'
            )
            cargo.chmod(0o755)
            env = dict(os.environ, PATH=str(bin_dir) + os.pathsep + os.environ["PATH"],
                       RUNNER_TEMP=str(temp), BUILD_CALLS=str(calls), LOCK_MODE=mode)
            result = subprocess.run(
                ["bash", "-c", build_block(workflow, name)],
                cwd=repo, env=env, capture_output=True, text=True,
            )
            actual = calls.read_text().splitlines() if calls.exists() else []
            evidence = temp / "tor-release-lock.sha256"
            if mode == "valid":
                self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
                self.assertEqual(actual, expected)
                self.assertEqual(evidence.read_text().split()[0], hashlib.sha256(LOCK).hexdigest())
                self.assertEqual(lock.read_bytes(), LOCK)
            else:
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertFalse(evidence.exists(), "failed build must not emit success checksum")
                if mode in ("missing", "untracked", "empty", "dirty", "staged"):
                    self.assertEqual(actual, [], "invalid snapshot must fail before Cargo")
                elif mode == "stale":
                    self.assertEqual(actual, expected[:1])
                else:
                    self.assertEqual(actual, expected)

    def test_exact_shell_positive_and_negative_controls(self):
        for profile in PROFILES:
            for mode in ("valid", "missing", "untracked", "empty", "dirty",
                         "staged", "stale", "tamper"):
                with self.subTest(profile=profile, mode=mode):
                    self.exercise(profile, mode)

    def test_lock_evidence_is_before_any_binary_signing_or_transfer(self):
        for workflow, name, expected in PROFILES.values():
            with self.subTest(workflow=workflow):
                body = (ROOT / ".github/workflows" / workflow).read_text()
                block = build_block(workflow, name)
                helper = "bash tools/build_tor_production_web.sh"
                first_build = helper if helper in block else "cargo build"
                expanded = block.replace(helper, (ROOT / "tools/build_tor_production_web.sh").read_text())
                commands = [x.replace('"', "").replace("'", "")
                            for x in re.findall(r"(?m)^cargo (.*)$", expanded)]
                self.assertEqual(commands, expected)
                self.assertLess(block.index("git diff --exit-code HEAD -- Cargo.lock"),
                                block.index(first_build))
                self.assertLess(block.rindex("cargo build"), block.index("cmp Cargo.lock"))
                self.assertLess(block.index("cmp Cargo.lock"), block.index("sha256sum Cargo.lock"))
                self.assertIn("tor-release-lock.sha256", body.split("uses: actions/upload-artifact@v4", 1)[1])
                if workflow == "tor-one-click-release.yml":
                    self.assertLess(body.index("sha256sum Cargo.lock"), body.index("ssh-keygen -Y sign"))
                    self.assertLess(body.index("sha256sum Cargo.lock"), body.index("scp -p"))


if __name__ == "__main__":
    unittest.main()
