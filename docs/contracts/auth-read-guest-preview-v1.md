# AUTH-READ v1 — guest excerpt, authenticated full FREE reading

Status: product intent approved, implementation contract proposed for review.
Owner request 28.11.2026; tracking [#1349](https://github.com/calex794/tor/issues/1349).
Baseline inspected: main@19fc89aeca7f59c3786db50276e4c7ddba30c509.
No code, migration, provider enablement, release approval or deployment in this PR.
Extends B-02/C-01/C-02/C-03/C-04/E-02/F-08/G-09/H-05/I-12; does not add
a duplicate scenario registry or change the 60 active historical scenarios.

## 1. Product contract

Two independent book-level modes: `open` (default for existing and new books)
and `login_preview` (owner opt-in). The latter leaves the public book card,
cover, annotation, catalog, recommendations and all otherwise visible TOC
entries accessible. For every otherwise public and age-permitted FREE chapter,
a guest gets only a prefix and an explicit "Войдите или зарегистрируйтесь,
чтобы прочитать главу полностью" action. An active eligible account receives
the exact same complete free edition at the same reader URL, without purchase,
subscription, wallet mutation, extra chapter or waiting period. The policy
applies to existing, newly published, scheduled and imported chapters.

Owner-approved extension: `guest_open_through_chapter_no` (N) permits full
guest reading of otherwise eligible FREE chapters with 1 <= chapter_no <= N.
These chapters are eligible for the existing SEO policy, not guaranteed index.
N=0 means no fully open exception; open mode ignores N. Use the existing
positive canonical chapter_no, not rank among current free/released rows,
so hidden/deleted/paid/missing numbers never shift the boundary or backfill
with a later chapter. N can include planned future chapter numbers; only
published eligible FREE chapters qualify. No separate price rule or change
to the existing per-chapter FREE/PAID workflow. Example: chapters 1–300 FREE,
N=10, p=20 -> 1–10 full guest text + SEO eligibility; 11–300 guest excerpts;
301+ PAID remain the existing paywall. All 300 can be FREE even when N=0.

The owner chooses 10/20/30/50 percent, default 20 when enabling. These are
proposed product defaults, not user-configurable arbitrary byte offsets.
Paid content keeps the existing metadata/paywall and paid entitlement rules;
no paid plaintext excerpt is introduced. Draft, held, blocked, deleted,
not-yet-released and age-ineligible content retains existing hard denial.
Public discovery of an ordinary book is not an entitlement to its text.
Saved FREE and historical paid purchases/Layer remain independent existing
authenticated capabilities; no rewrite of D-001 logical or physical editions.

| Existing authority | Anonymous open | Anonymous login_preview | Eligible active session |
| --- | --- | --- | --- |
| Public current FREE, chapter_no <= N | Full | Full | Full |
| Public current FREE, chapter_no > N | Full | Prefix only | Full |
| Public PAID, no paid right | Existing metadata/paywall | Existing metadata/paywall | Existing metadata/paywall |
| Pinned purchased V1 | Deny | Deny | Existing exact purchase check |
| Saved FREE V1 / Layer | Deny | Deny | Existing exact historical check |
| Age/hold/private/release denial | Existing deny | Existing deny | Existing deny absent independent authorized exception |

Owner and administrator preview routes retain their separately scoped rights.
Moderation or ordinary member role is not a new bypass. Googlebot and other
robots are guests; User-Agent never changes the decision. Expired/revoked
cookie and inactive account do not count as authenticated. PG uncertainty
returns unavailable, not an anonymous full-text fallback.

## 2. Boundaries and persistence

- B owns author policy and exact edition projection; C owns the reading
  decision; A supplies a live Actor, E checks discussion/quote rights, F
  consumes public visibility and indexing facts, G supplies verified bytes,
  H renders the result, I coordinates schema/runtime/RIL acceptance.
- Extend existing crates and ports under `crates/contracts`; do not create a
  separate ACL service, duplicate session store, second S3 authority or new
  cross-domain dependency. HTTP must not contain SQL authorization.
- Add an additive B-owned policy on books: `guest_read_mode`,
  `guest_preview_percent` (enumerated 10/20/30/50, default 20),
  `guest_open_through_chapter_no` (INTEGER >=0, default 0; same i32 upper
  bound as chapter_no), `guest_policy_revision` (non-negative monotonically increasing BIGINT).
  Existing records resolve to open. New migrations use current SQLx registry,
  not historical edits. Names are contract targets, not existing columns.
- Owner-only change through a dedicated field-scoped CAS command. Derive
  actor from live session, verify book ownership and applicable existing
  lifecycle/hold policy inside the same transaction, require Origin/CSRF and
  expected revision. Reuse author management UI and existing audit patterns.
  Stale revision -> 409; no-op does not increment revision; no cross-user
  access, no implicit setting changes on ordinary edits. Audit book ID, actor,
  old/new mode, N, percent, revision and UTC time; never log source, session
  or URL. Validate the entire policy atomically. Changing N/percent cannot
  mutate access_level, price, publication, version ID or wallet. Emit one
  idempotent B-owned SEO invalidation for each committed changed policy.
  Delegating the right to editors is deliberately out of v1 scope.
- Suggested native endpoint: POST /me/author/books/{id}/guest-reading
  (exact owner route prefix to be aligned with actual main at implementation);
  GET settings via existing author book workspace, not a second page.
  Register every new actual route in RIL. No price, revenue, entitlement,
  version-count or storage placement change from this command.

## 3. Materialized excerpt (not a second content authority)

B derives a small read projection from the exact normalized, verified
publication source, using the existing restricted reader renderer's
tokenization/visible-text rules. Factor a shared pure helper; no independent
regex Markdown parser, raw HTML truncation or script injection. v1 output is
escaped plain text with paragraph breaks, no active links, inline images,
media keys, source positions or quoted hidden tail. Link destinations are
not visible prose. No asset fetch for guest preview.

Define the versioned algorithm GUEST-PREFIX-1 over the canonical visible-text
stream. Count Unicode scalar values N after normalization; count no Markdown
formatting/destinations. For percentage p use L=min(floor(N*p/100),2000,N-1)
with saturating arithmetic for empty text. Cut only at a UTF-8/grapheme-safe
boundary at or before L; prefer a nearby paragraph/sentence/word boundary
when available without exceeding L. Never append ellipsis from the source
tail; UI ellipsis is separate. N<=1 or no safe nonempty boundary -> CTA
without text. Bound both scan (existing chapter input limits) and projection
(<=2000 scalar values and <=8000 UTF-8 bytes). A short chapter is never
promoted to full access merely because it fits the maximum.
02a uses the existing restricted Markdown tokenizer moved to a shared pure
contracts module with an unchanged reading facade and an alternate visible
text sink. Normalize whitespace within blocks and preserve paragraph breaks.
Pin unicode-segmentation 1.12.0 for the versioned grapheme rule. If literal
image/link destinations or HTML-like markup survive the historical tokenizer,
v1 conservatively returns CTA-only instead of guessing at unsupported syntax.
The internal maximum prefix uses a strict grapheme cap; nearby natural
boundary preference is applied to the selected guest percentage, not to
the stored maximum. No full source or full visible text is retained by the
result. This slice is pure only; exact version/SHA persistence and safe
HTML escaping of the selected plain text remain required.

Store one bounded derived maximum-prefix projection (p=50) per needed exact
FREE edition, not four copies or a full chapter. Candidate shape:
chapter_id, version_id, immutable source_sha256, algorithm_version,
visible_scalar_count, prefix_text, prefix_sha256, created_at. Unique exact
edition/algorithm and B-owned FK/constraints; bind source SHA to the immutable
edition, verify stored length/hash and publish only the needed p prefix
(server side). The 50-percent projection is internal, never a guest API
payload. Prefix and N must originate in the same verified source extraction.
Re-cut with same boundary rules at requested p; increasing percent reveals at
most the stored prefix and never fetches the complete chapter per guest read.
Do not expose N, internal projection digest, full source digest, byte offsets
or content plan to guest as part of the preview response.

02b persists one bounded derivative keyed by chapter_id, version_id and
algorithm_version with exact source and prefix SHA-256. A trusted B/G worker
must supply verified bytes for the current FREE edition, never an actor or
browser-provided arbitrary version. Verify book policy revision, cutoff,
publication and content hash under the book/chapter/publication locks before
committing; stale or mismatched work cannot attach. Legacy NULL content_sha256
is verified from the immutable PG body only during preparation, never during
guest delivery. Exact duplicate preparation is idempotent; inconsistent
existing rows fail closed. Derivative rows may be removed/rebuilt without
changing an edition; they add no independent retention or entitlement.
The D-001 installed foreign-key inventory explicitly classifies the exact
validated composite FK with ON DELETE CASCADE as derived-guest-prefix.
The regression checks logical, reader-retained and physical edition counts
are unchanged by projection preparation. This classification does not grant
physical deletion authority, change existing pin checks or modify GC.
Automatic publication/import/schedule and fenced backfill wiring remain a
separate slice; no read route or activation is introduced by this storage slice.

02c1 adds a bounded preparation discovery port. One request checks the
current prepared book revision, then scans at most 64 consecutive existing
chapter numbers beyond max(N, cursor), using the unique (book_id,chapter_no)
index and a materialized limited page. Apply publication, hold, FREE, release
and exact-derivative checks after the limit. The cursor advances over inspected
numbers even when all are ineligible or already prepared; a short/empty page
ends the sweep. No source body, provider request or full-catalog scan.
Discovery is advisory: another writer may change eligibility before
preparation; prepare_current still enforces the exact revision/source.
02c2a uses the existing outbox_events, not a new job queue. A bounded
missing-page producer rechecks current candidate facts in one INSERT/SELECT
and enqueues a distinct GuestPrefixPrepare event for guest_prefix_prepare_v1.
Payload is exactly book/chapter/version/revision/algorithm plus the registered
source digest (NULL only for legacy PG editions without a stored digest).
No source text, URL, storage key or session. One dedupe key per exact target
and revision; duplicate sweeps cannot enqueue duplicate events. Discovery
and enqueue are advisory, so a concurrent change is still rejected at the
consumer's write. A subsequent reconciliation pass starts a new keyset
sweep; delivery of a stale event is not proof of any current projection.
The fenced preparation port validates the actual claimed event and payload;
after locking book/chapter/publication, it locks that same live outbox row,
checks epoch, deadline, expected digest and unacknowledged consumer, then
rechecks the lease before committing the immutable derivative. A successor
cannot claim the locked row mid-commit. A lost acknowledgement permits an
identical idempotent retry via PgOutbox, not a second derivative. Unfenced
prepare_current is retained only for trusted direct preparation and tests;
the background consumer must call prepare_claimed. The existing G consumer
owns acknowledgement/retry. The next slice must bind source retrieval,
obsolete-vs-retry classification, verified SINGLE/PACK I/O and worker
dispatch; automatic scheduling, dead-letter recovery and repair of a
missing previously delivered derivative remain explicit follow-ups.
Unsupported literal markup is persisted as an immutable terminal
projection_status=unsupported with empty text, zero count and the exact
source hash; a valid empty/tiny projection is status=ready. Retries only
accept identical results. A new exact edition is eligible for new work.
Missing/corrupt data, unlike unsupported, remains a failure/rebuild case.
No new FK or retention right, no public API and no rollout activation.

New bodyless FREE full Markdown remains in verified S3 only; legacy PG text
retains existing migration semantics. The projection is a small intentional
derivative, rebuildable from source, never a substitute for source or an
additional retained logical edition. B/G derive at verified save/import
finalization and bind to the exact version before it becomes reader-visible.
For a restricted book, a new published edition must have verified projection
or remain without excerpt (safe CTA-only failure); prefer atomic attachment
alongside publication where existing job contracts permit. Never display a
stale V1 excerpt after V2 is current; no use of candidate/working draft
before release. The publication authority selects exact current version.

Backfill existing editions by bounded book/chapter keyset jobs only on opted-in
books (including an optional prepare phase); prioritize chapter_no>N, and
schedule newly restricted numbers when N decreases, reuse verified legacy PG/SINGLE/
active PACK source; no public grant to the worker. Use existing G lease/epoch,
idempotent exact version/hash upsert and bounded retries; no whole catalog
scan on owner toggle, no S3 calls from guest request path, no new generic job
runner. A concurrent edit/superseded job cannot attach to the wrong edition.
Missing, invalid or corrupt projection -> no body, honest unavailable
excerpt and login CTA; never full text, prior projection, unverified bytes
or an on-request full chapter fetch. Retain/delete derived rows only under
existing edition lifecycle rules; no new physical GC. DR restores/rebuilds
projection safely from exact verified content, never broadens policy.

## 4. Authoritative reading decision

C must distinguish `FullFree(exact edition, policy revision, transport)`,
`GuestExcerpt(exact edition, policy revision, bounded excerpt)`,
existing `PaidMetadata/Entitlement`, and `Denied/Unavailable`.
Use a single canonical PG predicate/typed contract for policy + live session;
keep existing publication, age, moderation, ownership and hard-denial facts.
For otherwise eligible current FREE, anonymous full = mode=open OR
(positive chapter_no <= N); in login_preview beyond N guest receives excerpt.
An active eligible session obtains full FREE regardless of N. Resolve live
chapter number and policy revision with exact publication facts in PG. The
same anonymous full/preview fact (never the current request's signed-in
state) is supplied to F SEO. Unknown policy, invalid numbers and lookup errors
never produce full text or positive SEO eligibility.
Do not infer authorization from a cookie's presence, an empty body_markdown,
a TOC row, query parameter or client-side state. Repository selection for a
guest excerpt must not fetch full Markdown or the full S3 object.

Existing code anchors on inspected main:
- `crates/reading/src/repositories/postgres.rs`:
  public_chapter_for_session currently selects body_markdown; basic bypasses
  session age checks. Separate typed preview from full body at SQL boundary.
- `crates/reading/src/repositories/free_s3_ref.rs`:
  for_session and for_active_pack_session currently admit public/basic
  guests. Apply full decision before obtaining object reference.
- `crates/web/src/public_reader_http.rs` and pages/reader_free.rs:
  distinguish excerpt from a bodyless S3 shell. Never invoke full loader,
  issue progress CSRF/permits or load chapter comments for a guest excerpt.
- `crates/web/src/free_reader_http.rs`:
  /free-grant with and without ?format=source, GET/HEAD, /free-open.
  Reuse verified same-origin source path and verified_grant post-I/O check.
- `crates/web/src/free_pack_http.rs`:
  /free-source-plan, /free-pack-source, /free-source and saved parameters.
  Restrict plan/source before any Range call; no whole-pack URL ever.

Inventory actual mounted route registry, JSON /read/reader aliases, fallback
and no-JS routes, progress/sync/reading position, archived FREE, layer and
admin/author paths, direct version, media, anchor, correction, quote,
comments, mail/notification/search snippets. Deny anonymous raw quote/comment
endpoints for restricted chapters in v1; book-level discussion unchanged.
Guest excerpt has no source offsets, hidden comment anchors or chapter
illustrations; existing public cover remains. Chapter assets must not be
publicly retrievable outside the matching full ACL, or mark that book
ineligible for activation until verified private delivery is available.
No arbitrary version selection, offset, range or client percentage override.

Recheck exact edition, policy revision, source digest, active storage
generation and live ACL after S3 I/O/signing before release. Adapt existing
verified_grant comparison and PACK post-I/O revalidation; do not make
authorization cache a separate source of truth. Stale state -> 409 or
existing deny; unavailable PG/S3 -> 503. Recheck full eligibility before
issuing progress evidence. Test mode switch during source read and session
revocation during signing/Range. No fallback from denial to legacy full.

## 5. Storage delivery and transition safety

Open FREE books and eligible open-prefix chapters keep existing S349 and
PACK behavior. For login_preview chapters beyond N an eligible ordinary full reader uses bounded same-origin, no-store
verified SINGLE source or existing PACK Range gateway (both reauthorize
each request). Do not issue a transferable signed full SINGLE URL even to
a logged-in ordinary reader in restricted mode; an anonymous client could
otherwise replay a copied URL. Direct PAID ciphertext/WebCrypto and existing
separately scoped historical/privileged access contracts are unchanged.
Historical/privileged capabilities have their existing short-TTL residual
risk; they must never be obtainable by a guest or by manipulating ordinary
public routes. An authorized reader can still copy downloaded text; this
is a login gate, not DRM.

This choice intentionally trades some bandwidth/CPU for immediate per-request
session enforcement on restricted FREE chapters only. Use existing source
semaphores, timeouts, size bounds and private Range path. Avoid holding PG
connection while performing S3 I/O. Browser handles a distinct authenticated
same-origin full transport; never fall back to signed-single for a restricted
edition. No new public full-object bucket or copy. Benchmark before activation
and evaluate a session-bound gateway later only with equivalent guarantees.

Open -> restricted (also decreasing N or moving a chapter beyond N): first
commit policy revision so no new guest full access or signed URLs can be
issued for newly restricted chapters by any serving process. Recheck number,
revision and eligibility after storage I/O. Increasing N opens only currently
eligible FREE chapters, never PAID/held/age-restricted ones. A prior S349 bearer
can work for its remaining <=90s TTL; earlier downloaded text is irrevocable.
For a previously public origin/CDN object, TTL alone is insufficient:
verify private origin, revoke/purge old full public access, demonstrate ordinary
anonymous negative GET/HEAD/Range from an independent network. Require
all serving binaries/jobs to be policy-aware before allowing owner activation,
and reject or mark transition pending if legacy public delivery or unbounded
cache remains. Do not promise instant deletion of third-party copies.
UI must distinguish active gate on new requests from residual transition
window until previous capabilities expire and edge evidence is confirmed.
Re-open/rollback are explicit audited owner actions; rollback binary or global
feature OFF must never interpret stored login_preview as open. Keep deny/preview
guards active, disable new author mutations rather than bypass existing gates.
Refuse rollback to an unaware binary while any restricted policy exists.

Every chapter response with actor-dependent text (including excerpt, errors,
source and grants) must be private,no-store; Vary Cookie where relevant,
no shared cache, ETag/304 or service-worker fallback carrying full text.
Do not rely solely on browser-side hide or robots.txt. Ensure Nginx/CDN
bypass, no cross-site content or Range bypass and no full source in SSR
hydration, devtools responses, logs, metrics or signed URL metadata.

## 6. Native UX, SEO, observations

Book workspace: "Доступ к бесплатным главам" -> "Всем" / "Полностью после
входа"; show "Первые N глав открыты всем и могут индексироваться" (0 allowed),
10/20/30/50 selector with 20 default, absolute limit hint, actual guest example,
counts of currently open/preview/paid and transition status. Explain that N
is chapter number cutoff, not a way to make paid chapters free. Save one CAS
command; warn when reducing N that old search results/issued URLs may persist. In reader:
normal title, TOC, paragraph fragment, CTA "Продолжить чтение бесплатно".
Reuse AUTH-UI login/register and verified same-site return-to exact chapter;
no arbitrary return URLs/open redirects. Login reloads same chapter and
full reader; logout or expired session reloads restricted reader as excerpt.
Mobile 390 and desktop 1440, keyboard and no-JS fallback must work. No
checkout modal, wallet balance, paid lock icon or new entitlement for FREE.

### Existing SEO integration (F-08 / S060)

Use the existing [SEO-01 policy](seo-policy-v1.md), `SeoPolicy`,
`SeoChapterFacts`, `SeoDecision`, `TrustedSeoOrigin` and B/C authorization
facts. No AUTH-READ-specific sitemap, robots authority or user-agent switch.
Chapter facts require a positive verified anonymous FULL current FREE
delivery result, including N and current policy revision; login alone is not
proof of public indexing. Extend the existing pure F decision with explicit
guest-full/preview/unknown facts and reason. Preview and unknown are noindex
and excluded from all chapter sitemap projections, even in an authenticated
response. Public mode, pilot approval, global index disable, basic age,
moderation, publication and single Pandaolate/Shadow index owner remain
strict intersections. No new right to index an adult, paid, held or private
chapter. Book-level `has_public_content` remains a separate fact for the
existing public basic book + released FREE chapter gate; a book with N=0
can retain its existing discoverable card and allowed book indexing, even
if all chapter texts require login. Do not equate that fact with full chapter
eligibility. Preserve F book sitemap and recommendation behavior.

Inspected live `crates/web/src/seo_http.rs` currently publishes a bounded
book-only sitemap and book head; pure chapter SEO exists in
`crates/search/src/services/seo.rs`, not a proven mounted chapter sitemap.
Extend the same F/H pipeline with chapter facts, canonical numeric
/reader/{id} and a bounded chapter sitemap shard under the existing sitemap
index (no new competing sitemap). Use stable numeric ID/keyset or ID buckets,
existing indexed book_id/chapter_no and publication joins, apply ACL/SEO gate
before pagination. Do not duplicate free counts, use OFFSET over a catalog,
or mistake FLOW SEO analytics/external search records for index authority.
Each canonical chapter SSR response, robots HTTP/meta and sitemap row must
agree with one current F decision. Preserve global/pilot release switches.
Unknown or stale facts -> noindex, no sitemap, no canonical promotion.

Indexable open-prefix chapter text must be genuinely accessible to a normal
anonymous client at the same URL; no full text exclusively for crawlers.
For the explicit SEO chapter rollout, prefer verified bounded SSR from
existing legacy PG/SINGLE/PACK read path for all anonymous clients on those
index-eligible chapters (including no-JS), rather than rely solely on a
bodyless JS loader. Reauthorize after S3 I/O, reuse safe renderer and limits;
measure egress/p95 before enabling this route. No additional full text stored
in PG, and no S3 calls for restricted guest excerpts. Open-prefix full reading
can still use the existing full transport while SEO chapter rollout is off.
Never expose restricted tail in SEO metadata, structured data or notifications.

N/mode, paid/free, publication, chapter number, hold/age/lifecycle and SEO
owner/setting changes invalidate affected chapter candidates via existing B
outbox/G fenced job patterns; key staged SEO generations by current fact
revisions, validate again before activation and intersect even active rows
with live authority when serving maps/HTML. A stale generation/rollback may
omit newly eligible entries but must not reintroduce a newly ineligible URL.
Changing N 10->30 or 30->10 updates only affected book/range (bounded keyset),
without global rebuild or per-chapter price mutations. For narrowing, deny
full text and emit noindex immediately on current reader requests, remove
stale chapter sitemap rows through live filtering and rebuild/invalidate
shards. Keep robots.txt crawlable for public chapter URLs so bots can see
noindex; do not Disallow the route to hide the directive. Purge stale
full-text edge caches and honor previous signed bearer TTL; third-party
copies/search snippets cannot be revoked retroactively. Sitemap inclusion
is permission, not a promise of search-engine indexing.

A preview is not a full open, reading completion, paid consumption, progress
or a billable chapter ad impression. No full-open intent or completion tokens,
progress/anchor controls, reader-end ad/shelf completion proof from preview.
Reuse only permitted anonymous aggregate RIL observation for preview visits
if needed; no new IP/device/user profile collection, no synthetic conversion
numbers. Any registration conversion measurement requires separate privacy
review and clear denominator; do not claim a conversion uplift before data.

## 7. Implementation stages and acceptance gates

| Stage | Owner | Deliverable and stop condition |
| --- | --- | --- |
| AUTH-READ-01 | B/I/F | Typed shared N/percent policy and SEO full-guest gate; additive schema, owner CAS, audit, default open, tests; no user activation |
| AUTH-READ-02 | B/G/C | Pure bounded excerpt builder and exact-version projection, legacy/import/schedule/backfill; no full fallback |
| AUTH-READ-03 | C/I | All full-reader and transport gates, same-origin SINGLE/PACK, post-I/O recheck, ancillary ACL; no owner activation |
| AUTH-READ-04 | H/A/E/F | Native N settings/CTA/login, complete TOC, no-JS, chapter SEO facts/head/sitemap/generation invalidation, no fake completion; PG+Chromium |
| AUTH-READ-05 | I | Verify all replicas, rollback fence, private provider/caches/TTL, restore, load, exact-head and post-merge CI; explicit activation only |

Default open preserves all existing books. Inactive operator rollout
prevents setting changes until every path is proven; once restricted data
exists, enforcement is unconditional. No existing release flag, live route,
provider, migration or deploy in this design PR.

Required unit: Unicode combining marks/emoji/CJK/RTL, empty and tiny text,
broken Markdown, links, huge first paragraph, 10/20/30/50 bounds, no raw HTML,
no full short chapter, deterministic hash and no duplicate full renderer.
Required PG18: default migration, N=0/10/30, gaps, non-free in prefix, future release,
renumbering and no paid/free mutation, allowed/denied roles, CAS races, live
sessions/revocation, exact current V1/V2 and pinned rights, scheduled and
bulk imports, wrong version/SHA, backfill retry/fence; no purchase side effect.
Required HTTP/MinIO: two free chapters with distinct tail sentinels, legacy
PG + SINGLE + active PACK, full same-origin response for active reader,
guest denied raw/source/plan/signed URL/HEAD/Range/query overrides/anchors/
media, concurrent publish/hold/logout/restriction, missing/corrupt projection,
provider outage, no fallback, wrong generation. Ensure anonymous S3 HEAD
and GET denied, whole-PACK URL absent, signed URL expiry and external CDN
checks separately. Verify guest preview still works when S3 is unavailable.
Required browser: public card -> any FREE chapter -> bounded fragment ->
login/register -> same exact full free chapter, logout -> preview; complete
TOC, native mobile/desktop/no-JS; no hidden tail or full URL in source,
network or hydration. Paid V1 purchase/read, saved FREE and Layer regressions.
Required SEO: 300 FREE+200 PAID fixture with N=10,p=20, chapter 10/11/300/301;
compare crawler and ordinary guest, authenticated preview noindex, sitemap/head
exact agreement, N=0 retains book card, N increase/decrease, stale generation,
rollback, owner switch, global disabled/pilot, held/age/paid/draft cases, missing
number/gaps, trusted canonical origin, no-JS content and no hidden tail.
Required load: at least existing 100k-chapter catalog fixture, bounded keyset
backfill with existing resource budgets, no N+1, no S3 on guest excerpt,
no full PG body retrieval for guest, measure authenticated source throughput,
memory and latency under shared pool. Report actual p95 and egress, not an
invented SLO. Required Recovery: restored policy keeps restriction, exact
current projection or safe rebuild, restored pin/ledger unchanged; no release
or production claim from synthetic CI.

Use existing architecture rules, RIL registration, targeted PG/browser suites
and full Integration CI for final exact head and post-merge SHA. Keep all
new mutation/activation disabled until cross-route negative tests and
operational acceptance are complete. No VPS changes or automatic deployment.

## References

[ACL](../roadmap/access-authorization-contract.md) ·
[S349](free-s3-delivery-v1.md) · [PACK Range](tor-store-drop06a-active-free-pack-range.md) ·
[SEO](seo-policy-v1.md) · [D-001](../roadmap/decisions-v1.md) ·
[Release checklist](../roadmap/release-checklist.md) ·
[Task #1349](https://github.com/calex794/tor/issues/1349).
