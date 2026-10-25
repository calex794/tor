# TOR I-05: original PostgreSQL migrations imported from bokarust

The nine SQL files in this directory are **unchanged Git blobs** copied from
[calex794/bokarust, commit d11b27b43125b93e59809147e8e687203fa0753b](https://github.com/calex794/bokarust/tree/d11b27b43125b93e59809147e8e687203fa0753b/crates/adapters/migrations).
**Do not modify any of the nine historical files.** The \`SOURCES.tsv\` manifests original Git
blob hashes, validated by \`python3 tools/check_migration_sources.py\`.

Migrations have monotonically increasing SQLx filename versions. The historical
\`bokarust\` SQLx adapter and app logic have **not** yet been moved to TOR; this
is the first schema handoff, not a connected product. The GitHub
\`TOR PostgreSQL 18 migration contract\` workflow spins up an ephemeral PostgreSQL 18
and uses \`psql\` to validate **fresh install, upgrade with existing data and core
schema invariants**. The CI smoke applies SQL directly to disposable databases;
it is **not** the production migration runner and does not populate
\`_sqlx_migrations\`.

**Do not run \`psql -f\` blindly against the VPS TOR database.** The next I-line
deliverable must add the canonical SQLx migrator, verify checksums and version
history, and take a database backup. Only after this step may the migrations be
applied to \`tor\` by \`tor_migrator\`, followed by actual SQLx repositories and the
HTTP → PG → browser path. The VPS \`tor-web\` is still RAM-demo.

The app \`tor_app\` has only DML access and lacks schema CREATE. Never use
the PostgreSQL superuser for the running site; passwords remain exclusively
on the VPS in \`/etc/tor/tor-app.env\` and \`/etc/tor/tor-migrator.env\`.
