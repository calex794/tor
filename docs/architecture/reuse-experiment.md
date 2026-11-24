# REUSE-01B: comparative analyzer experiment

Historical comparison; not a deployment or required quality gate. The three-tool experiment is superseded by the selected single-tool advisory audit described below.

Fixed input: a527bf2e3c05ce577e7a4363296d3c0a5978c69b, tracked regular Rust files under crates, including tests. Each job records SHA-256 manifest. Scanner-internal exclusions may differ, especially jscpd's AST test exclusion, and must be disclosed when interpreting counts. No source is mounted when installing tools. Scans run offline in containers with no repository credentials, read-only source, separate report directory, 2 CPUs, 4 GiB memory, and five minutes per pass. No production secrets, database, network scan, or runtime changes.

- cargo-dupes 0.2.1, Cargo locked installation, AST threshold .8, minimum 20 nodes / 8 lines.
- doppelscan 0.9.0, SHA-256-pinned release, strict/renamed/near, threshold .8 for near, minimum 50 tokens.
- jscpd 5.4.3, SHA-256-pinned release, token pass and normalized token + structural pass, threshold .8, minimum 20 AST nodes / 8 lines / 50 tokens.

The matching algorithms and granularity are different; raw clone totals and percentages are not directly comparable. Exit code, wall seconds (including container overhead), cgroup memory peak when available, stdout/stderr, OOM status and JSON reports are retained for seven days as private workflow artifacts. Failed or timed-out runs are not valid zero-clone results.

Review at least 20 high-ranked candidates from each successful structural pass, label actionable / intentional / unclear, check known duplicated HTTP guards and reader helper code, and record observed scope, false positives and operational cost. No automatic refactoring, semantic model, baseline acceptance, new required check, or merging before evidence review.

## First run (45421474646)

Input manifest: 1,436 Rust files, SHA-256 90dafd7f91f4d610d952e6e5af5733e12bad89d3a85a2493e21e13045840752d, identical for the two completed scans. Doppelscan strict: 624 groups, 10,890 duplicated lines, 1,283 ms, 146,788,352 peak cgroup bytes; normalized: 2,757 groups, 67,191 duplicated lines, 1,334 ms, 155,193,344 bytes; near: 145 groups, 1,397 ms, 138,149,888 bytes. Jscpd exact: 1,901 pairs, 24,911 duplicated lines, 1,393 analyzed files, 336,718 reported lines, 7.398% by its metric, 864.988 ms, 463,433,728 bytes. These counts overlap and use different definitions. Doppelscan's highest-ranked normalized candidate includes shared HTTP guard logic; its near results include repeated PostgreSQL fixtures, not all actionable. No zero-clone claims for failed scans.

Root causes: cargo-dupes requires Rust >=1.93, and jscpd interprets a detached `consistent` as a source path. Second run uses Rust 1.93.0 and `--ignore-identifiers=consistent`. Register the isolated experiment explicitly in the existing workflow inventory and test its read-only/advisory boundaries; do not change the required integration workflow. This was the historical experiment, not a permanent gate.

## Selection — 2026-11-24

Owner approved Doppelscan as the primary duplication analyzer. The comparative workflow is replaced by `.github/workflows/reuse-audit.yml`: one pinned Doppelscan 0.9.0 job, no Cargo-dupes or Jscpd installs or scans. See [repeat run](https://github.com/calex794/tor/actions/runs/45422324358). On 1,436 input Rust files, all three Doppelscan modes passed: strict 624 groups / 1.275 s; normalized 2,757 groups / 1.338 s; near 145 groups / 1.402 s. Peak cgroup memory respectively 147075072, 155193344 and 137830400 bytes. All three competing tools completed in that repeat run; the choice is operational, not a measured precision/recall ranking.

## Running the selected audit

- GitHub Actions: **Doppelscan audit (advisory)**, manual dispatch after this workflow is merged; use the desired branch/ref. A path-filtered same-repository PR run verifies changes to the workflow or this document.
- The workflow scans the actual immutable checkout SHA, not the old experiment snapshot, and records the input manifest, version, strict/normalized/near results, summary JSON, wall time, memory and failure information. Only tracked Rust files under crates are scanned at this stage. Other languages are a separate scope.
- Doppelscan 0.9.0 Linux x86_64 release is verified against the fixed SHA-256. It runs offline in an unprivileged container with read-only source, no checkout credentials and resource limits. Reports can include source fragments; artifacts are available to authorized repository users for seven days.
- For local analysis with the same verified release, run `doppelscan --only rust --min-tokens 50 --strict --format json --limit 0`, then remove `--strict` for normalized clones, and run `doppelscan near --only rust --min-tokens 50 --similarity 0.8 --format json --limit 0`. Run against a clean committed Rust-only snapshot to reproduce the CI scope.
- Before adding a helper, use `doppelscan similar --lang rust < snippet.rs` or `doppelscan at path/to/file.rs:10-30` to locate existing candidates. A clone is not proof of identical permissions or business semantics.

No automatic blocking on clone counts, no baseline acceptance, no source rewrite. REUSE-10 will review the baseline and introduce new-code checks separately. The main required integration workflow and all existing mandatory checks are unchanged. The independent REUSE-01 text inventory remains useful for SQL, JS, infrastructure, and a reproducible reference metric.
