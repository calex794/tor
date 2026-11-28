# AUTH-PROFILE-06b — bounded canonical snapshot repair

A-06/E-08, S004/S015. Stacked draft on the read-only inventory #1384.
No runtime caller, public endpoint, automatic task, migration or production use.
Acceptance of the inventory and full tests of this slice are required first.

Only marker-bearing system notification rows can enter the write transaction.
Reuse the inventory's static markers and eight event/category/binding predicates.
Additionally require version-1 source events and their actual user_notifications
delivery receipt. Reject invalid object IDs, friendship targets inconsistent with
the source actor and chapter targets inconsistent with the source aggregate.
No changes to user-authored text or event rows. Other producers and unmatched
markers remain separate inventory/review work.

The operator supplies the same bounded primary-key interval as the inventory.
A transaction captures at most 100 exact IDs, locks eligible snapshot rows in ID
order, builds replacements with the existing actor renderer and updates title,
body and URL in one statement. The active public name and claimed handle come
from the current actor, never the stale snapshot. A missing/inactive safe name
falls back to the existing neutral label. No recipients, actors, source event
IDs, categories, timestamps, read flags, delivery receipts or permissions change.
Existing read-time visibility guards remain authoritative.

Local row-lock and statement timeouts are 500 ms and 2 s. A timeout rolls back
the entire batch; never skip locked candidates or advance an errored cursor.
The high watermark is not a cross-transaction snapshot; run the read-only
inventory again after a pass and resolve unmatched/malformed rows separately.
A second repair of a successfully replaced snapshot is a no-op. No raw text,
mailbox or user IDs are returned by this internal API.

The PostgreSQL E-08 test covers real mail/friendship source events, lock
contention/rollback, mismatched actor binding, bounded batches, metadata
equality, canonical stored URLs, recipient scoping and replay. No production
scan, cleanup or deployment. Logical replacement is not physical removal from
WAL, backups, logs or exports; those require independent approved procedures.
