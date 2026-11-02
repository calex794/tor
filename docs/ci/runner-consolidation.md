# One automatic CI planner; all suite coverage retained

Automatic PR and main changes run **TOR integration CI** and **Repository rules**.
TOR integration CI calls `.github/workflows/ci-selection.yml` once, then runs
the same fourteen suite job bodies (with the same runtimes, PostgreSQL services,
S3/MinIO, browsers, fail-open selector gates, and upload artifacts) as before.
On main, all fourteen suites run regardless of changes. On pull requests, the
conservative selector may skip suites only when safe. Repo rules are unconditional.

The original fourteen suite workflows remain available through **manual**
`workflow_dispatch` for one-off investigations. They no longer trigger from PR
or main pushes. The architecture test compares their job bodies exactly with
the corresponding job in the automatic combined workflow. Any change to one
must update the other in the same PR.

The exact-main release audit now requires two successful push workflow runs:
Repository rules (job `rules`) and TOR integration CI (one successful selector
and **all fourteen** successful suite jobs). It rejects unknown, missing, skipped,
pending, duplicated or failed jobs, stale SHAs and incomplete API pagination.
The report schema advances to `tor.release_ci_evidence.v2`; CI success remains
**not** permission to deploy. Manual dispatch workflows are not main evidence.

**Required-check migration:** Job check names change when suite jobs move under
the TOR integration CI workflow. An administrator must inspect configured
GitHub branch protection / rulesets and replace any old required contexts
before merging this change. The available connector does not have access to
the restricted branch-protection endpoint; a green draft PR is not sufficient
evidence of that migration.

**Metrics:** Before and after workflows differ in run and job names. Compare
per-SHA selector runner count (14 to 1 expected), total runner wall-clock time,
and queue depth, but do not claim an exact matched-workflow speedup from the
`tools/ci_metrics.py` comparison when check identities have changed.

**Stable required check:** The combined workflow runs a single `ci-required`
job after the selector and all fourteen suite jobs. On PRs, it verifies that
every selected suite succeeded and every skipped suite was genuinely unselected.
If the selector fails, the gate requires the full suite pack to succeed.
On main and manual combined runs, all fourteen suites must succeed. The release
audit also requires `ci-required` to be successful on the exact main SHA.

For branch protection, the administrator should require the actual checks
`rules` and `ci-required` (confirm their observed names in GitHub). Avoid
requiring individual suite jobs that legitimately skip on narrow PR changes.
This code change does not configure protection; #289 still needs admin action.
