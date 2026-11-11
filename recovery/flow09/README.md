# FLOW-09 recovery checkpoint — uncompiled work

This branch is a recovery copy of incomplete authoring work. It is not a product
change, accepted implementation, pull request, or release. Only this directory is
added to the published parent commit
85f47ab9dbfa1ec15562e6e0602c61d08a9582fd.

## Provenance

The unavailable local worktree was tor-flow09, branch feat/flow09-completion.
It started at local integration commit fb9b7c36a, which includes the durable
observation-health work from local signals commit cc418415a. The lifecycle commit
f88ba10a5 was cherry-picked as ea76afae8. These local prerequisites are not asserted
to be present in the remote parent of this recovery branch.

The contents directory preserves eight new-file authoring texts from successful
apply_patch calls. The applied patches preserve the corresponding changes to
existing files. No formatter or compiler ran on this new package. After the
environment became unavailable, these texts were reconstructed from the exact
tool-call inputs; there was no filesystem readback. No Git tree or filesystem byte
equivalence is claimed.

## Inventory and order

- contents/ contains new migration 202611294686, public-page domain types,
  PostgreSQL repository and its three child modules, HEAD service, and web worker.
- patches/01-applied-core.patch contains the completed-window RR exposure/health
  owner read, complete genre summary, new signal types and exposure adapter edits.
- patches/02-applied-wiring.patch contains the local module/lifecycle declarations.
- patches/03-pending-ui-worker.patch is the exact apply_patch call that did not run
  because the execution transport closed. It adds the immediate post-HEAD policy
  recheck, page evidence adapter, signal worker composition, admin route and HTML.
- patches/04-prepared-ui-completion.patch was prepared after environment loss.
  It adds the full-genre summary's bounded explanatory labels and remaining UI
  labels, source links and navigation. It has not been applied.
- manifest.json records the status, original paths, counts and remaining work.

Patches use the original absolute worktree path. Adjust only that path when
applying them to a recovered checkout. Do not apply the already-applied patches
again to the original dirty worktree; inspect it and checkpoint it first.

## Intended bounded behavior

The public-page source checks HEAD on at most four current targets: home, catalog,
one anonymous basic-public book, and one published free chapter. It uses only the
configured TOR_PUBLIC_ORIGIN, HTTPS except loopback development origins, no
credentials/cookies/proxy, no redirects, two concurrent requests, a 750 ms request
budget and a five-second round budget. One fenced database lease allows a round
per minute. At most four result rows and one lease row are stored.

Current B/C policy and row revisions are checked before requests, immediately
after requests (pending patch), and under brief SHARE locks before storing.
Lawful hold/delete/access/release/revision changes do not manufacture a page-loss
alarm. A prior 200 followed by another HTTP status on the unchanged eligible
target can be a loss. Network errors, first failed observations, changed targets,
changed origin, missing baseline and stale evidence remain unknown. Admin source
links are rechecked against current public policy.

Genre totals cover every distinct nonempty current genre before the bounded
detail list. Duplicate labels on a book count once; overlapping genres are not
summed into all-book totals. Classification uses existing 20 DOM acknowledgement,
80 percent delivery, and fewer than five observed-view thresholds. Unknown plus
zero cannot prove recovery. Closed exposure counts, genre totals and durable
health share one repeatable-read read-only transaction.

These are intended behaviors and unverified implementation inputs, not acceptance
claims.

## Remaining before integration or publication

1. Restore/checkpoint the original dirty worktree; apply the pending patches.
2. Finish append-only RIL registration GET /admin/audience/pages at ID 408
   (409 route pairs / 411 bindings), preserving existing IDs.
3. Update 13-rule test fixtures and migration fixtures; reconcile the shared signal
   worker/render edits with H-owned HTML-health work (migration 685).
4. Add and run real PostgreSQL regressions for all-genre totals beyond detail 100,
   duplicate labels, partial unknown zero, a deterministic concurrent RR snapshot,
   origin/revision/eligibility races, lease expiry and restart.
5. Run a real HEAD fixture and native admin HTTP/browser paths. Assert no redirects,
   credentials, false audience/completion facts, authorization bypass or unknown
   recovery. Exercise actual current source report and all 13 rules.
6. Run formatter, compiler, affected suites, lint, architecture/RIL rules and a
   fresh/repeated installed-schema migration pass. Update contracts and acceptance
   evidence only after actual results.
7. Small static follow-ups already identified: enforce exact slots 1..4 on save;
   suppress historical success from a different configured origin in the UI.

No production deployment, persistent database, main branch, or PR is changed by
this checkpoint. New code here is NOT COMPILED and all current acceptance is PENDING.
