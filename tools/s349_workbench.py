"""Prepare a precise CI ordering fix; no target branch or production writes."""
from pathlib import Path
import hashlib
import json
import os
import shutil
import subprocess
import tempfile

PIN = "d815510d24bdf192209f515249a8d54ccc68c594"
root = Path(__file__).resolve().parents[1]
scratch = Path(tempfile.mkdtemp(prefix="s349-test-order-", dir=os.environ["RUNNER_TEMP"]))
subprocess.run(["git", "init", "-q", str(scratch)], check=True)
subprocess.run(["git", "-C", str(scratch), "fetch", "--depth=1", "https://github.com/calex794/tor.git", PIN], check=True)
subprocess.run(["git", "-C", str(scratch), "checkout", "--detach", "FETCH_HEAD"], check=True)
assert subprocess.check_output(["git", "-C", str(scratch), "rev-parse", "HEAD"], text=True).strip() == PIN
command = "          TOR_FREE_S3_PUBLIC_ORIGIN=http://127.0.0.1:9000 cargo test -p tor-web --features s3-free --test free_author_s3_pg -- --ignored --test-threads=1\n"
last_paid = "          node tools/paid_reader_private_media_browser.cjs\n"
allowed = {".github/workflows/tor-paid-grant-e2e.yml", ".github/workflows/tor-integration.yml"}
for path in sorted(allowed):
    target = scratch / path
    source = target.read_text()
    assert source.count(command) == 1, path
    assert source.count(last_paid) == 1, path
    reordered = source.replace(command, "").replace(last_paid, last_paid + command)
    assert reordered.replace(command, "") == source.replace(command, ""), path
    assert reordered.index(last_paid) < reordered.index(command), path
    target.write_text(reordered)
subprocess.run(["git", "diff", "--check"], cwd=scratch, check=True)
checks = {}
for pattern in ("test_ci_workflow_wiring.py", "test_ci_workspace_dedup.py", "test_ril_exposure.py"):
    result = subprocess.run(["python3", "-m", "unittest", "discover", "-s", "tests/architecture", "-p", pattern], cwd=scratch)
    checks[pattern] = result.returncode
assert all(code == 0 for code in checks.values()), checks
paths = subprocess.check_output(["git", "diff", "--name-only"], cwd=scratch, text=True).splitlines()
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
    before = subprocess.check_output(["git", "rev-parse", "HEAD:" + path], cwd=scratch, text=True).strip()
    manifest["files"].append({"path": path, "before": before, "sha": blob, "bytes": len(data)})
(output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
print(json.dumps(manifest), flush=True)
