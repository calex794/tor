# AUTH-UI-03b — email and claimed-handle login admission

Status: draft; 03b1 separates existing peer and submitted-identity admission and implements the private account budget plus transactional reset in the repository. No HTTP behavior changes, schema changes or deployment in this slice. Depends on AUTH-UI-03a (#1345) and the accepted profile privacy contract (#1365).

## Product contract

- Signup still accepts email and password; a handle is claimed separately in settings.
- Both login aliases enter the same existing POST /login route and password worker. Keep the existing form field name `email` for compatible clients; the login input becomes text with username autocomplete. Registration and password reset retain email input.
- Existing email-only accounts and legacy email normalization remain valid. Public name is never a login alias.

## Admission and privacy

1. Preserve same-origin, CSRF, honeypot and bounded credential validation. Charge the verified peer before database alias lookup. Never create a global loopback counter.
2. Keep the existing submitted-identity key and public 429 behavior independent of lookup results. Normalize both namespaces; an unknown valid handle receives the same treatment as an unknown email.
3. After resolving the alias, enforce an additional shared internal account attempt budget across both aliases. A private canonical key must not be returned or logged. Do not expose a distinct public 429 or Retry-After solely because that shared bucket is exhausted: doing so would create a cross-alias linkage oracle for a guessed private mailbox. Use generic credential rejection and bounded equivalent password work. Review migration/compatibility of existing attempt counters and fail closed on storage errors.
4. Unknown identities must still perform bounded dummy password work. Evaluate timing differences, disabled/locked accounts, lookup failure and worker saturation. This design does not claim cryptographic elimination of all timing side channels.
5. The existing spray source lock is independent of the submitted-identity limit. Review target pseudonym semantics explicitly: counting resolved aliases as one target can expose linkage through an observable source lock threshold. Do not silently use a canonical mailbox as a publicly distinguishable spray target. Maintain source admission and abuse detection; prove equivalent public responses and document residual correlation risk before enabling handle login.
6. Revalidate the requested alias and password hash under the account lock during session issuance. Reset successful-attempt counters only for the authenticated identity/account; examine ordering and transaction failure so stale proofs cannot clear another account's budget.
7. Existing diagnostic telemetry must remain opt-in, keyed and bounded; no raw mailbox, handle, password, cookie or hash in logs or response bodies.

## Repository slice (pending CI)

- Reuses the existing atomic `auth_attempts` storage with an independent hashed user-ID namespace and separate unresolved-identity fallback. The public submitted key remains byte-for-byte unchanged.
- Both private and proven submitted counters reset within the transaction that revalidates the alias/password hash and issues a session. Stale proofs and insertion failure cannot reset either counter.
- Tests exercise key separation, concurrent attempts at the limit, a stale alias, rollback on duplicate session token and successful reset. The current HTTP handler does not yet invoke the new budget.

## Required acceptance (not yet implemented)

- Disposable PostgreSQL + HTTP: email and handle success, case normalization, old no-handle accounts, mismatched and stale alias, password rotation race.
- Mixed wrong-password attempts cannot obtain two independent account password budgets; multiple IPs do not bypass the account budget. Unknown identities receive the same public credential failure as known identities. Test shared-bucket linkage attempts without asserting impossibility of all timing attacks.
- Existing public peer/identity budgets, spray mode, no-store, CSRF, honeypot, bounded worker and audit semantics remain intact.
- Browser: same two-field login form on standalone/modal pages, username autocomplete, handle submission, email-only registration/recovery; passwords cleared as before.
- Exact-head Repository rules and full Integration CI, then post-merge acceptance. No production migration/deployment.

