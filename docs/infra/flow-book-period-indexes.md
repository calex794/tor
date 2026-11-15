# FLOW-05: book-period index migrations and interrupted-build recovery

Owner: I with C. Initial implementation base: `80386c64a0175d5658f02db6e2787d7be99d241a`.
The corresponding C evidence query is reviewed at `5c9da751dc64e4a885c738746a16e2b75471d197`.
This package changes indexes and disposable migration acceptance. It adds no production repair automation, business facts, HTTP behavior, collector, or retention policy.

## Purpose and limits

`closed_book_evidence` selects up to 50 currently eligible book IDs and reads starts/completions in `[until - 2 * days, until)`, where `days` is 1/7/30/90. The ID limit does not limit matching facts to 50 rows. Existing user/book/time and time-only indexes do not provide the direct book-then-time access path.

| SQLx version | Index | Keys |
| --- | --- | --- |
| `202611294687` | `reader_chapter_starts_book_period` | `(book_id, started_at)` |
| `202611294688` | `reader_chapter_completions_book_period` | `(book_id, completed_at)` |

Each additive file starts at byte zero with `-- no-transaction` and contains one `CREATE INDEX CONCURRENTLY`. Existing indexes and applied SQL bytes remain unchanged. There is no `IF NOT EXISTS`, partial predicate, or `INCLUDE` payload.

This supplies an index access option; it does not guarantee PostgreSQL will choose it or make popular books cheap. No production `EXPLAIN`, representative-data latency, full-catalog rank benchmark, or build-time estimate has been performed by this package. The existing query timeout remains a failure bound, not a scale measurement.

## Execution and failure boundary

TOR invokes SQLx's migrator without an outer transaction. SQLx 0.8.6 recognizes the leading marker and executes marked SQL directly, then records the successful migration separately. Keep one SQL statement per marked file: adding `SET`, a second `CREATE`, or a `DO` block can introduce a PostgreSQL transaction boundary that is incompatible with concurrent creation.

A cancelled concurrent build can leave an invalid index. A disconnect after successful DDL but before the SQLx ledger insert can leave a valid index without a migration record. In either state, retry fails explicitly on the existing relation. `IF NOT EXISTS` would not prove index validity or its definition and must not be substituted.

`tests/postgres/verify-migrations.sh` preserves a shared transaction for each contiguous sequence of ordinary files. It flushes that sequence before a marked file, executes the marked file alone without `psql -1`, then starts the next ordinary sequence. Failures stop later files, including when the function is called from a conditional regression test.

## Operator recovery: exactly the two new indexes

This is a reviewable operator procedure for an interrupted, separately authorized migration. It is not run by the application or by CI against a retained database. Stop the failed migrator process first so its session advisory lock and any unfinished index command are gone. Do not run a second migrator concurrently.

1. Inspect the target database, migration ledger, and exact catalog object. Do not infer success from an index name.

```sql
SELECT current_database(), current_user;
SELECT version, success, encode(checksum, 'hex') AS checksum
FROM public._sqlx_migrations
WHERE version IN (202611294687, 202611294688)
ORDER BY version;

SELECT n.nspname, c.relname, i.indisvalid, i.indisready, i.indislive,
       pg_get_indexdef(i.indexrelid) AS definition
FROM pg_catalog.pg_index i
JOIN pg_catalog.pg_class c ON c.oid = i.indexrelid
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = 'public'
  AND c.relname IN ('reader_chapter_starts_book_period',
                    'reader_chapter_completions_book_period');
```

2. A ledger entry already present, a different schema/table/key definition, an unexpected object type, or an unrelated name is outside this recipe: stop and investigate. Do not update, insert, or delete SQLx ledger rows. For the affected version with no ledger entry, confirm the exact expected nonunique B-tree definition shown in the table above.

3. Remove only the confirmed affected new index, as one standalone command outside any transaction. Use the starts command **or** the completions command according to the inspected failed version; do not remove an already accepted sibling index.

```sql
DROP INDEX CONCURRENTLY public.reader_chapter_starts_book_period;
```

```sql
DROP INDEX CONCURRENTLY public.reader_chapter_completions_book_period;
```

4. Confirm that the affected object is absent and its ledger version is still absent. Run the unchanged, reviewed `tor-migrate` binary through the normal authorized migration procedure. Inspect `indisvalid`, `indisready`, the exact definition and the real SQLx checksum/ledger entry. Repeat the migrator once to prove no replay.

The existing canonical rows, immutable versions, older indexes and prior ledger entries are not repair targets. Concurrent creation still performs database work and can wait for writers/snapshots; scheduling and operational authorization remain separate from merging this source.

## Disposable acceptance

Create fresh loopback PostgreSQL 18 databases `tor_ci_flow_index_fresh`, `tor_ci_flow_index_upgrade`, `tor_ci_flow_index_invalid`, and `tor_ci_flow_index_unrecorded`, all owned by `tor_migrator`. The ordinary `tor_app` role must exist. Tests reject a non-loopback address, nonstandard port, wrong base database, wrong role or pre-existing ledger.

```sh
FLOW_BOOK_INDEX_MIGRATOR_URL='postgresql://tor_migrator:tor-ci-migrate@127.0.0.1:5432/tor_ci_flow_index_fresh' \
  cargo test -p tor-migrations --test flow_book_indexes_pg -- --ignored --test-threads=1 --nocapture

# Existing disposable smoke also invokes its real transaction-mode regression:
bash tests/postgres/verify-migrations.sh
```

The Rust target checks fresh/replay; upgrade with retained canonical starts, completions, outbox and immutable versions; cancellation at PostgreSQL's observed `waiting for writers before build` phase; and valid DDL without its later ledger insert. Both affected indexes are tested. Failed retries preserve the exact prior ledger, explicit repair drops only the inspected new index, and unchanged SQLx retry creates a valid index and a real ledger record. The writer-wait case also checks that another writer can take a table lock while concurrent creation waits.

The normal smoke entrypoint invokes `verify-migration-modes.sh` exactly once; do not invoke it again against the same disposable service. The shell regression proves ordinary multi-file rollback, the committed boundary around a real concurrent index, rollback of a following ordinary batch, and stopping after a marked failure.

Acceptance requires actual successful Actions evidence on the final source SHA. Temporary WIP workflow files are not part of the production patch. The permanent CI invocation is composed by I.

## Primary implementation references

- [SQLx 0.8.6 marker parsing](https://github.com/transact-rs/sqlx/blob/v0.8.6/sqlx-core/src/migrate/source.rs)
- [SQLx 0.8.6 PostgreSQL execution and subsequent ledger insert](https://github.com/transact-rs/sqlx/blob/v0.8.6/sqlx-postgres/src/migrate.rs)
- [PostgreSQL 18 CREATE INDEX: concurrent creation and failure recovery](https://www.postgresql.org/docs/18/sql-createindex.html)
- [PostgreSQL 18 multicolumn index access](https://www.postgresql.org/docs/18/indexes-multicolumn.html)
