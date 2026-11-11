# Proposed H health fixes — uncompiled and unverified

This is a proposed follow-up to recovery checkpoint
`9d4e2d9f5324712a846a3e1f36ab86f5d17707d0`.
The original checkpoint sources and edit records are unchanged.

No Rust compiler, formatter, PostgreSQL runtime, browser, or CI has checked this
patch. The JSON and candidate sources are review material, not acceptance
evidence. They require the exact unpublished local base
`cc418415a813c527237a54b71426c9a4c69abf9b` plus the original H edits.
The published parent of the recovery branch is only a storage base.

## Apply order

Recover the exact local base. Compare/apply the original H authoring sources and
edits. Then apply `../fixes.json` in order, requiring exactly one match for each
anchor. The six full candidate files under `sources/` are the result of those
operations for files whose entire H source was retained. Other files are
represented only by exact edits to known H-authored snippets; their missing base
has not been reconstructed. Format, compile and run the resulting new tree.

## Four corrected invariants

### H1 — an expired lease is not evidence that a worker stopped

A non-retired epoch has an open upper range for overlap and must confirm through
the full end of each intersecting hour. The union of other workers' acknowledged
ranges does not waive that requirement. Retired epochs retain a finite bound.

The range index uses the same expression as the read. Cleanup deletes only
retired epochs after retention; age alone never closes an unresolved source.
The normal hourly retention remains bounded. Unresolved epoch records remain
until a real stop witness exists. A returning worker's existing retire operation
ends at its actual return and preserves its unconfirmed tail.

The PG draft creates A whose lease ended before both comparison periods and B
which covers both periods. Both periods remain unknown. It also checks that an
unresolved record older than 181 days survives cleanup while a closed record can
be removed.

This proves only the behavior of registered collectors. There is no global
replica registry or automatic witness of a disappeared process. An unresolved
worker can therefore keep the source unknown indefinitely; this patch does not
invent a stop time or silently reset the problem.

### H2 — starting a new epoch must not forgive earlier known losses

The recorder retains its monotonic creation instant. Once DB readiness returns,
the service obtains a DB clock sample and subtracts the recorder/gap age measured
after that response. The resulting lower bound precedes the actual gap start,
including arbitrary reply delay; no guessed round-trip-time allowance is used.
Age rounds upward, the DB sample rounds downward, and an extra millisecond keeps
the boundary conservative.

One transaction writes a closed, unacknowledged interval before registration,
persists any unreported loss/error flags over that interval, and creates the
new live epoch with its initial counters. Only after that transaction succeeds
does the service advance its in-memory reported cursor. A known registration gap
therefore remains unknown even if no recorded loss was present. Losses which
arrive after a successful retirement are retained for the next registration.

The service deliberately does not advance its reported cursor merely because
retire returned successfully. This is conservative across an ambiguous retire
commit followed by new losses: the following gap may retain duplicate boolean
failure evidence, but cannot silently consume a new loss.

Draft coverage includes a loss before first readiness in the real pump, a gap
crossing a UTC hour by one millisecond while B is healthy, a new loss after
retirement, and deterministic lower-bound arithmetic with a delayed DB reply.
An ambiguous first registration can leave an unresolved epoch; it remains
unknown under H1. No new epoch is treated as proof that an earlier one stopped.

### H3 — missing health capability is distinct from a failed source

Counts, capability detection and optional health proof share one repeatable-read
snapshot. Both health tables absent with no successful health migration yield
available counts plus absent health. An applied marker with absent tables, one
table without the other, existing tables without the applied migration, or an
error reading the installed ledger yields an error.

The signal adapter receives missing evidence when health is absent. It cannot
produce Healthy or invent a zero from the available count rows. The audience
view preserves those counts and explicitly says the health journal is not yet
installed. A real installed-source failure retains its unavailable response.

The isolated legacy PG draft excludes public from search_path, keeps the
available observation tables and counts, and progressively checks absent,
applied-but-missing, partial, installed, and locked health. It never falls back
after a SQL error. If the observation source itself is absent, its existing
unavailable behavior remains; this patch does not invent historical counts.

### H4 — clock acceptance includes both sides of SQL uncertainty

Activation records monotonic instants before SQL and after its response. Each
system-clock read is itself bracketed by two monotonic instants. Given the
floor-rounded DB sample, these yield a lower and upper bound for possible DB
time at observation. The timestamp is accepted only in the intersection
[upper minus 2 seconds, lower plus 2 seconds].

Arithmetic uses nanoseconds and includes the DB sample's one-millisecond
rounding interval. The 25-second lease still starts at the earlier instant.
An interval wider than four seconds cannot pass. The draft tests include
positive and negative offsets, both endpoints and their one-nanosecond
neighbors, 3999/4000/4001ms RTT boundaries, the previous +2.9s false acceptance,
and the exact UTC hour boundary covered by inclusive failure flags.

## Remaining acceptance and review

These changes have only been assembled and their retained-source anchors
replayed in memory. That is not Rust or database verification. Restore the exact
base, apply with strict anchors, run formatter and compile, then run the focused
PG/unit/HTTP/browser cases and exact-revision CI.

An additional review finding about a done FLOW batch lacking its immutable
receipt is being prepared separately. The receipt is required for FLOW-bearing
batches, while a purely technical batch may legitimately have no FLOW receipt.

No implementation PR, main ref, application code on the published storage base,
production database or deployment is changed by these recovery artifacts.
