# Atomic collector replacement — uncompiled and unverified

This proposed follow-up applies after the original H checkpoint, the four-fix
patch `08fd158`, and the receipt patch `e81ba6c`.
Only recovery artifacts are changed. No Rust or runtime acceptance has run.

## The corrected handoff

Expiry means the current collector cannot renew its acknowledgement. It does
not mean HTTP stopped. The service now closes its in-memory gate and remembers
its own predecessor without retiring that epoch in the database. The predecessor
remains an open unknown source if the process stalls, loses DB access, aborts or
never succeeds in registering again.

A later registration uses one transaction and one DB instant to persist known
predecessor failures, close that predecessor, store the preceding unacknowledged
gap, and create the live successor. There is no committed state with the
predecessor closed and successor absent. A failure before commit rolls back all
parts. During the transaction a reader sees the earlier open predecessor; after
commit it sees the successor. Historical unknown intervals are never filled.

The owner service replaces only its own predecessor identity held in memory.
There is no search for expired workers and no automatic bulk retirement. A new
process does not close a previous process's unresolved epoch.

## Terminal shutdown is a different witness

The public pump shutdown contract now explicitly requires the HTTP listener to
have stopped accepting requests and all in-flight middleware to have drained
before terminal retire. Closing the recorder gate, receiving SIGTERM, dropping
a task or starting a replacement process is not sufficient evidence.

A normal replacement uses the atomic transaction above. Only a final, drained
HTTP shutdown can invoke standalone retire. Drop/abort/panic closes the local
gate while leaving the database epoch unresolved.

Root accepted this ordering, but its flow_runtime integration source is
currently unavailable. Its actual stop order has NOT been checked. After source
recovery, verify it with the real server lifecycle and an interrupted shutdown
case. This requirement must remain visible in the final integration README.

## Proposed regression

The PG draft starts an owned pump epoch, expires it before both comparison
windows, and supplies a second healthy worker covering the periods. It invokes
the service expiry transition without a new successful registration and checks
that the predecessor remains open and coverage remains unknown.

A health-hour lock then forces the new registration transaction to fail. The
test checks that no successor exists and the predecessor is still open. A later
successful transaction closes the predecessor at exactly the successor's start
instant while preserving the old confirmed boundary and historical unknown
coverage.

The fixture submits no further FLOW observations before its final shutdown; it
has no live HTTP listener. This unit/integration fixture is not evidence that
the real application's shutdown order is correct.

## Restore and validate

Apply `../handoff-fix.json` after the preceding patches. Respect each operation's
expected match count; one deliberate operation updates both Expired branches.
The three full candidate files under `sources/` contain all preceding changes.

They remain unformatted, uncompiled and unverified. Restore the exact local
base, apply strict anchors, format, compile and run the focused regressions,
actual HTTP/browser lifecycle checks and required exact-revision CI.
