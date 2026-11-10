"""Run pinned free-grant unit tests; no target branch or production access."""
from pathlib import Path
import hashlib
import json
import os
import re
import subprocess
import tempfile

PIN = "0e185cbf3ae283b6fb6a6e9543079b48a2dae57a"
root = Path(__file__).resolve().parents[1]
scratch = Path(tempfile.mkdtemp(prefix="s349-grant-tests-", dir=os.environ["RUNNER_TEMP"]))
subprocess.run(["git", "init", "-q", str(scratch)], check=True)
subprocess.run(["git", "-C", str(scratch), "fetch", "--depth=1", "https://github.com/calex794/tor.git", PIN], check=True)
subprocess.run(["git", "-C", str(scratch), "checkout", "--detach", "FETCH_HEAD"], check=True)
assert subprocess.check_output(["git", "-C", str(scratch), "rev-parse", "HEAD"], text=True).strip() == PIN
command = ["cargo", "test", "--locked", "-p", "tor-web", "--features", "s3-free", "--lib", "free_reader_http::verified_grant::tests", "--", "--nocapture"]
environment = dict(os.environ, CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="2", CARGO_PROFILE_DEV_DEBUG="0", CARGO_PROFILE_TEST_DEBUG="0", CARGO_TERM_COLOR="never")
for key in list(environment):
    if key.startswith("TOR_") or key in ("DATABASE_URL", "TEST_DATABASE_URL", "MIGRATOR_URL"):
        environment.pop(key)
# This repository does not commit Cargo.lock. Resolve once in the disposable
# checkout, then test locked and record the generated dependency fingerprint.
subprocess.run(["cargo", "generate-lockfile"], cwd=scratch, env=environment, check=True, timeout=120)
lock_sha = hashlib.sha256((scratch / "Cargo.lock").read_bytes()).hexdigest()
output = root / "tools/s349-output"
output.mkdir(parents=True, exist_ok=True)
try:
    result = subprocess.run(command, cwd=scratch, env=environment, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=900)
    log = result.stdout
    report = {"source_commit": PIN, "dependency_lock_sha256": lock_sha, "command": command, "returncode": result.returncode, "six_tests_passed": bool(re.search(r"test result: ok\. 6 passed; 0 failed; 0 ignored;", log))}
except subprocess.TimeoutExpired as error:
    log = error.stdout or ""
    if isinstance(log, bytes):
        log = log.decode("utf-8", "replace")
    report = {"source_commit": PIN, "dependency_lock_sha256": lock_sha, "command": command, "returncode": None, "six_tests_passed": False, "error": "build_timeout"}
(output / "grant-unit-tests.log").write_text(log)
(output / "grant-unit-tests.json").write_text(json.dumps(report, indent=2) + "\n")
print(log[-12000:], flush=True)
print(json.dumps(report), flush=True)
subprocess.run(["git", "diff", "--exit-code", "HEAD"], cwd=scratch, check=True)
