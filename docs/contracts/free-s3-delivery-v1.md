# S349: direct delivery of immutable free editions

Related scenarios: S006, S008, S013, S020, S023. Owner: B publication,
C reading authority, E source anchors, G objects/jobs, H browser, I wiring.
Tracks [#349](https://github.com/calex794/tor/issues/349); does not close it.

## Write and publish

With the `s3-free` build, free author saves and imports require the configured
S3 publisher. A missing publisher fails closed; no HTTP or worker fallback
writes the body into PostgreSQL. The service normalizes source Markdown before
hashing. PostgreSQL keeps IDs, SHA256, byte length, edition pointers and bounded
UTF-16 block lengths. New free `chapter_versions.body_markdown` is NULL.

B reserves immutable edition IDs before the S3 upload. G performs an immutable
PUT and a bounded GET to verify actual bytes, Content-Type, length and SHA256.
Only then may B attach the receipt and publish the edition in one transaction.
Immediate first release emits the existing canonical event once. A staged
edition has no reader pointer until the existing release/scheduler transaction.
Editing keeps the previous reader pointer until verification and optimistic
revision checks succeed. Existing free pins retain their exact V1 reference.

Batch imports reserve IDs outside the live chapters table. Object checkpoints
allow retries, but checkpointed objects are read and verified again before
finalization. All live chapter/version/shape/receipt/publication rows commit
together. Queued imports additionally lock and validate the current G job,
lease epoch, expiry and approved fingerprint in that same transaction, together
with the succeeded state. Cancelled or stale work cannot occupy chapter numbers.
Failed uploads can leave private provisional objects/metadata, never a public
chapter. Removing those provisional objects is separate storage GC work.

## Read and revoke

`GET /api/v1/chapters/{id}/free-grant` resolves the current free publication
under current book/chapter visibility, holds, release time and session age
permissions. G checks object metadata and signs a private GET for 90 seconds.
C rechecks the exact authority and edition after signing. Responses are private,
no-store and contain metadata only, never chapter text.

`?mode=saved&version_id=…` requires an active session and its exact free pin.
It can read that V1 after a new publication, free-to-paid change or ordinary
deletion, subject to the existing pin, age, hold and hard-deletion rules. It
does not grant the current edition or anybody else's pin.

Administrative current/history reading keeps the existing session, role,
Origin, CSRF, rate limit and immutable audit checks. For a free S3 edition,
the privileged POST returns a short verified capability instead of plaintext.
The administrative browser hashes the direct S3 response before rendering.
Moderators and revoked administrators cannot obtain this capability.

The reader HTML contains metadata/navigation and a loader. The browser fetches
only from the explicitly configured storage origin, omits credentials/referrer,
rejects redirects, bounds the stream and verifies SHA256 before constructing
safe Markdown DOM. Progress and explicit completion use B's immutable source
shape with C's existing ACL/CAS transaction. Quote corrections and anchored
comments use a separately authorized, verified server-side source read; E
rechecks its immutable hash and rights before accepting an anchor.

The no-JavaScript link uses the same authority with `&format=source`, then a
303 directly to private S3 Markdown. It does not proxy bytes through TOR.
Search engines receive title/navigation metadata and can use the same loader;
formatted server-side chapter text is deliberately absent from the HTML.

Hide, hold, free-to-paid and deletion stop new public grants immediately.
An already issued capability remains valid for at most 90 seconds; downloaded
text cannot be revoked. This path does not publish a public CDN object or
introduce an anonymous long-lived text cache.

## Runtime and acceptance

Build with `s3-free` (alongside the existing S3 features). Configure
`TOR_FREE_S3_BUCKET` as a private bucket and `TOR_FREE_S3_PUBLIC_ORIGIN` as the
reader-reachable origin. Standard AWS provider credentials/region are used.
The bucket must permit CORS GET from the site origin; CSP permits only that
configured origin. Startup and this change do not create buckets, apply
migrations or deploy anything. There is no historical-text migration or dual
write. Existing paid encrypted delivery and cover delivery remain separate.

Mandatory CI includes B lifecycle PG tests, immutable G adapter tests against
disposable MinIO, and the native author/browser journey in
`free_author_s3_pg`. The latter checks mobile/desktop direct S3 requests,
absence of a source marker in TOR reader/grant responses, CSRF, replay,
corruption, hold, unavailable storage, edit V2, saved V1, anchored comments,
corrections and audited administrative history. Browser Network records direct
S3 response sizes; the Axum socket boundary independently inspects and counts
the exact reader/grant response bytes. No signed URLs are recorded. Contract tests
exercise hostile metadata, byte limits, timeouts and safe UTF-16 formatting.
Final acceptance requires the successful integration run on the PR's exact SHA.
