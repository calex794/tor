# REUSE-01B: comparative analyzer experiment

Experimental only; not a deployment or required quality gate. Do not merge this workflow as a permanent CI check before reviewing the results.

Fixed input: a527bf2e3c05ce577e7a4363296d3c0a5978c69b, tracked regular Rust files under crates, including tests. Each job records SHA-256 manifest. Scanner-internal exclusions may differ, especially jscpd's AST test exclusion, and must be disclosed when interpreting counts. No source is mounted when installing tools. Scans run offline in containers with no repository credentials, read-only source, separate report directory, 2 CPUs, 4 GiB memory, and five minutes per pass. No production secrets, database, network scan, or runtime changes.

- cargo-dupes 0.2.1, Cargo locked installation, AST threshold .8, minimum 20 nodes / 8 lines.
- doppelscan 0.9.0, SHA-256-pinned release, strict/renamed/near, threshold .8 for near, minimum 50 tokens.
- jscpd 5.4.3, SHA-256-pinned release, token pass and normalized token + structural pass, threshold .8, minimum 20 AST nodes / 8 lines / 50 tokens.

The matching algorithms and granularity are different; raw clone totals and percentages are not directly comparable. Exit code, wall seconds (including container overhead), cgroup memory peak when available, stdout/stderr, OOM status and JSON reports are retained for seven days as private workflow artifacts. Failed or timed-out runs are not valid zero-clone results.

Review at least 20 high-ranked candidates from each successful structural pass, label actionable / intentional / unclear, check known duplicated HTTP guards and reader helper code, and record observed scope, false positives and operational cost. No automatic refactoring, semantic model, baseline acceptance, new required check, or merging before evidence review.
