# Exact FLOW receipt acknowledgement — uncompiled and unverified

Apply this proposed follow-up after the original H authoring checkpoint and
the four-fix patch at `08fd158bc46146bac586c4b8ffccc138e4fab836`.
The exact required local base is still unavailable. No formatter, compiler,
PostgreSQL test, HTTP test, browser test or CI has run this new tree.

`../ack-fix.json` contains the strict edit and regression append. The two
candidate files under `sources/` include the preceding four-fix candidates.
The original recovery sources and earlier fixes remain unchanged.

The previous finish query treated a missing receipt as zero discarded events.
That is valid for a purely technical RIL batch, but does not prove that a FLOW
batch was materialized. The new predicate requires the immutable receipt when
the batch has staged observed FLOW rows or a positive observed discard count.
It continues to accept a done technical batch without FLOW.

The added PG draft uses an isolated LIKE fixture without production triggers to
simulate a done FLOW batch written without its receipt. The acknowledgement
must stay pending and confirmed time must not advance. Normal materialization
then permits acknowledgement. A technical batch without FLOW is accepted, while
a discard-only FLOW batch without a receipt is rejected.

This is defense of the existing exact-batch invariant, not an additional
requirement that every technical RIL batch receive a FLOW receipt. The subquery
is scoped to the exact batch key and retains the existing bounded transaction.

This recovery artifact is not a passing implementation revision or a merge
request. Run its regressions and full required acceptance on the restored tree.
