# MAIL-01b2 — live DNS transport (Draft)

Adapter: Hickory Resolver 0.26.4 (pinned; official tag https://github.com/hickory-dns/hickory-dns/tree/v0.26.4). Use system-config and tokio; no recursive resolver, arbitrary external provider or SMTP probing. Query absolute DNS names, ignore hosts file, preserve response status and remaining TTL; classify NXDOMAIN/NODATA as definitive only when SOA and negative TTL are present. Treat all other errors as Retry. The existing MAIL-01b1 service supplies an outer 2-second per-lookup budget; resolver timeout 1.5 seconds, one attempt, one server per concurrent request; internal cache 64 responses with TTL caps 24 h / 1 h. Application-level concurrency/coalescing/cache are MAIL-01c. The adapter is not wired into registration or real sending yet.

Deterministic UDP loopback fixture covers MX, CNAME, Null MX, A/AAAA fallback, SOA-backed NXDOMAIN/NODATA, unsupported negative response without SOA, SERVFAIL, REFUSED and timeout. No public DNS, mail addresses, API keys or real messages in CI.

Dependency lock: temporary read-only preflight step emits Cargo.lock diff and rejects a stale lock. It MUST be removed once the generated lock has been committed and checked; normal CI must then run unchanged. No deployment or merge before verified lock, RustSec and full integration acceptance. Local environment has no Cargo/network; the lock is not guessed.
