# UI-10B — real mixed FREE/PAID book visual acceptance

Status: PR candidate; no deployment.

Reuse S034 disposable PostgreSQL 18 and verified local MinIO,
two PAID ciphertext chapters plus two S3 FREE chapters in the SAME book,
before BR-060 owner deletion and archive testing.

Verify genuine Axum /books/:id metadata as guest and buyer:
real paid/free links and badges, one title, strict script-src none,
no encrypted media URLs, paid bodies, DEKs or unescaped unsafe markup.

Then run real Chromium on the same Axum router: 3 layouts x 2 themes
x 390/1440; validate first-screen read CTA, original href parity,
no horizontal overflow, computed CSS differences and screenshots.
Only public numeric chapter IDs reach Node; no sessions or grants.

Use existing paid-private-e2e job and its masked screenshot artifact.
Never mutate commerce, grants, S3 persistence or permanent database.
Accept only with exact-head full CI and post-merge main CI.
External Yandex S3 and VPS out of scope.