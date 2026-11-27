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

## Lockfile bootstrap and steady state

At introduction TOR has no tracked Cargo.lock. The first run generates one
in its disposable runner and publishes `rustsec-lockfile` before installing
the scanners. This is a baseline, not proof of the deployed binary's dependency
versions. The bootstrap resolved and committed an audited candidate lockfile. Subsequent
runs require the committed file from the outset: missing or stale lockfiles
fail closed. `cargo metadata --locked --all-features` verifies the manifests
against the snapshot, and byte comparison plus `git diff` detect changes.
Update the lockfile explicitly when dependencies change. The manual production
packaging (both v1 and v2) and FREE S3 candidate workflows require a tracked,
nonempty, unchanged Cargo.lock and locked builds for every binary. The shared
production web helper retains its exact reviewed complete S3 capability profile;
compiled capabilities are not permission to activate a provider. The FREE
candidate remains build-only. Both workflows retain the lockfile SHA-256 as
build evidence before signing or transfer; no server manifest or runtime policy
change. A locked build is not a fresh advisory scan or deployment proof.

The independent audit is registered in the workflow-wiring inventory. It does
not replace integration suites or add a main-push/deployment workflow.

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

## Candidate evidence (before full integration)

- Baseline: 15 known RustSec advisories and the Chrono unmaintained warning.
- Candidate audit run 45475500855: 0 advisories, 0 warnings, all workspace
  targets/features compile; 150 selected tests passed (6 ignored).
- Candidate lockfile SHA-256: `7e677aa3da7d4b2a7c85f3c0af88cf4af8a49532231cccdc04b5bc2561d9ff47`.
- Exact local dependency versions removed 11 wildcard diagnostics in run
  45478315026. The remaining 21 license diagnostics block policy acceptance;
  no new license grants or advisory exemptions have been made.
- Full integration tests and final combined-head audit remain mandatory.

## Owner-approved source ownership policy

- TOR's original code is not granted a general open-source license. Remove the
  misleading unused workspace MIT declaration; inherit the root ownership notice
  and `publish = false` in every workspace package. This does not revoke existing
  valid grants or change repository visibility.
- Cargo-deny's private-license setting applies only to unpublished workspace
  packages, never external registries. Advisory, source and ban checks retain
  their original fail-closed policy. No advisory exceptions.
- Permit CDLA-Permissive-2.0 only for the exact reviewed `webpki-roots 1.0.12`.
  Preserve the agreement text and review upstream notices before distribution.
- Acceptance: unchanged lockfile, no publishable workspace packages, clean real
  cargo-audit and cargo-deny, passing architecture and required integration CI
  on the final head. Public/private GitHub visibility is a separate decision.
