# REUSE-01 — inventory before refactoring

Owner: I (tooling and measurement); subsequent refactors belong to the owning A–H domain. No runtime, SQL migration, route, security policy or deployment changes are part of this slice.

## Run

From the repository:

    python3 tools/audit_code_reuse.py --ref HEAD --top 100 --output /tmp/tor-reuse.json
    python3 -m unittest discover -s tests/architecture -p test_code_reuse.py -v

The tool reads tracked, regular UTF-8 blobs directly from the resolved Git commit, not dirty checkout files, caches or dependencies. It scans Rust, JS/CJS/MJS, CSS, HTML, Python, SQL, TOML, YAML and shell sources across all crates, tests and infrastructure. Output has a commit SHA, per-area and per-scope counts, exact byte-identical blob groups, candidate line ranges, and skipped high-frequency windows. Source text is not included in the report. Existing Repository rules unittest discovery emits a compact actual-checkout report (top 30 pairs) without a duplication threshold or new CI job.

## Method and interpretation

Default: eight consecutive nonblank lines, stripped at their edges only, at least 160 characters, compared within a language. Adjacent matching windows are consolidated into candidate pairs; within-file overlapping copies are not reported. Windows occurring more than 16 times are counted and skipped to bound boilerplate comparisons. The candidate-line count is the union of participating nonblank physical lines, before truncating the displayed pairs. Exact file groups are independent; do not add those counts to candidate lines. Inline test modules are included in application scope; directory and filename classification is approximate. The reported ratio is a conservative heuristic coverage measure, not semantic duplication, a maintainability grade, or a target to optimize.

This method does not detect reordered or renamed code reliably, repeated business rules expressed differently, or all SQL/HTML strings. Different domains can legitimately use similar code; migrations, compatibility shims, negative tests, and generated-like declarations need review. A hash or a match is not authorization to delete code. Reports can be compared only with the same settings and scope.

## Sequencing

| ID | Work | Acceptance |
| --- | --- | --- |
| REUSE-01 | Read-only inventory and repeatable baseline | Fixtures, actual committed-checkout report |
| REUSE-02 | HTTP request guards | Preserve active actor, HTML vs strict JSON Origin/CSRF, status/role distinctions and RIL coverage |
| REUSE-03 | Shared book presentation | Preserve compact, library and recommendation variants, analytics, escaping and cover fallback |
| REUSE-04 | Reader UI helpers | Preserve public, paid, archived rights and browser behavior |
| REUSE-05 | Storage operations review | Coordinate with DROP/FND; preserve exact versions, reauthorization, SINGLE/PACK |
| REUSE-06 | Commerce primitives review | Preserve transaction boundaries, idempotency, ledger and allocation policy |
| REUSE-07 | Test support | Keep independent fixtures and negative cases where intentional |
| REUSE-08 | UI controls | Reuse forms, dialogs and pagination where semantics agree |
| REUSE-09 | SQL review | Preserve authoritative writers, access filters and frozen migrations |
| REUSE-10 | CI policy | Baseline and review first; no blanket ban or automatic rewrite |

No work on the runtime steps is implied by a successful REUSE-01 report. Each follow-up requires its own bounded PR, domain-owner review and relevant regression evidence. Do not merge based on a clone score.

## REUSE-01B — measured comparison, 2026-11-24

Evidence: [experiment #1204](https://github.com/calex794/tor/pull/1204), [Actions run 45422324358](https://github.com/calex794/tor/actions/runs/45422324358). All six scan modes returned exit 0 on the same committed Rust input, a527bf2e3c05ce577e7a4363296d3c0a5978c69b. Input: 1,436 files; manifest SHA-256 90dafd7f91f4d610d952e6e5af5733e12bad89d3a85a2493e21e13045840752d for all three jobs. Source read-only and offline at scan time; 2 CPUs, 4 GiB container limit. Tool installation time not included in scan times.

| Mode | Analyzer count | Reported scan time | Peak cgroup bytes |
| --- | ---: | ---: | ---: |
| Doppelscan 0.9.0 strict | 624 groups | 1.275 s | 147075072 |
| Doppelscan normalized | 2757 groups | 1.338 s | 155193344 |
| Doppelscan near | 145 groups | 1.402 s | 137830400 |
| Cargo-dupes 0.2.1 AST | 255 exact + 145 near groups | 7 s wall incl. container | 159379456 |
| Jscpd 5.4.3 exact | 1901 pairs | 0.572 s | 463998976 |
| Jscpd normalized + structural | 8519 pairs (355 exact, 7958 renamed, 206 similar) | 1.38 s | 569556992 |

Counts are NOT comparable across tools. Jscpd reports 1393 exact and 1394 structural sources; it excludes some files and structural mode treats test code differently. Cargo-dupes analyzed 5867 units and warned about a parse failure in layer_picker_http_pg/staged_flow.rs; its stdout contains consecutive JSON values, not a single JSON document. Doppelscan reports 1436 files. No semantic-equivalence claims.

The independent REUSE-01 report from [rules job 138539452128](https://github.com/calex794/tor/actions/runs/45420803304/job/138539452128) measured merge checkout c9f80e99428d2ac48f6e821ac46020eb16a68e2b: 2097 files, 403187 nonblank lines, 30473 candidate lines (7.56%), 3507 candidate pairs, 15 high-frequency windows skipped. Approximate application scope: 13060/175641 candidate lines (7.44%); tests: 10526/177146 (5.94%); infrastructure: 6887/50400 (13.66%). This is a different, broader snapshot and method from REUSE-01B.

### Decision and next review

Use Doppelscan as preferred **advisory** structural detector, retain independent offline textual inventory, and use Jscpd for targeted deeper audits. This is a provisional operational choice based on measured cost, repeatable output, and relevant findings, not a quantified precision/recall study. Do not merge the experimental workflow as a permanent gate. A reviewed baseline and new-code policy belong to REUSE-10.

Confirmed source candidates and limits:
- Doppelscan normalized groups 1 (80 occurrences) and 2 (24 occurrences): repeated session/HTTP mutation checks. REUSE-02 begins with the two paid delivery grant endpoints. Jscpd reports their function similarity as 0.864 and an exact 15-line block. Preserve authentication-before-CSRF, explicit Origin, status precedence, role and resource-specific checks, rate limits, audit, and RIL regression tests. No generic has_access.
- Doppelscan normalized groups 6 and 17: free/paid comment appeal repositories (397 lines structurally similar) and HTTP forms/handlers (186 lines). Same social-domain owner; distinct table sets and moderation semantics require separate tests. Queue as a subsequent E-domain slice, not a global abstraction.
- Repeated PG fixtures and independent paid-access tests: review shared setup only, not assertions or negative coverage (REUSE-07).
- Workflow job body duplicates are currently intentionally enforced by CI wiring tests; do not delete or consolidate merely because REUSE-01 reports them.
- Repeated historical migrations remain immutable. No migration rewrite.

Do not infer a reduction target from these metrics. Candidate classification is preliminary; individual refactor PRs must demonstrate behavioral equivalence.
