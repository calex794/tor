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

Price a proposal against leaving the source in place over the same time
horizon. In each store, include reads of both source objects, any backup
or copy operations, writes of the replacement and read-back verification.
Charge time when both layouts coexist. If an archive must retain the old
copy for the whole horizon, charge overlap for the whole horizon and set
its time after retirement to zero. Use tariffs in a single currency, with
separate rates and request counts for each store. A nonpositive estimate
gives no economic reason to proceed. The estimate covers bytes and the
recorded read/write charges; separately account for cross-provider
transfer, minimum object billing and compute where they apply.

## PostgreSQL reference evidence

Export one registered PACK from the repository root with a read-only
operator database connection. The CSV is ordered by frame offset and
contains the PACK and manifest digests, exact edition identities and
each observed protection.

```sh
psql -Xq --dbname "$TOR_INVENTORY_DATABASE_URL" -v pack_id=123 \
  -f tools/drop07_export_pack_retention.sql > pack-123-retention.csv
```

Use the CSV only if `psql` exits successfully. A missing PACK, an empty
membership list, duplicate editions or more than 4,096 registered members
prevents export. Verify completeness by comparing every emitted identity,
offset, size and digest with the full independent PACK and manifest readback.

`RETAIN` records at least one protection from the same database snapshot.
The report includes an exact active placement, a current publication,
a reader-saved FREE pin, a Layer FREE archive pin, an administrative hold,
or pending PACK work. Rows without any of these protections are
`UNKNOWN`. Keep them `Unknown` in the simulator until there is positive
evidence for retention or expiry from the applicable policy and backups.

Before wiring the operator path, derive the complete retention inventory
from a fenced PostgreSQL snapshot and verified backup evidence. Revalidate
it when advancing placements and before any source retirement. Retain old
placements through quarantine and prove version-specific DELETE on the
actual provider. Matching aggregate inventory counts provide no substitute
for these object-specific checks.

## PostgreSQL cutover race fixture

`drop_07k_pack_row_lock_misses_new_pins_bounded_fence_rechecks_before_cas`
uses the TOR schema and separate sessions on disposable PostgreSQL 18.
An archive writer commits a new Layer pin while another session holds a
lock on the source PACK row. A second read in the original repeatable-read
transaction still returns the old evidence.

The fixture then acquires table locks with `NOWAIT` before reading fresh
evidence in a new transaction. It refuses to start while a saved-FREE
write is in progress. Once the writer commits, the fresh check detects the
changed evidence and leaves the source pointer in place. With matching
evidence, the fixture advances the exact active generation through the
existing compare-and-swap. A competing pin deletion times out while the
transaction holds the locks. Readers still see the committed generation.
After commit, both protected pins and the old immutable placement remain.

The test uses catalog fixtures for PACK receipts. Table locks cover the
exporter's current data sources and can block writes for unrelated books.
They are confined to the fixture. Before an operator path is introduced,
write coordination must cover all retention inputs and preserve future
grants after commit. Retirement needs a separate durable decision before
any external object deletion.
