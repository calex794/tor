# AUTH-02a / A-03: rolling browser session lifetime

Owner decision: 30-day session with renewal on activity, self-service and
administrator revocation, no new authentication stack. Source: NEwBOO
`backend/app/core/config.py`, `backend/app/services/auth_service.py` and
`backend/app/main.py` at 17125ef758a791e9630505c14d24f7fe65aed228.
TOR retains opaque 256-bit cookie tokens, SHA-256 digests, PostgreSQL authority,
existing CSRF/Origin controls and password rotation.

This first slice sets the login/registration/password-rotation lifetime to 30
days. Successful user-activated same-origin top-level HTML navigations renew
the existing live row to 30 days. PostgreSQL time is authoritative; the expiry
moves at most once per 24 hours, so the effective idle timeout is 29–30 days.
A live old seven-day session can be upgraded without login. Expired/revoked or
inactive accounts are never renewed. Assets, health, API, automatic fetch,
cross-site navigation and administrative report responses do not renew.
Browser cookie age is synchronized to remaining server expiry even if a
previous response was lost. No schema migration or secret rotation is needed.

This slice does not introduce a maximum absolute lifetime, an administrator
session policy or a client-side activity endpoint. Reader AJAX/scroll
activity, full session/device management, administrator revocation with audit,
load testing and release/restore continuity remain subsequent slices.
No production rollout is authorized by this PR.

Acceptance: the existing Integration CI accounts and
`account_sessions_http_pg a_03_` suites cover 30-day issuance, live legacy
upgrade, background/cross-site exclusion, no repeat expiry writes,
concurrency of fresh reads, and refusal to revive revoked/expired sessions.
Real browser reader coverage and restart/rollback evidence are still required
before declaring the full program complete.
