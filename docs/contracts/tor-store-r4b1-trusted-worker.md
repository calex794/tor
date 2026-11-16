# DROP-R4b1 — trusted offline full S3 proof → B candidate attachment

**Status: draft, inactive, test-only.** Builds on merged [R4a #968](https://github.com/calex794/tor/pull/968), not the superseded [#971](https://github.com/calex794/tor/pull/971). Do not enable active storage generations.

## Boundary

- A trusted pack builder supplies an exact private target and complete ordered immutable FREE edition inventory (never an HTTP user-supplied receipt).
- B checks each edition against immutable PostgreSQL source metadata and existing S349 SINGLE receipts. It freezes the JSON spec and fingerprint in `free_pack_job_specs`, linked to the existing `free_pack_jobs` queue. The spec is insert-only.
- Claim uses `FOR UPDATE SKIP LOCKED` and increments `lease_epoch` on each takeover; renewal requires the exact live epoch. Lost or expired claims cannot finalize.
- The internal web coordinator reconstructs the G proof target from the frozen B claim, calls `S3PackProofStore::verify_independent_full_get` to read PACK+manifest from private MinIO/S3 and validate *every* frame, and compares the resulting neutral receipt to the approved spec.
- **Only after full independent readback** does the coordinator call B's `PgFreePackCatalog::finish_pack_job`, preserving R4a one-transaction candidate attachment and lost-PG-ACK replay.
- No HTTP route, no worker startup, no direct signed PACK URL, no `free_edition_active_storage` change, no paid text or publication mutation, and no deletion.

## Evidence gate

The disposable `tor-web --features s3-free --test drop_r4b1_pg_minio` uses PG18 `tor_ci` and a loopback MinIO bucket. It uploads real SINGLE sources, creates a standards-compatible Zstd seekable PACK and manifest, uploads both to private storage, freezes the exact B spec, claims an epoch, full-GETs and verifies all frames before B commit, checks duplicate replay and unchanged active reader pointers, then tests corrupt PACK and cancellation fail-closed.

This slice deliberately assumes the immutable PACK and manifest were uploaded beforehand. It does **not** implement production pack building, automatic source grouping, S3 conditional-write retry, unattended scheduler startup, or a browser reader. Those remain R4b2/R5. No benchmark or cloud-provider cost claim follows from this test.

## Follow-up

1. Produce immutable PACK/manifest in a bounded G writer with conditional PUT and lost-ACK recovery via verified full GET; B preapproval must bind identical bytes before upload.
2. Add heartbeat during long uploads/readback, cancellation and timeout fault injection, concurrent claim takeover, PG outage and lost-commit-ACK tests.
3. When full R4b is accepted, build C-authorized R5 Range Gateway with age/holds/saved-V1 rights and negative neighbor tests; only then consider feature-flagged physical cutover.
