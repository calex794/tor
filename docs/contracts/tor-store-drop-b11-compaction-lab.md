# DROP-B11 — offline PACK rewrite analysis

A verified source PACK and the complete ordered immutable edition inventory
are required. The caller must provide the reference outcome for each exact
chapter and version. Active, pinned and backup-required editions remain in
the result. An unknown reference prevents a proposal. A whole pack with no
retained members requires its own retention process.

The simulator decodes the retained frames, checks their actual edition
digests and bytes, and builds a new PACK and manifest under a different
key. It reports the measured rewrite I/O and the potential byte savings
after retirement. Read/write and minimum saving budgets prevent proposals
that would exceed the allocation. It performs no bucket calls, reference
changes or deletion. The disposable fixture covers pinned V1, an expired
member, uncertainty, corrupt input and a restrictive rewrite budget.

Before wiring the operator path, derive the complete retention inventory
from a fenced PostgreSQL snapshot and verified backup evidence. Revalidate
it when advancing placements and before any source retirement. Retain old
placements through quarantine and prove version-specific DELETE on the
actual provider. Matching aggregate inventory counts provide no substitute
for these object-specific checks.
