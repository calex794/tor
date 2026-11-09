"""Isolated source preparation; never writes the target branch or production."""
from pathlib import Path
import hashlib
import json
import os
import re
import shutil
import subprocess
import tempfile

PIN = "e863ff7c175f2bb291c7d6ab061d56f6915c5980"
root = Path(__file__).resolve().parents[1]
scratch = Path(tempfile.mkdtemp(prefix="s349-reviewed-", dir=os.environ["RUNNER_TEMP"]))
subprocess.run(["git", "init", "-q", str(scratch)], check=True)
subprocess.run(["git", "-C", str(scratch), "fetch", "--depth=1", "https://github.com/calex794/tor.git", PIN], check=True)
subprocess.run(["git", "-C", str(scratch), "checkout", "--detach", "FETCH_HEAD"], check=True)
assert subprocess.check_output(["git", "-C", str(scratch), "rev-parse", "HEAD"], text=True).strip() == PIN

def change(path, old, new, expected=1):
    target = scratch / path
    source = target.read_text()
    assert source.count(old) == expected, (path, source.count(old), expected)
    target.write_text(source.replace(old, new))

# Preserve exactly the same reviewed policy records, restoring canonical layout.
p = scratch / "docs/contracts/ril-route-guards-v1.json"
policy = json.loads(p.read_text())
assert len(policy["routes"]) == 375
assert len({(r["method"], r["template"]) for r in policy["routes"]}) == 373
p.write_text(json.dumps(policy, ensure_ascii=False, indent=2) + "\n")

# Manual and automatic jobs must compile the same feature matrix.
for name in ("rust-demo.yml", "tor-cover-s3.yml"):
    path = ".github/workflows/" + name
    change(path, "s3-covers,s3-paid,s3-chapter-import'", "s3-covers,s3-paid,s3-chapter-import,s3-free'")
for name in ("test_ci_workflow_wiring.py", "test_ci_workspace_dedup.py"):
    path = "tests/architecture/" + name
    p = scratch / path
    old = "s3-covers,s3-paid,s3-chapter-import'"
    assert old in p.read_text(), path
    p.write_text(p.read_text().replace(old, "s3-covers,s3-paid,s3-chapter-import,s3-free'"))
change("tests/architecture/test_ril_exposure.py", 'report["route_bindings"], 372', 'report["route_bindings"], 375')
change("tests/architecture/test_ril_exposure.py", 'report["route_pairs"], 370', 'report["route_pairs"], 373')

# One explicit pair of writer capabilities avoids an eighth route argument.
path = "crates/web/src/paid_reader_router.rs"
p = scratch / path
source = p.read_text()
name = "router_with_postgres_reading_auth_paid_grants_browser_author_free"
start = source.index("pub fn " + name + "(")
end = source.index("\nfn paid_grants_router(", start)
section = source[start:end]
old = '''    author: crate::paid_author_publish::PgPaidAuthorPublisher<
        tor_storage::repositories::s3::S3PrivateStore,
    >,
    free: crate::free_author_publish::PgFreeAuthorPublisher<
        tor_storage::repositories::free_chapter_s3::S3FreeChapterStore,
    >,'''
assert section.count(old) == 1
section = section.replace(old, "    authors: S3ChapterAuthors,")
section = section.replace("    let (secure, public_origin) = origin;", "    let (author, free) = authors;\n    let (secure, public_origin) = origin;", 1)
alias = '''#[cfg(all(feature = "s3-paid", feature = "s3-free"))]
pub type S3ChapterAuthors = (
    crate::paid_author_publish::PgPaidAuthorPublisher<tor_storage::repositories::s3::S3PrivateStore>,
    crate::free_author_publish::PgFreeAuthorPublisher<tor_storage::repositories::free_chapter_s3::S3FreeChapterStore>,
);

'''
anchor = '/// Combined paid and free S3 writers, sharing one authenticated author router.\n'
assert source.count(anchor) == 1
p.write_text((source[:start] + section + source[end:]).replace(anchor, alias + anchor, 1))
p = scratch / "crates/web/src/auth_startup.rs"
source = p.read_text()
pattern = re.compile(r"(router_with_postgres_reading_auth_paid_grants_browser_author_free\([\s\S]*?covers,\n)(\s*)author,\n\s*free,")
assert len(pattern.findall(source)) == 1
p.write_text(pattern.sub(r"\1\2(author, free),", source))
subprocess.run(["cargo", "fmt", "--all"], cwd=scratch, check=True)
subprocess.run(["git", "diff", "--check"], cwd=scratch, check=True)
checks = {}
for pattern in ("test_ci_workflow_wiring.py", "test_ci_workspace_dedup.py", "test_ril_exposure.py", "test_s020_age_editor_survival.py"):
    result = subprocess.run(["python3", "-m", "unittest", "discover", "-s", "tests/architecture", "-p", pattern], cwd=scratch)
    checks[pattern] = result.returncode
allowed = {
    "docs/contracts/ril-route-guards-v1.json",
    ".github/workflows/rust-demo.yml",
    ".github/workflows/tor-cover-s3.yml",
    "tests/architecture/test_ci_workflow_wiring.py",
    "tests/architecture/test_ci_workspace_dedup.py",
    "tests/architecture/test_ril_exposure.py",
    "crates/web/src/paid_reader_router.rs",
    "crates/web/src/auth_startup.rs",
}
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
# This existing workbench's heavy service export is not needed for source fixes.
(root / "tools/s349_export_toolchain").unlink(missing_ok=True)
