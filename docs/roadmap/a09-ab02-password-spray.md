# TOR / AB-02 — PostgreSQL password-spraying guard

Source: NEwBOO `backend/app/services/auth_spray_detection_service.py` @ `17125ef758`.
The donor uses a process-local failure detector, with default 12 different
identities/IP and 24/subnet in 600 seconds, and a 600-second local lock.
Its distributed enforcement is **not** the donor's default runtime.

## TOR implementation, safe rollout

- `TOR_AUTH_SPRAY_MODE=off|observe|enforce` defaults to **off**. No VPS flags
  or production data have been changed. An unrecognized mode is an explicit
  error rather than silent unlimited authentication.
- Both non-off modes require an independent 32-byte HMAC key
  `TOR_AUTH_SPRAY_KEY` encoded as 64 nonzero hex characters. Do not reuse
  session, reset, RIL, or `TOR_AUTH_OBSERVATION_KEY` secrets.
- The existing PostgreSQL login limiter remains mandatory (60 attempts per
  verified IP and 8 per normalized account per 600 seconds), and runs before
  the spray preflight. Verified peers use the existing trusted-edge policy;
  arbitrary Forwarded/X-Real-IP values are never trusted.
- The new `auth_spray_windows` and `auth_spray_targets` relations keep
  only keyed HMAC pseudonyms and bounded counts. An IP/subnet row and unique
  identity are locked/updated in one transaction, serialized across Axum
  processes. Neither best-effort auth observations nor RIL is enforcement.
- A **confirmed invalid password** or unknown account counts as a failure.
  Invalid Origin/CSRF, filled honeypot, unavailable DB/Argon2, or successful
  login do not add spray evidence.
- To reduce false positives on NAT/CGNAT, only a verified **exact IP** can
  acquire a temporary 90-second lock, after at least 20 confirmed failures
  against at least 12 distinct accounts within 600 seconds. The shared
  /24 or /64 subnet is counted for diagnostics, **never** used as an
  automatic blanket ban. One account's repeated typo cannot reach the gate.
- On a lock, every login identity gets the same private 429, with real
  `Retry-After`; the login handler returns before account lookup and Argon2.
  No raw email, IP, user agent, path, cookie, or password enters these tables
  or a metric label. There is no persistent IP ban.
- In `observe` mode thresholds are measured, not enforced. In `enforce`
  mode missing/invalid key or unavailable PostgreSQL returns 503 rather
  than silently allowing unlimited verification. For an unverified proxy IP
  there is no source bucket: the account-specific cap remains effective.
- The choice of 90 seconds/20 failures instead of the donor's 600 seconds/12
  identities is an **intentional** safer first enforcement policy; tune only
  after a real false-positive/load audit. This does not assert exact parity
  with the donor's enforcement settings.
- Existing maintenance removes stale source rows and associated unique
  target rows after one day, at most 1000 sources per pass. State survives
  restart and is shared across processes; rotating the HMAC key naturally
  stops matching old pseudonyms until retention cleanup.

## Acceptance

- `cargo fmt --all -- --check`, Rust unit/Clippy, SQL migrations twice,
  PostgreSQL and real HTTP integration.
- `cargo test -p tor-accounts --test auth_spray_pg -- --ignored --test-threads=1`
  checks two independent pools, concurrent distinct inserts, IP and subnet
  separation, passive observation, expiry, maintenance, and pseudonym privacy.
- `cargo test -p tor-web --test auth_spray_http_pg -- --ignored --test-threads=1`
  checks 429/Retry-After, indistinguishable known/unknown accounts, CSRF,
  no session cookie, and valid login after a short lock expires.
- **Not in scope:** AB-05 CAPTCHA; global/domain risk friction; automatic
  permanent IP bans; manual admin blocklist; AB-08/09 scoped write limits;
  and production Nginx/deployment. Observe-only acceptance on live traffic
  and rollback drills remain operator decisions.

Rollback: change `TOR_AUTH_SPRAY_MODE=off` in a later controlled deployment.
The existing login limits and ACLs continue unchanged. Never run migrations
against the permanent database as part of this PR.
