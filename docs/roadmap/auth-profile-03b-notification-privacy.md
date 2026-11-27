# AUTH-PROFILE-03b — system notification identity snapshots

The existing in-app bell persists actor display names in the body and in friendship URLs.
Repairing the account display name alone cannot protect these previously stored projections.
The event remains canonical and its actor/recipient IDs, permissions, timestamps and receipts
are unchanged. For the eight actor-name-bearing source event types, the same Rust renderer
is used by delivery and by the read path when stored text or URLs contain mailbox markers.
The read side resolves the current public identity using the stored event and actor IDs,
not the old body. Unavailable or mailbox-like names become a neutral actor label;
unavailable friendship targets return to notifications. Ordinary snapshots stay unchanged.
Chapter visibility and transfer validity checks remain in the existing SQL policy.

The regression test creates real mail and friendship events, injects a synthetic old mailbox
snapshot, and verifies that the recipient receives the same safe notification and target,
while unrelated accounts cannot read or acknowledge it. This change does not modify
persisted historical notification text, general user-authored text or past access logs.
Physical cleanup of retained snapshots and audit of other producers are separate steps.
No deployment or external mail activation.
