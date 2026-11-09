"""Prepare explicit test files; no target branch writes or production access."""
from pathlib import Path
import hashlib
import json
import os
import shutil
import subprocess
import tempfile

PIN = "50259c4e38876b722cc257468564a503b20af99f"
root = Path(__file__).resolve().parents[1]
scratch = Path(tempfile.mkdtemp(prefix="s349-e2e-", dir=os.environ["RUNNER_TEMP"]))
subprocess.run(["git", "init", "-q", str(scratch)], check=True)
subprocess.run(["git", "-C", str(scratch), "fetch", "--depth=1", "https://github.com/calex794/tor.git", PIN], check=True)
subprocess.run(["git", "-C", str(scratch), "checkout", "--detach", "FETCH_HEAD"], check=True)
assert subprocess.check_output(["git", "-C", str(scratch), "rev-parse", "HEAD"], text=True).strip() == PIN
for source, destination in (
    ("free_author_s3_pg.rs", "crates/web/tests/free_author_s3_pg.rs"),
    ("browser.cjs", "crates/web/tests/free_author_s3_pg/browser.cjs"),
):
    target = scratch / destination
    assert not target.exists(), destination
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes((root / "tools/s349-input" / source).read_bytes())
needle = "          TOR_PAID_CHECKOUT_ENABLED=1 TOR_LAYER_READER_ENABLED=1 cargo test -p tor-web --features s3-paid --test paid_grants_s3_pg -- --ignored --test-threads=1"
addition = "          TOR_FREE_S3_PUBLIC_ORIGIN=http://127.0.0.1:9000 cargo test -p tor-web --features s3-free --test free_author_s3_pg -- --ignored --test-threads=1"
for name in ("tor-paid-grant-e2e.yml", "tor-integration.yml"):
    target = scratch / ".github/workflows" / name
    source = target.read_text()
    assert source.count(needle) == 1, name
    target.write_text(source.replace(needle, needle + "\n" + addition))
subprocess.run(["cargo", "fmt", "--all"], cwd=scratch, check=True)
subprocess.run(["git", "diff", "--check"], cwd=scratch, check=True)
subprocess.run(["node", "--check", "crates/web/tests/free_author_s3_pg/browser.cjs"], cwd=scratch, check=True)
checks = {}
for pattern in ("test_ci_workflow_wiring.py", "test_ci_workspace_dedup.py", "test_ril_exposure.py", "test_s020_age_editor_survival.py"):
    result = subprocess.run(["python3", "-m", "unittest", "discover", "-s", "tests/architecture", "-p", pattern], cwd=scratch)
    checks[pattern] = result.returncode
paths = subprocess.check_output(["git", "diff", "--name-only"], cwd=scratch, text=True).splitlines()
paths += subprocess.check_output(["git", "ls-files", "--others", "--exclude-standard"], cwd=scratch, text=True).splitlines()
allowed = {"crates/web/tests/free_author_s3_pg.rs", "crates/web/tests/free_author_s3_pg/browser.cjs", ".github/workflows/tor-paid-grant-e2e.yml", ".github/workflows/tor-integration.yml"}
assert set(paths) == allowed, paths
output = root / "tools/s349-output"
if output.exists():
    shutil.rmtree(output)
manifest = {"source_commit": PIN, "checks": checks, "files": []}
for path in sorted(paths):
    data = (scratch / path).read_bytes()
    target = output / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(data)
    blob = hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()
    previous = subprocess.run(["git", "rev-parse", "HEAD:" + path], cwd=scratch, text=True, capture_output=True)
    manifest["files"].append({"path": path, "before": previous.stdout.strip() if previous.returncode == 0 else None, "sha": blob, "bytes": len(data)})
(output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
print(json.dumps(manifest), flush=True)
