# DROP-07b — bidirectional FREE S3/PostgreSQL dry-run, no GC

Status: **Draft**, stacked on [DROP-07a / PR #1021](https://github.com/calex794/tor/pull/1021). TOR has no pre-existing catalogue; this is a greenfield storage lifecycle.

The intended output is a **bounded operator-only evidence report**. Nothing here is connected to public HTTP, startup, active placement, an automatic schedule, an object DELETE or the existing G-09 PAID GC.

## Read-only directions

**B → G:** Read up to 128 immutable expected objects per call from PostgreSQL S349 SINGLE receipts, committed PACKs and sidecar manifests. For each, perform an independent private S3 GET with bounded full-byte length and SHA-256 verification. Report only counts: verified, wrong length, wrong SHA and unverifiable. An ambiguous 404, 403 or network outage is *unverifiable*, never permission to delete. An opaque internal PG keyset cursor permits continuation.

**G → B:** Read up to 128 private S3 keys per LIST request, separately for the two fixed namespaces `private/free-chapters/` and `private/free-packs/`. Require a well-formed provider pagination token and strictly ordered, bounded metadata. PostgreSQL classifies every listed object as:

- `Registered`: immutable SINGLE, PACK or manifest has a canonical receipt. This includes old retained SINGLE copies and PACKs that are not currently active.
- `ProtectedPending`: immutable pack-direct import stage, or a scheduled/running/succeeded R4a PACK job whose trusted candidate spec references this object or manifest.
- `Unknown`: no matching receipt in the **current bounded lookup**. Unknown is never an orphan or a delete candidate.

Time since S3's last-modified timestamp only subdivides UNKNOWN into recent, older than 24 hours, or uncertain/missing/future clock. Even older UNKNOWN objects may belong to in-flight PUTs, pending import, snapshots, saved V1, backup or crash recovery. This age is an audit hint, not a retention/quorum check. No candidate-delete flag is produced.

Both directions use a private, internal token/cursor and return aggregated counters; never expose keys, digests, chapter content or provider tokens to public readers.

## Crash/replay and snapshot limitations

The PG registry and the S3 LIST are not a transactionally consistent snapshot. New keys can appear between pages or in prior lexical ranges; full traversal can therefore *not* prove physical completeness. A verified row can change state between B scan and G GET; evidence is anchored to the immutable receipt, not current reader rights. No state or checkpoint is persisted in this slice. A future reviewed operator workflow must persist namespace+bucket+opaque token+start/end instants, enforce a finite retry budget, and redo complete cycles before making any reclamation decisions.

## Verification and next gates

- Real disposable PostgreSQL 18: registered SINGLE/PACK/manifest, pending R4a PACK and pending sidecar protected, unknown deliberately undecided, duplicate/invalid observed keys rejected.
- Real disposable MinIO: SINGLE namespace pagination, distinct PACK/manifest listing, strict max keys and invalid bucket, no DELETE.
- Pure aggregation: protected, recent unknown, aged unknown, future timestamp and missing time remain counts only; none can produce a delete instruction.
- CI exact-head and post-merge must pass. There is no VPS deployment and no productive migration.

**Remaining original DROP-07:** saved-free V1 and external backup reference enumeration, consistent checkpoint/provenance and repeat-cycle reconciliation, sanitised dashboard with explicit observability, quarantine/retention/quorum validation, and separately security-approved opt-in GC. Those are deliberately not completed by this Draft.

[Original plan](https://app.notion.com/p/41a32a84beec816db12deb24688716d6).
