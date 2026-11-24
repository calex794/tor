# S3-ADMIN-02a — prepared credential snapshot, no activation

Depends on accepted #1190 / #1188. Stage 1 saved one inert profile. Stage 2
must support coherent dynamic activation without a restart, not changing
the environment of a running process.

## This slice
- New saves use envelope V2: the fixed provider/purpose and exact bucket are
  authenticated as AES-GCM AAD, with a random nonce. Existing V1 rows remain
  metadata only and require both keys to be re-entered.
- A backend-only loader accepts an independently selected expected bucket;
  validates envelope version, lengths, integrity and credential shape;
  returns zeroizing, non-serializable candidate credentials and a revision.
  No missing/invalid profile fallback, no HTTP endpoint, no runtime switch.
- Isolated PostgreSQL + MinIO HTTP acceptance uses synthetic keys and an
  explicitly synthetic master key. No request to Yandex, real objects or VPS.

## Remaining activation acceptance (separate changes)
1. Inventory current startup consumers: FREE SINGLE/reader, PACK
   reader/writer/planner/cutover, paid publisher/grants, covers and import
   staging. Preserve independent opt-ins, object locators and historical
   readers. Do not mutate AWS environment variables or route objects to a
   different bucket on a credential edit.
2. Create durable immutable candidate/active generations, distinct rollback
   snapshot, exact-revision CAS and live-admin authorization in transaction.
   Never treat a mutable singleton or HeadBucket as an activation proof.
3. Verify fixed target and provider policy, reader/CORS/Range and immutable
   write/denied delete behavior on disposable Yandex acceptance resources,
   including independent restore of PostgreSQL, objects and master key.
4. Use one coherent request/job-scoped runtime snapshot and preserve old
   in-flight grants. Test concurrent saves, activation, process failure,
   lost acknowledgement, rotation and rollback; fail closed on corruption.
5. Production release and live provider changes remain separately authorized.
   No active flag, deployment, live migration, physical deletion, or automatic
   activation belongs to this slice.
