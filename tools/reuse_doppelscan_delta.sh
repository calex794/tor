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
trap 'rm -rf -- "$base"' EXIT
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
