# S025 / CDN-REV-01 — staging-only edge revocation acceptance

Tracks [issue #684](https://github.com/calex794/tor/issues/684) and [S025](https://app.notion.com/p/40432a84beec81ddb123ffc5dee763f0). This procedure does not authorize use of production CDN, S3, tokens or databases.

## Safety requirements

- An operator must explicitly approve use of a dedicated, disposable Yandex Cloud CDN **staging** resource, separate public S3 bucket, and synthetic WebP test cover.
- Run the probe from an approved operator workstation; there are **no network requests by default**. `--execute-readonly` allows only bounded HTTPS **GET** to the exact staging host and immutable S025 URL. No API POST/PUT/DELETE or purge is implemented.
- Never use a real book, production resource, production domain, shared origin or customer image key. Operator evidence is private. No bearer tokens, URL query strings or object bytes appear in probe output.
- The staging host must contain an explicit `staging`, `stage` or `test` DNS label. Production-looking names, redirects, query strings, embedded credentials and nonstandard ports are rejected.
- `purged` in PostgreSQL is not evidence that every CDN edge or browser cache no longer has the bytes.

## Steps

1. Verify in the Yandex Cloud console that the resource is disposable staging, configured with isolated origin. Check **browser cache lifetime** separately from **edge cache lifetime**. Effective browser `Cache-Control` must be revalidated within at most 60 seconds.
2. Create a synthetic test Book, upload a synthetic image and approve the exact candidate through the ordinary site-moderation queue. Start the operator-opt-in S025 worker in staging only. Confirm `current_cdn_copies` produces the card and main immutable paths after the durable B reservation and S3 copy are confirmed.
3. Warm the old immutable URL normally, without cache-busting query parameters. Run the read-only probe:

    python3 tools/s025_cdn_staging_probe.py --phase before --url "$STAGING_OLD_CDN_URL" --staging-host "$STAGING_CDN_HOST" --execute-readonly

    This must return HTTP 200, `image/webp`, the SHA-256 embedded in the filename, and a short effective browser TTL (`max-age <= 60`, `no-store` or `no-cache`). Missing or one-year `immutable` headers are failures.
4. Perform the replacement or `pending_deletion` through normal authorized TOR actions. Verify that the public HTML no longer exposes the old immutable URL. Check the exact old key by authorized staging S3 `HeadObject`; a permission-denied response alone is not proof of absence. Confirm the private staging original, approved current content and moderator audit are intact.
5. Verify the existing S025 worker's durable transitions for that old identity: `copied → removal_due → origin_deleted → purge_submitted → purged`. The provider request must contain exactly one path such as `/public/covers/...webp*`, with a nonempty `paths` array. Verify the Yandex operation is `done=true` with a response and no error. **Never send an empty `paths` array**: it purges the entire resource. A trailing wildcard is necessary for segmented content and certain `Vary` variants.
6. After operation completion and the provider propagation window (allow up to 15 minutes), run the normal edge URL through the probe again:

    python3 tools/s025_cdn_staging_probe.py --phase after --url "$STAGING_OLD_CDN_URL" --staging-host "$STAGING_CDN_HOST" --execute-readonly

    The after-phase must return three ordinary GET responses with HTTP 403, 404 or 410. Any 200/206, redirect, 5xx, stale image bytes or request timeout fails the acceptance. No `Cache-Control: no-cache` request override: it could bypass the normal cached path.
7. Repeat the after-phase from an independent region/network when feasible. Verify both card and main, and replacement and deletion. Preserve sanitized timestamps, short TTL evidence, object negative-HEAD proof, provider operation ID/status, and edge-negative GET outcomes in a restricted evidence folder.
8. Disable the staging worker, verify cleanup and record rollback. Previously downloaded browser-local copies cannot be forcibly erased; do not promise instant universal recall.

## Offline-only CI

The normal command **without** `--execute-readonly` returns `PREFLIGHT_ONLY` and makes zero network requests. This is a safety check, **not** real CDN acceptance.

    python3 -m unittest discover -s tools -p test_s025_cdn_staging_probe.py

This unittest runs in mandatory TOR Integration CI preflight with injected responses, no cloud or network. Real staging validation is operator-owned and must be authorized separately.

Provider reference: [Cache.Purge REST](https://yandex.cloud/en/docs/cdn/api-ref/Cache/purge), [CDN caching and separate browser TTL](https://yandex.cloud/en/docs/cdn/concepts/caching), [selective purge including suffix wildcard](https://yandex.cloud/en/docs/cdn/operations/resources/purge-cache).
