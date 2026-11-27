# FALLBACK-01–03 / I-11 — autonomous Pandaolate edge page

The existing Nginx on the dedicated TOR VPS returns its generic 502 when the
loopback backend is unavailable. The new page is a static edge asset; it
requires neither Rust, PostgreSQL, S3, a CDN nor a release symlink.

## First slice

* Scope: edge-generated 502/503/504 only; preserve original status (never 200).
* Only GET/HEAD on explicit public HTML navigation paths receive the game.
  API, non-HTML, private, admin, health and write requests never receive it.
  Upstream responses, including application JSON 503, pass through unchanged.
* Strict CSP; no third-party script, account access, tracking or private data.
* Game scores are local only. Catalog checks are anonymous, bounded, no-store,
  once per 60–75 seconds in a visible tab, plus manual retry. HTTP 200 from
  catalog is not proof of full TOR readiness. No automatic navigation.
* Telegram appears only after an operator supplies a verified public channel
  URL of the form https://t.me/channel_name in config.json; no fake default.
* No book data or excerpts are exported in this slice. FALLBACK-04 must use a
  validated last-known-good public catalog snapshot, not a live DB/S3 request.

## Review / activation (manual only, not part of a release or PR)

On an authorized TOR host, inspect the actual Nginx HTTPS vhost and preserve
all existing TLS, route restrictions, headers, limits and proxy settings.
The repository does not contain the live vhost. Stage the five files from
ops/fallback/site/ in a separate root-owned directory /var/lib/tor-fallback
(readable by Nginx, not writable by the app). Use atomic file replacement;
do not overwrite an existing operator config.json with the default. Do not
copy unreviewed data or secrets there.

Install the three reviewed snippets to root-owned Nginx snippet paths.
Include nginx-http.conf once in http{}; nginx-server.conf in the existing
pandaolate.ru TLS server{}; nginx-proxy.conf in the public proxy location.
Do not duplicate existing proxy_intercept_errors/error_page directives.
Do not modify the home-ingress examples (different future architecture).
Nginx 1.24 location add_header directives do not inherit parent add_header:
preserve any existing HSTS and other required response headers when adding
these locations. Review MIME types, exact aliases and the noindex policy.

Back up the previous vhost, run nginx -t, and only after separate operator
approval reload Nginx. Validate real HTTPS and original status, upstream
application 503 and non-HTML/write exclusions. A disposable CI test is not
permission to stop a live service. To revert, remove the three includes and
validate/reload the old configuration. Do not alter systemd, Rust release
symlinks, database, backup or secrets. If Nginx/VPS itself is down, this
page cannot be served: external failover is a separate FALLBACK-08 task.

## CI acceptance

The dedicated workflow uses an isolated loopback Nginx plus an HTTP fixture,
without production credentials or deploy steps. No PostgreSQL is required
for an intentionally independent edge asset. Test real gateway status,
upstream 503 pass-through, direct internal-path denial, non-navigation
exclusion, CSP, local assets, recovery and desktop/mobile browser gameplay.
Evidence is pending until the exact PR SHA completes its runs.
