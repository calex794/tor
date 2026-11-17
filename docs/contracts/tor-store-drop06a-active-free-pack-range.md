# DROP-06a — C-authorized ACTIVE FREE PACK HTTP Range gateway

Implements the next original **DROP-06** vertical slice after accepted DROP-04a/04b. **The gateway is independently mountable for disposable HTTP/PG18/MinIO acceptance only, not wired to main.rs or the existing S349 browser reader.** No production rollout.

## Exact trust chain

1. C uses a dedicated active-PACK read reference query, with the current S349 free-publication/saved-pin policy (session status, book and chapter visibility, age-level grants, moderation holds, deletion/lifecycle, release time). A PACK-direct edition has **no SINGLE receipt**, so a fake SINGLE row must never be created just to make it readable. The existing S349 SINGLE SQL and grant continue unchanged.
2. B requires **the exact active generation** from `free_edition_active_storage`; a mere candidate generation is never sufficient. B additionally requires a succeeded R4a repack job, or a `succeeded` S023 direct-import job whose immutable `free_pack_direct_import_stages` receipt matches the pack hash, key, bucket and **exact frame offsets/lengths/IDs**. All source hash and size fields must match the immutable B `chapter_versions` identity.
3. G issues one *private server-side* S3 Range GET for only that bounded compressed Zstd frame. The trusted provider verifies the returned Content-Range/body length/total/encoding and G decodes only that single independent frame with bounded CPU/memory, exact UTF-8 and SHA256. Never return a signed whole-PACK URL, S3 credentials, object name or neighbor bytes to the browser.
4. C/B repeat authorization/physical resolution after S3 I/O. The HTTP layer only returns `text/plain; charset=utf-8` and the exact authorized version ID, byte count and SHA256. `Cache-Control: no-store`, `nosniff`, `no-referrer`, deny framing, no CORS permission. Reject caller-supplied `Range` or `If-Range` and cross-site fetches. The body is limited to 200 KiB.
5. The route `GET /api/v1/chapters/{id}/free-pack-source[?mode=saved&version_id=…]` lives only in `tor_web::free_pack_http::router`; it is **not mounted by the runtime**. Invalid or absent ACL/active proof yields 404, PG/S3 outage yields 503 and switched placement yields 409. No fallback to an unverified or previously published neighboring frame.

## Integration acceptance

Extend the existing disposable PG18+MinIO `drop_04b_direct_pack_finalizer_is_atomic_replayable_and_stays_private` test: after the internal atomic direct-PACK commit, publicize the same otherwise private draft book in the isolated fixture, independently mount `free_pack_http::router`, and GET both distinct Unicode chapters. Confirm exact decoded bytes and sha, successful HTTP response headers, and **no** possible SINGLE grant. Refuse unsigned saved version, arbitrary client Range/whole-PACK request, cross-site request, moderator hold, anonymous restricted-age book and hidden book. All tests run without live routes or real provider migration.

## Still required by original DROP-06

- Browser transport negotiation with explicit PACK source mode, bounded body digest validation, strict version/saved V1 behavior and no silent insecure SINGLE fallback; mobile/desktop, UTF-16 progress/anchors/comments.
- Saved free V1 under historical conditions, account grants/revocation during Range, in-flight generation replacement and full browser Chromium E2E, plus paid V1 remains independently authorized/unchanged.
- Explicit operator-only deployment feature gate and live HTTP merge, followed by guarded S023 PACK-direct worker cutover and original DROP-04 100k chapter benchmark/DR/fault injection. Until then KEEP default SINGLE reader and importer untouched.

[Original plan](https://app.notion.com/p/41a32a84beec816db12deb24688716d6).
