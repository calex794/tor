# TOR I-03/I-05: first PostgreSQL-backed public catalog

Branch: `feat/i-postgres-catalog-20261026`. Source:
[bokarust@d11b27b](https://github.com/calex794/bokarust/blob/d11b27b43125b93e59809147e8e687203fa0753b/crates/adapters/src/postgres.rs).

- Web application now accepts `DATABASE_URL` and uses the restricted `tor_app` role for catalog reads; `TOR_DEMO_MODE=1` remains available without a URL.
- `tor-books` implements `PgCatalog`; no SQLx in web handlers, service, or domain. Draft, held and deleted books always stay hidden. A book lookup no longer arbitrarily scans only the first 100 IDs.
- Dedicated `tor-migrations` binary embeds all nine donor files via SQLx, records checksums in `_sqlx_migrations`, requires `tor_migrator`, and rejects unexpected database names. It is **not** invoked by HTTP startup.
- `/readyz` checks connection, applied migrations and ability to read the books schema in PG mode; `/healthz` remains process-only.
- CI runs real PostgreSQL 18 with segregated roles, migrations twice, negative DDL assertions, Rust tests and HTTP integration. Demo-mode auto-deploy remains unchanged.

**Important:** This is NOT registration or chapter reading yet. Do not switch the VPS to PG mode before a verified off-host backup, migration runner release, test restore, passing PR CI, and controlled activation. No secrets in GitHub/Notion. URL book crawler remains frozen.

Acceptance evidence must come from the PR checks; this document records intended checks, not a passing result.
