# AUTH-PROFILE-06a — read-only historical notification inventory

A-06/E-08, S004/S015. Follow-up to accepted #1365. Current delivery and read-time
projection hide known legacy actor mailbox labels; persisted snapshots can still
contain those labels. Do not equate read-time privacy with physical erasure.

The repository exposes only aggregate counts for an explicitly bounded primary-key
interval. Capture a high watermark, scan at most 1,000 existing rows per batch,
advance to the returned last scanned ID, and stop at an empty batch or upper bound.
Markers match the existing read-path checks: @ in title/body/URL or %40 in URL.
These are indicators, not proof of private mailbox data. The matched subset has
one of the eight canonical actor event types, expected notification category, and
matching actor/recipient IDs. Remaining markers require separate producer review.
No raw content, mailbox, recipient/actor IDs, or URLs are returned; the numeric
cursor is for a private internal caller, not a public endpoint.

The high watermark bounds the scan, not a consistent snapshot across transactions.
Repeat under the approved maintenance policy if concurrent writes occurred.
No update/delete, migration, production scan, CLI, HTTP route, or automatic task.
Physical cleanup is a later separately tested change: it must revalidate bindings,
current actor identity and canonical target, preserve recipients/event facts/read
state, handle missing sources, and distinguish system projections from user text.
Past logs, backups and exports are separate scope. No deployment.

The disposable PostgreSQL test checks sparse bounded ranges, marker detection,
inconsistent binding, empty/invalid input and unchanged stored content. It runs
under the existing E-08 filter in the foundation PostgreSQL lane.
