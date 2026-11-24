# I-07 / S062 — Rust dependency security

The independent Rust dependency security workflow runs on dependency-related
pull requests, daily at 04:17 UTC, and by manual dispatch. It does not add a
main-push workflow or change the existing release registry, mandatory CI suites,
deployment scripts, production or staging.

## Checks

- `cargo audit` 0.23.0: current RustSec advisories against the exact lockfile,
  including transitive packages. Findings and tool failures are not suppressed.
- `cargo deny` 0.20.2: advisories, yanked and unmaintained/unsound packages,
  duplicate versions (warnings), wildcard requirements, dependency origins,
  and licenses across workspace features. The permissive-license allowlist is
  a starting policy, not a legal clearance; review changes explicitly.
- No advisory exemptions or unknown git/registry sources are allowed.
- The job uses a read-only token and normal pull_request events; no secrets,
  build scripts of application dependencies, database, or SSH are required.
- Both tool versions are pinned; their own Cargo.lock is used at installation.
  The advisory DB is fetched each run. The reports and lockfile are retained
  for 14 days. Duplicate versions alone are not a vulnerability.

## Initial lockfile bootstrap

At introduction TOR has no tracked Cargo.lock. The first run generates one
in its disposable runner and publishes `rustsec-lockfile` before installing
the scanners. This is a baseline, not proof of the deployed binary's dependency
versions. The job deliberately fails the tracked-lock assertion until the
exact generated Cargo.lock is reviewed and committed. Review the snapshot
SHA and checksum, commit that lockfile without manual edits, then rerun all
mandatory tests and both scanners on the final PR SHA. After that, update
the lockfile explicitly when dependencies change; use locked builds for
releases once their build contract is updated and verified.

## Triage

Inspect the `rustsec-report` artifact and job log. For each advisory record
the identifier, affected crate/version, dependency path, affected features,
and fixed version or upstream blocker. Prefer the smallest compatible update;
never conceal findings to obtain green CI. Review any incompatible dependency
or license change before merging. A failing scanner installation/network
operation is not a clean audit. Scheduled scans should be checked and triaged
even when application code did not change.

The initial PR must remain unmerged until the lockfile and baseline findings
are resolved or explicitly reviewed. Required branch status and release-gate
changes are a separate, tested integration step; no production release is
authorized by this scan alone.
