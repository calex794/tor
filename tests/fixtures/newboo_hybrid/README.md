# Pinned NEwBOO REC-07 oracle

These eight Python files are unchanged snapshots from
`HeisLuka/newboo@17125ef758a791e9630505c14d24f7fe65aed228`.
`manifest.json` records each original URL, Git blob SHA and SHA-256.
They are test fixtures, never production modules. The test loader recreates the
original module names without booting the Python web app or contacting services.

`python3 tools/check_hybrid_parity.py` executes the original scorer and validates
REC-M01/M02/M03/M08. `cargo test -p tor-recommendations --test hybrid_parity`
compares every returned field with Rust across 181 deterministic inputs: the
four golden cases, 17 explicit edge cases and 160 seeded generated cases.
The test is unignored and runs in the existing workspace test CI.

The old REC-M08 fixture confused a **quality score** with its **boost**.
The pinned function applies `clamp(score,0,1000) * clamp(multiplier,0,1000)
// 1000 // 20`, capped at 50, excluding degraded/reject signals and taking the
maximum signal per book. Raw quality 50 adds 2; raw quality 1000 adds 50.
The fixture now supplies raw quality 1000 and preserves expected score 760.

This proves only the pure scoring boundary, not recommendation assembly,
database projections, five live UI surfaces, privacy, rollout or product parity.
