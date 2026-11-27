# I-15 — SQLx migration history and open-Draft collision audit

**Status:** tooling only. No SQL migrations, refund policy, wallet writes,
main merge, deployment or changes to existing Draft branches.

## 1. Why this is necessary

The green isolated I-14 test [#243] upgrades the exact public main schema
(41 migrations) to an integrated Draft schema (48 migrations), preserving
all 41 version/description/SHA-384 checksums. This does **not** prove that
any persistent database has the same SQLx history: old experimental Draft
migrations might have used the same number with different SQL.

Likewise green CI on a single PR does not reserve an SQLx version across
unmerged PRs. #235 tracks version conflicts with #178, #189, #204, #209,
#216, #227, #228 and main. The script reports *candidates*, not a valid
merge plan.

## 2. One database at a time, read-only

An authorized operator selects an existing **SELECT-only** PostgreSQL
connection through a local libpq \`PGSERVICE\` entry or \`PGDATABASE\` plus
other \`PG*\` environment variables. Never paste credentials, hosts or
the inventory result into an issue. Run from a checkout of the **exact
intended integration SHA**, and store the report in the organization's
approved private audit storage:

\`\`\`sh
PGSERVICE=tor_readonly_prod_a \
  python3 tools/audit_sqlx_history.py inventory --from-psql \
  --migrations migrations > audit-tor-prod-a.json
\`\`\`

The command executes **one fixed SQL \`COPY (SELECT ...) TO STDOUT\`**
through \`psql -X\`, with \`default_transaction_read_only=on\`,
statement timeout and no SQL parameters from user input. It neither
applies nor repairs migrations. Keep the database role SELECT-only,
even though the command also enforces read-only transaction defaults.

Alternatively export the same five fields with CSV headers
(\`database_name,version,description,checksum_sha384_hex,success\`)
to a protected local file and run:

\`\`\`sh
python3 tools/audit_sqlx_history.py inventory \
  --csv private_history.csv --migrations migrations
\`\`\`

Exit 2 means incomplete evidence or drift. \`BLOCK\` means a failed,
unknown, altered-description, or altered-checksum applied migration.
\`NO_APPLIED_DRIFT_DETECTED\` means only that the scanned applied rows
match the supplied tree; it is **not deployment approval**. Pending local
migrations are listed separately; an older-than-latest pending version
needs tested upgrade ordering.

Repeat for **each retained primary, standby, backup/restore candidate
that may become authoritative**, plus any rollout environment holding
SQLx history. A clean CI \`tor_test\` is not a substitute.

## 3. Open PR collision inventory

\`\`\`sh
python3 tools/audit_sqlx_history.py scan-open-prs --repo calex794/tor \
  > migration-pr-collisions.json
\`\`\`

This uses read-only public GitHub REST queries, optionally
\`GITHUB_TOKEN\` for rate limits. It compares files in exact current
\`main\` against the changed migration files of **all** open PRs,
including stacked Drafts, by numeric version, relative path and Git
blob SHA. Repeated ownership of the **same path and blob** is de-duplicated.
Different files with the same SQLx number, or the same file with
different blob content, are flagged for reconciliation.

The default scan reports conflicts without making unrelated PRs
fail; \`--fail-on-conflict\` is available for a deliberate integration
gate. This is a PR-diff inventory, not an applied-DB check or simulated
merge, and possible renames/removals require human review.

## 4. Release gate remains closed

A migration owner must reconcile all detected versions, establish the
actual installed checksum ledger in each retained environment, and
validate any required additive compatibility repair on a **restored
isolated copy**. Never edit an applied \`_sqlx_migrations\` checksum.
Only after a complete combined-tree, exact-SHA PostgreSQL 18
upgrade/HTTP/S3 CI and separate finance authorization may release
work be reconsidered. #235 and #236 remain open.
