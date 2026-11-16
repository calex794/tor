# DROP-R4b2 — private immutable Zstd seekable PACK writer

**Status: Draft / feature inactive.** Requires merged R2/R3a/R3b/R4a/R4b1 (#963, #966, #967, #968, #974). No runtime startup, no reader cutover and no production migration.

## Contract

1. B owns the immutable list of FREE edition identities and source hashes. A trusted offline worker gathers each body's exact bytes from its verified original SINGLE object, never from a browser-supplied manifest.
2. `tor-storage::services::free_pack_write::prepare_free_pack` preflights edition IDs/hash/size/UTF-8, duplicate identities, total decoded budget (128 MiB), member count (4096), compressed frame bounds, PACK (16 MiB) and manifest (1 MiB). It independently compresses each edition using Zstd level 3, creates a **standard seekable Zstd** footer with exact offset table and a complete JSON sidecar. The whole preflight then runs the existing R3b `verify_full_pack` against authoritative edition metadata. This CPU-bound step must be executed in a bounded blocking worker, not on the async reactor.
3. B freezes the exact complete descriptor and fingerprint through R4b1 `schedule_pack_job`, then claims an epoch. The candidate objects are not yet active.
4. `S3FreePackWriter::upload_and_verify` uses conditional `If-None-Match: *` PUT to both private PACK and sidecar keys, never overwriting either. Conflicting writes and ambiguous network ACKs are NOT success signals: a fresh independently bounded GET of both objects must satisfy full SHA256, exact sizes, Zstd table, identity and every decoded frame. A missing/wrong/corrupt object fails closed.
5. Only after an exact comparison of the independent verified G receipt to the preapproved B job does `upload_one_approved_pack` call R4a `finish_pack_job`. The B transaction rechecks the lease epoch, frozen fingerprints, admin holds and original SINGLE receipts. No S3 network call happens within the PG transaction.
6. The physical PACK remains a **candidate**. The active placement pointer, saved V1, live S349 path and paid encrypted content remain unchanged. Orphan cleanup remains separate.

## Evidence / negative testing

- Unit: deterministic seekable encoding, mixed Unicode editions, duplicate IDs, forged source hash, invalid namespace and cumulative decompression limit.
- Combined disposable PG18 + private MinIO: real SINGLE publish, preapproved PACK, B job claim, conditional S3 PUT, independent full GET and atomic candidate attachment; replay must keep same pack ID with no active pointer.
- MinIO ambiguous-ACK simulation: first exact object PUT already committed, second `If-None-Match: *` fails, independent GET recovers only the byte-identical PACK; wrong pre-existing object must not be overwritten and never yield a verified receipt.
- R4b1 keeps negative tests for tampered S3 and cancelled lease.

**Scope remaining:** background worker orchestration/heartbeat and lease theft during upload/PG outage tests (R4b3), verified provider rollout, actual C-authorized R5 Range Gateway and feature-gated reader cutover. No VPS/production S3/working PG changes are authorized by this stage.

[Notion «Дроп»](https://app.notion.com/p/41a32a84beec816db12deb24688716d6).
