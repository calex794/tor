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
