# I-11 / DR-06 — offline object-inventory evidence (operator-only)

Status: **code and disposable tests only**. No live inventory, external S3 access, object backup, deployment, timer or independent restore has been performed by this change. Tracking: [#739](https://github.com/calex794/tor/issues/739), [DR-03](i11-s3-keys-offsite.md), [independent DR acceptance](i11-independent-dr-acceptance.md).

## Why this exists

The existing DR-03 copy uses `rclone copy --immutable` and verifies the objects **still listed** by the source using `rclone check --download --one-way`. This cannot detect an object deleted from the source *before* the backup began, nor prove the complete historical set of purchased V1 objects. An operator must retain an independently protected **pre-incident** object manifest and compare it with the restored data and the recovered PostgreSQL object registry.

`tools/i11_object_manifest.py` operates only on **local staged directories**. It never contacts S3, PostgreSQL, TOR or a network. It does not access encryption keys, issue payment requests or alter the source objects.

## Two operator-only phases

1. **Before an incident**: on a separately approved trusted machine, stage a *frozen, known-complete* object tree from the appropriate source snapshot. Verify its exact prefix/version scope against the corresponding database `storage_objects` registry and protected historical purchases. Do not silently generate the baseline from an already depleted bucket.
2. Generate a manifest of relative keys, byte lengths and SHA-256 hashes, using an **absolute** output path *outside* the scanned tree. New output is created with private permissions and cannot overwrite an existing file:

   ~~~sh
   python3 tools/i11_object_manifest.py snapshot \
     --root /srv/dr-staging/frozen-objects \
     --output /srv/dr-private/preincident-objects.json
   ~~~

   Record the reported manifest SHA-256 **outside** the storage account, alongside the UTC snapshot identity, database recovery point and backup-set identifier. Store the manifest in a separately protected, versioned/immutable location. Its contents may disclose private object names: do not upload it to Git, GitHub Actions, Notion or public logging.
3. **After a simulated loss**: on a different trusted isolated machine, restore the *same selected snapshot scope* exclusively from the independent external archive, using restore-only credentials. Do not use live primary S3 as a fallback. Confirm that the manifest/digest came from the independently retained **pre-incident** records.
4. Check the restored tree against the retained manifest; pass the previously recorded digest explicitly:

   ~~~sh
   python3 tools/i11_object_manifest.py verify \
     --root /srv/dr-restore/restored-objects \
     --manifest /srv/dr-private/preincident-objects.json \
     --expected-manifest-sha256 <independently-recorded-64-char-digest>
   ~~~

   A successful result emits `TOR_I11_OFFSITE_MANIFEST_VERIFIED` with object count and total bytes; any missing, unexpected or modified object, unsafe path, symlink, unreadable subtree, malformed manifest or wrong digest produces `TOR_I11_MANIFEST_REFUSED` and exit code 2. Output does not include object names.
5. Independently cross-check the set of objects in the manifest against the **recovered PostgreSQL registry** and historical paid-edition references at the chosen recovery point. Verify the separately sealed KEKs, application decryption, purchased V1 after V2, refund/ACL denial and external alerts using the main DR runbook. A manifest match alone does **not** prove any of these.

## Scope and limitations

- A raw SHA-256 digest is an integrity check, **not an authenticated signature**: keeping the digest beside a mutable manifest offers no independent proof. Protect the digest and manifest via external custody/retention, preferably including independently verifiable signatures.
- A manifest generated *after* partial deletion will faithfully describe the already damaged set. It must exist before the incident and be checked against a database/object baseline and independently preserved history. A file tree scanned while it changes is not a coherent snapshot; stage only frozen/immutable input.
- `snapshot` and `verify` scan the entire selected directory recursively. The tree must represent the **same prefix, version and time scope** in both phases. Extra later revisions or unrelated backups cause a deliberate mismatch. Never compare one selected snapshot to a bucket with mixed versions.
- The tool refuses empty/all-zero inventories, symlinks, unsafe object keys, unreadable subtrees, non-regular files, duplicate manifest keys and altered object bytes. It has bounded manifest and object counts; an oversized archive needs a reviewed streaming design.
- It does not determine offsite independence, IAM separation, versioning/Object Lock, WAL continuity, backup age or RPO/RTO. No approval of #739 or S061 follows from the local verification token.
- No production operation, S3 listing, `rclone` invocation, service installation, schedule, migration or remote secret is introduced.

## Disposable regression

~~~sh
python3 -m unittest discover -s tests/architecture -p 'test_i11_object_manifest.py' -v
~~~

Local fixtures cover a valid match and refusal for missing/corrupt/extra objects, forged manifest digest, duplicate/unsafe keys, symlink paths and empty/in-tree output. This is **not** an independent offsite restore test.
