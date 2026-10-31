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
# PRIVATE_SQLX_DIR must be an approved local, operator-owned private path.
# The command refuses a directory readable by others or an existing CSV.
umask 077
mkdir -p -m 700 "$PRIVATE_SQLX_DIR"
PGSERVICE=tor_readonly_prod_a \
  python3 tools/audit_sqlx_history.py inventory --from-psql \
  --migrations migrations \
  --save-raw-csv "$PRIVATE_SQLX_DIR/tor-prod-a-ledger.csv" \
  > "$PRIVATE_SQLX_DIR/tor-prod-a-summary.json"
\`\`\`

The command executes **one fixed SQL \`COPY (SELECT ...) TO STDOUT\`**
through \`psql -X\`, with \`default_transaction_read_only=on\`,
statement timeout and no SQL parameters from user input. It neither
applies nor repairs migrations. Keep the database role SELECT-only,
even though the command also enforces read-only transaction defaults.

The optional \`--save-raw-csv\` is supported **only with \`--from-psql\`**.
It preserves the complete original five-column SQLx ledger, including
all SHA-384 checksums, in a *new* mode-0600 file inside an operator-owned
mode-0700 directory. It refuses to overwrite an existing file, follow
a final-component symlink, or save incomplete/malformed CSV. The JSON
summary still goes to stdout; **never** post either artifact or a
database connection string to GitHub, CI logs, chat, or a public Notion page.
The operator must store them in approved private storage and repeat the
command separately for every retained, standby, and restore-capable DB.
A \`BLOCK\` still writes the original ledger for private investigation;
an incomplete inventory does not create a misleading evidence file.

Alternatively export the same five fields with CSV headers
(\`database_name,version,description,checksum_sha384_hex,success\`)
to a protected local file and run:

\`\`\`sh
python3 tools/audit_sqlx_history.py inventory \
  --csv private_history.csv --migrations migrations
\`\`\`

Exit 2 means incomplete evidence or drift. `BLOCK` means a failed,
unknown, altered-description, or altered-checksum applied migration, **or
two distinct applied versions reusing an identical SHA-384 checksum**.
The script also rejects a supplied migration checkout that has **different
version numbers but byte-identical SQL content**. In both cases it reports
version numbers only, not SQL bodies or raw installed checksums.

`NO_APPLIED_DRIFT_DETECTED` means only that the scanned applied rows
match the supplied tree and no exact repeated applied checksum was found;
it is **not deployment approval**. Pending local migrations are listed
separately. An unapplied version **lower than the highest applied version**
is a `BLOCK`, even if every applied row matches: the migration owner must
review ordering/compatibility on an isolated restored copy before any DDL.
Newer, not-yet-applied migrations may remain `pending` without being an
installed-history mismatch, but still require an approved upgrade plan.
Semantically equivalent SQL with different bytes is not detected, and
duplicate no-op/checkpoint migrations require migration-owner review.

Repeat for **each retained primary, standby, backup/restore candidate
that may become authoritative**, plus any rollout environment holding
SQLx history. A clean CI \`tor_test\` is not a substitute.

## 3. Open PR collision inventory

\`\`\`sh
python3 tools/audit_sqlx_history.py scan-open-prs --repo calex794/tor \
  > migration-pr-collisions.json

# To check one OPEN draft (e.g. #348) against current main and all other open drafts,
# while still scanning the complete repository:
python3 tools/audit_sqlx_history.py scan-open-prs --repo calex794/tor \
  --focus-pr 348 --fail-on-conflict > migration-pr-348-conflicts.json
\`\`\`

This uses read-only public GitHub REST queries, optionally
\`GITHUB_TOKEN\` for rate limits. It compares files in exact current
\`main\` against the changed migration files of **all** open PRs,
including stacked Drafts, by numeric version, relative path and Git
blob SHA. Repeated ownership of the **same path and blob** is de-duplicated.
Different files with the same SQLx number, or the same file with
different blob content, are flagged in `collisions` for reconciliation.

A separate diagnostic `cross_version_identical_sql` reports **the same Git
blob SHA** (byte-identical migration SQL) appearing under *different*
numeric versions, with each source path and owning PR. Repeated
path/version/blob inherited by stacked PRs counts once. This catches
accidental double-numbering of the **same exact SQL bytes**, but it does
not detect equivalent SQL whose whitespace, comments or formatting changed.
Neither category proves that the migration was applied in a retained DB.

GitHub's \`/pulls/{number}/files\` endpoint is mutable. The scanner
pins \`main\` and the complete OPEN PR number/head/base map before file reads,
then checks both again afterwards. Changed main, PR membership, heads or
bases produces **INCOMPLETE** (exit 2, no accepted JSON even without
\`--fail-on-conflict\`). These checks add two read-only GitHub requests.
They are **not an atomic GitHub snapshot**: a rapid update-and-revert
could still escape detection. Revalidate exact refs before planning a merge.

The default scan reports candidates without failing unrelated PRs;
`--fail-on-conflict` exits 2 if **either** numeric collision or exact
cross-version reuse is found. For day-to-day PR work, optional
`--focus-pr N` still performs the **same full double-checked OPEN PR scan**,
but returns only collisions actually involving PR `N` and exits 2 on
those when `--fail-on-conflict` is supplied. It also reports counts of
the global conflicts separately so a clean focused result cannot be
mistaken for a clean repository. A missing/closed target PR fails closed,
rather than silently saying that all its versions are free.
This helps avoid blind reassignment of a contested version; after
#365 was merged, SQLx version `202610310293` became owned by **main**,
and #348's different migration at the same version is an example of a
current main-vs-Draft collision. A closed/merged PR (e.g. #365 itself)
is not a valid `--focus-pr` target. This report does **not reserve** a
number or approve a merge. This is a PR-diff inventory, not an applied-DB
check or simulated merge, and possible renames/removals require human review.
Never renumber a migration applied in any retained/restore-capable DB without
a privately approved compatibility plan.

## 4. Release gate remains closed

A migration owner must reconcile all detected versions, establish the
actual installed checksum ledger in each retained environment, and
validate any required additive compatibility repair on a **restored
isolated copy**. Never edit an applied \`_sqlx_migrations\` checksum.
Only after a complete combined-tree, exact-SHA PostgreSQL 18
upgrade/HTTP/S3 CI and separate finance authorization may release
work be reconsidered. #235 and #236 remain open.
