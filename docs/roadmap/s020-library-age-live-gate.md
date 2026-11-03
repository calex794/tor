# S020/C-08: live age-tier guard for saved Library metadata

Reference: [NEwBOO book_access_policy.py](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/books/policies/book_access_policy.py).

## Implemented

- `PgLibrary::add_book` authorizes in the **INSERT ... SELECT** PostgreSQL statement; a prior GET, client checkbox or purchase record grants nothing.
- `PgLibrary::list_books` applies the same eligibility test **before** bounded keyset pagination: an unauthorized saved title is not returned and does not displace authorized items.
- Uses the active account row from `users`, live `books.visibility='public'` and `visibility_status='active'`, non-held/non-deleted, and `books.access_level`: `basic` for active accounts, `adult` only after `users.adult_enabled=TRUE`, `pro_adult` only with an active `user_level_grants` record. `NULL` unknown level is denied.
- Retains saved rows on revoked eligibility. A later opt-in/grant restores visibility; removal is still allowed while the Book is hidden or the grant is revoked.
- Adds real disposable PostgreSQL test for basic/18+/21+/NULL, live opt-in and grant revocation, moderation visibility, and Book-tier changes, registered in both integration workflows. Existing 105-book fixture is expressly marked `basic`.

## Not included / #486 remains open

- This controls only **saved-book library metadata**. The `/me/library` page also includes continue-reading, updates and purchased links through separate repositories; they still need their own privacy/age review.
- The paid V1/V2/Layer and S3 grants need a separate age gate while preserving purchase entitlements. The dedicated administrator reader remains unchanged.
- Not a full NEwBOO parity acceptance and not an age settings UI. Keep S020 **Partial**, strict project acceptance unchanged (24/60).
- No production migrations, deployed services or VPS changes.
