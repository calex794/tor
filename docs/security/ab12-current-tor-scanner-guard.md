# AB-12: current TOR/Axum scanner surface (no new ingress deployment)

The one-click production release packages tor-web, tor-migrate and tor-trace-store
binaries, **not the Nginx virtual-host configuration**. Main does not contain
the installed /etc/nginx site file. The earlier home-ingress PR #1192 describes
a future topology and must not be represented as live VPS protection.

This change adds a minimal, independent guard directly around the **final**
Rust Router in crates/web/src/main.rs, after optional modules and FLOW are
merged. It applies to every request that reaches tor-web from the existing
Nginx without any new server configuration or database migration.

Scope: known WordPress/PHP, CGI and secret-file probe paths only. Respond
HTTP 404, no body, Cache-Control no-store. Real TOR routes, unfamiliar paths,
the root path, sitemap, search bots, reading, API calls and shared-NAT users
are unaffected. The guard does not inspect an IP address, user-agent, header,
body, credential, query string, or signed URL; no state proportional to clients.
A fixed-cardinality aggregate scanner-rejected counter is included in the
already protected /metrics endpoint. A count of probes **is not a count of
bots, visitors or successful attacks**.

This is not a replacement for the existing PostgreSQL-backed auth limit,
spray observe/enforce mode, honeypot forms, CSRF, permissions, or Nginx.
It does not claim to protect the Nginx front door against bandwidth or
connection exhaustion. Adding IP bans, CAPTCHA, unknown-route adaptive
limits and proxy-level per-IP request caps requires separate acceptance and
review of the real-IP trust chain; those protections are NOT part of this PR.

Acceptance: Rust unit tests (known malicious paths rejected, legitimate
paths passed, downstream never invoked), Python wiring tests, and exact SHA
GitHub CI. No production deployment, Nginx reload, or env change.
