"""Prepare immutable reviewed integration objects only; never update a ref or run tests."""
from pathlib import Path
import base64
import json
import os
import re
import subprocess
import tempfile
import urllib.request

BASE = '2c660dd75aec763e78ab477617ec0bb0d27959f4'
MAIN = '9e6a43b4f9f3ab42bd380c0b9cc96029e4b39709'
EXPECTED_TREE = 'e271448fe1d7cd7ac61059ec43b41e3f80623893'
REPO = 'calex794/tor'
PREP_REF = 'refs/heads/integration/rec13-main-9e6a43b-objects'
assert os.environ['GITHUB_REPOSITORY'] == REPO
assert os.environ['GITHUB_REF'] == PREP_REF
assert os.environ['GITHUB_EVENT_NAME'] == 'push'
checkout = Path.cwd()
root = Path(tempfile.mkdtemp(prefix='rec13-integration-')) / 'tree'

def git(*args, cwd=None):
    return subprocess.check_output(['git', *args], cwd=cwd or root, text=True).strip()

def read(ref, path):
    return subprocess.check_output(['git', 'show', f'{ref}:{path}'], cwd=root, text=True)

git('worktree', 'add', '--detach', str(root), BASE, cwd=checkout)
git('config', 'user.name', 'TOR integration preparation')
git('config', 'user.email', 'tor-integration@users.noreply.github.com')
merge = subprocess.run(['git', 'merge', '--no-commit', '--no-ff', MAIN], cwd=root, check=False)
assert merge.returncode in (0, 1)
assert git('rev-parse', 'MERGE_HEAD') == MAIN
route = 'crates/route_intelligence/src/domain/routes.rs'
ours, theirs = read(BASE, route), read(MAIN, route)
extra = [s for s in ours.splitlines() if 'RouteRegistration::new(' in s and 'recommendation' in s]
assert len(extra) == 16
extra = [re.sub(r'// \d+$', f'// {123+i}', s) for i, s in enumerate(extra)]
assert '[RouteRegistration; 123]' in theirs
(root / route).write_text(theirs.replace('[RouteRegistration; 123]', '[RouteRegistration; 139]').replace('\n];', '\n'+'\n'.join(extra)+'\n];'))
path = 'crates/route_intelligence/src/domain/routes/classifier.rs'
ours, theirs = read(BASE, path), read(MAIN, path)
extra = [s for s in ours.splitlines() if 'recommendation' in s and '=> Some(' in s]
assert len(extra) == 16
extra = [re.sub(r'Some\((\d+)\)', lambda m: f'Some({int(m[1])+7})', s) for s in extra]
(root / path).write_text(theirs.replace('        _ => None,', '\n'.join(extra)+'\n        _ => None,'))
path = 'crates/route_intelligence/src/domain/tests.rs'
(root / path).write_text(read(MAIN, path).replace('unique.len(), 123', 'unique.len(), 139'))
path = 'tests/architecture/test_ril_exposure.py'
(root / path).write_text(read(MAIN, path).replace('report["route_bindings"], 125', 'report["route_bindings"], 141').replace('report["route_pairs"], 123', 'report["route_pairs"], 139'))
path = 'docs/contracts/ril-route-guards-v1.json'
a, b = json.loads(read(BASE, path)), json.loads(read(MAIN, path))
key = lambda r: (r['method'], r['template'], r['source'], r['handler'])
rows = {key(r): r for r in b['routes']}
for row in a['routes']:
    if key(row) in rows:
        assert rows[key(row)] == row
    else:
        rows[key(row)] = row
b['routes'] = sorted(rows.values(), key=key)
assert len(b['routes']) == 141
(root / path).write_text(json.dumps(b, ensure_ascii=False, indent=2)+'\n')
for path in ['tools/rec13-hardening.applied', 'tools/rec13-integration.applied']:
    (root / path).unlink()
p = root / 'tools/ci_test_map.py'
s = p.read_text()
needle = 'PERFORMANCE_WORKFLOWS = {"tor-http-load.yml"}'
assert s.count(needle) == 1
s = s.replace(needle, needle+'\n# Additional exact-head PG/HTTP/Chromium acceptance; never a deployment gate.\nACCEPTANCE_WORKFLOWS = {"rec-feedback-acceptance.yml"}')
p.write_text(s)
p = root / 'tests/architecture/test_ci_workflow_wiring.py'
s = p.read_text().replace('    CI_REQUIRED_JOB, CI_WORKFLOW, MANUAL_WORKFLOWS, PERFORMANCE_WORKFLOWS, SUITES,', '    ACCEPTANCE_WORKFLOWS, CI_REQUIRED_JOB, CI_WORKFLOW, MANUAL_WORKFLOWS, PERFORMANCE_WORKFLOWS, SUITES,').replace('| MANUAL_WORKFLOWS | PERFORMANCE_WORKFLOWS)', '| MANUAL_WORKFLOWS | PERFORMANCE_WORKFLOWS | ACCEPTANCE_WORKFLOWS)')
needle = '    def test_no_unknown_workflows_or_missing_harness(self):'
assert s.count(needle) == 1
s = s.replace(needle, '''    def test_rec13_acceptance_tests_committed_source_without_rewriting(self):
        self.assertEqual(ACCEPTANCE_WORKFLOWS, {"rec-feedback-acceptance.yml"})
        text = (WORKFLOWS / "rec-feedback-acceptance.yml").read_text()
        self.assertIn("contents: read", text)
        self.assertNotIn("contents: write", text)
        self.assertNotIn("pull_request_target", text)
        self.assertIn("ref: ${{ github.event.pull_request.head.sha || github.sha }}", text)
        self.assertIn("persist-credentials: false", text)
        self.assertIn("git diff --exit-code", text)
        for command in (
            "cargo fmt --all -- --check",
            "cargo test -p tor-recommendations --test feedback_pg -- --ignored",
            "cargo test -p tor-recommendations --test generations_pg -- --ignored",
            "cargo test -p tor-web --test recommendation_feedback_http_pg -- --ignored",
            "bash tools/rec13_browser_acceptance.sh",
            "cargo clippy --workspace --all-targets -- -D warnings",
        ):
            self.assertIn(command, text)
        for forbidden in ("rec13_integrate.py", "rec13_harden.py", "rec13_publish_tree.py"):
            self.assertNotIn(forbidden, text)

'''+needle)
p.write_text(s)
git('add', '-u')
assert not git('diff', '--name-only', '--diff-filter=U')
git('diff', '--cached', '--check')
tree = git('write-tree')
assert tree == EXPECTED_TREE, (tree, EXPECTED_TREE)

def entries(ref):
    data = subprocess.check_output(['git', 'ls-tree', '-r', '-z', ref], cwd=root).decode()
    result = {}
    for row in data.split('\0'):
        if not row:
            continue
        meta, path = row.split('\t', 1)
        mode, kind, sha = meta.split()
        result[path] = (mode, kind, sha)
    return result

base, main, integrated = entries(BASE), entries(MAIN), entries(tree)
known = {v[2] for mapping in (base, main) for v in mapping.values()}
token = os.environ['OBJECT_WRITE_TOKEN']

def post(endpoint, payload):
    # Explicitly disallow commits, refs, contents, branches and deployments.
    assert endpoint in ('git/blobs', 'git/trees')
    request = urllib.request.Request(
        f'https://api.github.com/repos/{REPO}/{endpoint}',
        data=json.dumps(payload).encode(), method='POST',
        headers={'Authorization': f'Bearer {token}', 'Accept': 'application/vnd.github+json',
                 'X-GitHub-Api-Version': '2022-11-28', 'Content-Type': 'application/json'})
    with urllib.request.urlopen(request, timeout=60) as response:
        return json.load(response)

elements = []
for path in sorted(set(base) | set(integrated)):
    if base.get(path) == integrated.get(path):
        continue
    if path not in integrated:
        elements.append({'path': path, 'mode': base[path][0], 'type': 'blob', 'sha': None})
        continue
    mode, kind, sha = integrated[path]
    assert kind == 'blob' and mode in ('100644', '100755')
    assert not path.endswith('.pyc') and '__pycache__' not in path
    if sha not in known:
        raw = subprocess.check_output(['git', 'cat-file', 'blob', sha], cwd=root)
        result = post('git/blobs', {'encoding': 'base64', 'content': base64.b64encode(raw).decode()})
        assert result['sha'] == sha
        known.add(sha)
    elements.append({'path': path, 'mode': mode, 'type': kind, 'sha': sha})
remote = post('git/trees', {'base_tree': git('rev-parse', f'{BASE}^{{tree}}'), 'tree': elements})
assert remote['sha'] == EXPECTED_TREE
manifest = {'repository': REPO, 'base': BASE, 'main': MAIN, 'tree': EXPECTED_TREE,
            'purpose': 'Immutable object preparation only; no tests and no ref writes', 'changes': elements}
output = Path(os.environ['RUNNER_TEMP'])/'rec13-prepared-tree.json'
output.write_text(json.dumps(manifest, indent=2)+'\n')
print(f'Prepared {len(elements)} entries; immutable tree {EXPECTED_TREE}; no ref changed')
