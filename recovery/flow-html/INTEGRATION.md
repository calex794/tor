# H HTML health: recovery and integration status

Status: **UNCOMPILED / UNVERIFIED**. These are recovery artifacts and proposed
source changes, not an accepted implementation revision. No PR or main update
is authorized by this file.

## Exact prerequisite and original checkpoint

The required local implementation base is
`cc418415a813c527237a54b71426c9a4c69abf9b` on `feat/flow-signals`.
It includes the earlier `dd8afae` and `c5c5cc5d` signals work. Its complete
tree is missing from the retained cache and is not reconstructed here.

Recovery commit `9d4e2d9f5324712a846a3e1f36ab86f5d17707d0` preserves the
unfinished H authoring sources, known edits, and validation history from before
the workspace failure. Its published parent `85f47ab9` is only a storage base.
The original fifteen recovery files remain unchanged.

These retained inputs are not guaranteed byte-identical to the last local
Rustfmt output. Compare a recovered working tree before applying anything. Do
not transfer old PASS results to this newly authored tree.

## Apply order

1. Recover and verify the exact local cc418 tree.
2. Compare/apply the original H sources and edit records from 9d4e2d9.
3. Apply `fixes.json` from `08fd158bc46146bac586c4b8ffccc138e4fab836`.
4. Apply `ack-fix.json` from `e81ba6ccaca081558c3d9cb5bc1d58ba367a6463`.
5. Apply `handoff-fix.json` from `a5affa9d6dd336dac9ebcc0e2cb1f75396f1c9f2`.
6. Apply `contract-fix.json` from this integration checkpoint.

Execute each operation in order and require its documented exact match count.
Do not blindly run these textual edits against formatted or independently
modified source. Resolve a mismatch by inspecting the actual code. Do not copy
a candidate file and then apply its preceding JSON changes to it a second time.

The complete candidate copies exist only for retained new H files. Existing
base files are represented by edits to known H-authored fragments. Root owns
integration with the other FLOW rules, routes and startup/shutdown wiring.

## Where to inspect the latest candidate files

| Intended repository file | Latest recovery candidate |
| --- | --- |
| `migrations/202611294685_flow_html_health.sql` | `fixes/sources/migrations/202611294685_flow_html_health.sql` |
| `crates/traffic_behavior/src/domain/html_health.rs` | Original `sources/` mirror |
| `crates/traffic_behavior/src/services/recorder/flow_health.rs` | `fixes/sources/` mirror |
| `crates/traffic_behavior/src/repositories/html_health/read.rs` | `fixes/sources/` mirror |
| `crates/traffic_behavior/src/repositories/html_health.rs` | `handoff-fix/sources/` mirror |
| `crates/traffic_behavior/src/services/html_pump.rs` | `handoff-fix/sources/` mirror |
| `crates/traffic_behavior/tests/ril_rollups_pg/health.rs` | `handoff-fix/sources/` mirror |
| `docs/contracts/flow-html-health-v1.md` | `contract-fix/sources/` mirror |

## Proposed guarantees and limits

The proof covers registered anonymous HTML collectors. It does not establish
that every process in the installation is registered, and it does not count
people. No reader identity, cookie, IP history, consent flow or behavioral
attribution is added.

An expired lease does not close a non-retired worker's interval. Another healthy
worker, a clean restart or retention cleanup cannot make that unknown tail
healthy. Unresolved epoch records remain until an actual lifecycle witness
exists; this may keep the source unknown indefinitely.

First registration persists its preceding unacknowledged interval and known
unreported loss/error flags in the same transaction as its new epoch. The lower
bound uses an earlier DB clock sample minus the monotonic age measured after
the reply. A new counter baseline cannot swallow startup losses.

Reregistration only stages local intent until one transaction closes the
caller's own predecessor, records the gap and creates its successor with a
common DB instant. There is no committed period with the old worker closed and
the replacement absent. Failed registration leaves the predecessor unresolved.

Counts and optional health are read in one repeatable-read snapshot. A genuinely
absent, unapplied health capability preserves available counts with unknown
quality. Partial, applied-but-missing, or unreadable installed health is an
error. Missing history is never converted to zero without complete no-loss
acknowledgement.

Clock acceptance accounts for both endpoints of the SQL sample interval and
the system-clock read interval, with the DB floor-rounding uncertainty. The
accepted timestamp must lie within +/- 2 seconds for every possible DB instant.
The 25-second monotonic lease begins at the earlier SQL start instant.

A done FLOW-bearing batch requires its immutable observed receipt before it
can advance a checkpoint. A purely technical RIL batch may legitimately lack
such a receipt.

## Mandatory real shutdown ordering — NOT YET VERIFIED

The HTTP listener must stop accepting requests and all in-flight middleware
must drain before terminal retire. Closing the recorder gate or receiving
SIGTERM alone is not evidence that HTTP stopped. Drop, task abort and panic
without that witness leave the epoch unresolved.

Root accepted this ordering, but the actual flow_runtime integration source
was unavailable during this work. Its stop order has **not** been checked.
The real server shutdown path and an interrupted shutdown path must be
verified after recovery. Do not report SIGTERM acceptance as complete based on
the isolated pump fixture.

## Evidence completed here

GitHub commit parent/tree metadata and every added artifact were read back and
compared with the stored authoring contents. Strict edits for retained full
sources were replayed in memory to confirm that their anchors produce the
candidate copies. Those checks establish artifact preservation, not Rust,
SQL, HTTP or product correctness.

Independent static review covered the actual recovery sources for H1-H4,
the conditional FLOW receipt rule, and atomic handoff through a5affa9d. The
reviewer found no remaining substantive source blocker in that scope. The
reviewer independently replayed the exact edits into the retained full candidate
sources. This was source/test-draft review, not compiler, PostgreSQL, HTTP,
browser or CI execution. The actual I-layer HTTP stop/drain witness remains
unverified. The contract follow-up removes the earlier obsolete lifecycle and
retention wording and explicitly records the pending acceptance.

## Remaining acceptance

Use a fresh isolated validation database because the unpublished 685 migration
proposal changes its index expression. Format and compile the restored tree,
then run the focused regression cases for unresolved epochs, cleanup, startup
losses, UTC boundaries, legacy capability, installed-schema failures, clock
bounds, receipt acknowledgement and atomic handoff.

Verify real HTTP response behavior for missing versus failed health, read-only
GET behavior, mobile/desktop rendering, and the actual startup/shutdown order.
Run the required exact-revision CI and normal repository acceptance after root
integration. Only those results can support implementation closure.
