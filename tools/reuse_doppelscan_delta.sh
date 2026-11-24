#!/usr/bin/env bash
# REUSE-10: compare two immutable Rust snapshots; findings are advisory.
set -euo pipefail
test "${GITHUB_ACTIONS:-}" = true
: "${RUNNER_TEMP:?}" "${SCAN_SHA:?}" "${BASE_SHA:?}" "${AUDIT_EVENT:?}"
[[ "$BASE_SHA" =~ ^[0-9a-f]{40}$ && "$SCAN_SHA" =~ ^[0-9a-f]{40}$ ]]
test "$(git rev-parse HEAD)" = "$SCAN_SHA"
test "$(git rev-parse --verify "$BASE_SHA^{commit}")" = "$BASE_SHA"
git merge-base --is-ancestor "$BASE_SHA" "$SCAN_SHA"
if [[ "$AUDIT_EVENT" == pull_request ]]; then
  test "$(git rev-parse "$SCAN_SHA^1")" = "$BASE_SHA"
else
  test "$AUDIT_EVENT" = workflow_dispatch
fi
report="$RUNNER_TEMP/reuse-report"
head="$RUNNER_TEMP/reuse-source"
base=$(mktemp -d "$RUNNER_TEMP/reuse-base.XXXXXX")
smoke=$(mktemp -d "$RUNNER_TEMP/reuse-smoke.XXXXXX")
trap 'rm -rf -- "$base" "$smoke"' EXIT
test -s "$report/input.sha256"
test -x "$RUNNER_TEMP/reuse-bin/bin/doppelscan"
git archive "$BASE_SHA" crates | tar -x -C "$base"
find "$base" -type f ! -name '*.rs' -delete
find "$base" -type l -delete
(cd "$base"; find . -type f -name '*.rs' -print0 | sort -z | xargs -0 -r sha256sum) > "$report/base-input.sha256"
test -s "$report/base-input.sha256"
(cd "$head"; sha256sum -c --quiet "$report/input.sha256")
mkdir -p "$report/src"
printf 'base=%s\nhead=%s\n' "$BASE_SHA" "$SCAN_SHA" > "$report/delta-input.txt"

# Both snapshots have the same in-container path. Baseline paths are relative
# to /work, and the scanner never gets repository history or credentials.
scan() {
  local source="$1"
  shift
  timeout --kill-after=10s 300s docker run --rm --network none --read-only \
    --cap-drop ALL --security-opt no-new-privileges --pids-limit 256 \
    --cpus 2 --memory 4g --user "$(id -u):$(id -g)" \
    --tmpfs /tmp:rw,nosuid,nodev,size=128m -e HOME=/tmp \
    -v "$report:/work:rw" -v "$source:/work/src:ro" \
    -v "$RUNNER_TEMP/reuse-bin/bin:/tools:ro" -w /work/src ubuntu:24.04 \
    /tools/doppelscan "$@"
}
# Prove the pinned scanner distinguishes all three outcomes before relying
# on its real report. Synthetic files are outside both repository snapshots.
mkdir -p "$smoke/base" "$smoke/head"
cat > "$smoke/base/a.rs" <<'RS'
pub fn weighted_total(values: &[i64], limit: i64) -> Option<i64> {
    let mut total = 0_i64;
    let mut count = 0_i64;
    for value in values.iter().copied() {
        if value < 0 || value > limit {
            continue;
        }
        let weight = value.checked_mul(3)?.checked_add(7)?;
        total = total.checked_add(weight)?;
        count = count.checked_add(1)?;
        if total > limit.checked_mul(10)? {
            return None;
        }
    }
    match count {
        0 => Some(0),
        _ => total.checked_div(count),
    }
}
RS
cp "$smoke/base/a.rs" "$smoke/base/b.rs"
cp "$smoke/base/"*.rs "$smoke/head/"
cp "$smoke/base/a.rs" "$smoke/head/c.rs"
scan "$smoke/base" baseline /work/src --output /work/smoke-baseline.json --no-config \
  --only rust --min-tokens 50 > "$report/smoke-baseline.stdout" 2> "$report/smoke-baseline.stderr"
scan "$smoke/base" check /work/src --baseline /work/smoke-baseline.json --no-config \
  --only rust --min-tokens 50 --format json > "$report/smoke-unchanged.json" 2> "$report/smoke-unchanged.stderr"
added_status=0
scan "$smoke/head" check /work/src --baseline /work/smoke-baseline.json --no-config \
  --only rust --min-tokens 50 --format json > "$report/smoke-added.json" 2> "$report/smoke-added.stderr" || added_status=$?
test "$added_status" = 1
printf 'not valid json\n' > "$report/smoke-bad-baseline.json"
bad_status=0
scan "$smoke/base" check /work/src --baseline /work/smoke-bad-baseline.json --no-config \
  --only rust --min-tokens 50 --format json > "$report/smoke-bad.stdout" 2> "$report/smoke-bad.stderr" || bad_status=$?
test "$bad_status" = 2
python3 - "$report" <<'PY'
import json
from pathlib import Path
import sys

root = Path(sys.argv[1])
baseline = json.loads((root / "smoke-baseline.json").read_text())
unchanged = json.loads((root / "smoke-unchanged.json").read_text())
added = json.loads((root / "smoke-added.json").read_text())
assert baseline["groups"], "fixture did not produce a baseline clone"
assert unchanged["summary"]["files"] == 2
assert not unchanged["new"] and not unchanged["smaller"]
assert added["summary"]["files"] == 3
assert added["new"], "third copy did not produce a new candidate"
result = {"unchanged": 0, "third_copy": 1, "invalid_baseline": 2}
(root / "delta-smoke.json").write_text(json.dumps(result) + "\n")
print("REUSE-10 scanner smoke: " + json.dumps(result, sort_keys=True))
PY

scan "$base" baseline /work/src --output /work/base.json --no-config \
  --only rust --min-tokens 50 > "$report/delta-baseline.stdout" 2> "$report/delta-baseline.stderr"
test -s "$report/base.json"

status=0
scan "$head" check /work/src --baseline /work/base.json --no-config \
  --only rust --min-tokens 50 --format json > "$report/delta.json" 2> "$report/delta.stderr" || status=$?
printf '%s\n' "$status" > "$report/delta-exit.txt"
# Exit 1 means candidates, NOT an infrastructure failure or merge rejection.
# Every other nonzero code is an incomplete comparison, never "no duplicates".
case "$status" in
  0|1) ;;
  *) echo "::error::Doppelscan delta comparison incomplete: exit $status"; exit 1 ;;
esac
python3 - "$report" "$BASE_SHA" "$SCAN_SHA" "$status" <<'PY'
import json
from pathlib import Path
import sys

root = Path(sys.argv[1])
base_sha, head_sha = sys.argv[2:4]
status = int(sys.argv[4])
data = json.loads((root / "delta.json").read_text())
baseline = json.loads((root / "base.json").read_text())
assert isinstance(baseline["groups"], list)
assert isinstance(data["new"], list)
assert isinstance(data["smaller"], list)
assert data["against"] == "/work/base.json"
assert (status == 1) == bool(data["new"])
expected = len((root / "input.sha256").read_text().splitlines())
scanned = data["summary"]["files"]
assert 0 < scanned <= expected
result = {
    "base": base_sha,
    "head": head_sha,
    "baseline_groups": len(baseline["groups"]),
    "new_groups": len(data["new"]),
    "smaller_groups": len(data["smaller"]),
    "resolved_groups": data["resolved"],
    "input_files": expected,
    "scanned_files": scanned,
    "scanner_exit": status,
    "advisory": True,
}
(root / "delta-summary.json").write_text(json.dumps(result, indent=2) + "\n")
print("REUSE-10 advisory comparison: " + json.dumps(result, sort_keys=True))
if scanned != expected:
    print("::warning::Scanner excluded some head files")
if status == 1:
    print("::warning::New clone candidates require human review; no automatic rejection")
PY
