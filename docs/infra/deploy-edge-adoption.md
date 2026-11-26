# DEPLOY-02a / I-11 — первичное принятие существующего Nginx

This stacked PR adds a root-only one-time adoption helper, not a release.
Requires the trusted installed helpers and an already verified signed v2
bundle; the production installer and workflow are not connected yet.
The live VPS configuration is unavailable in GitHub.

## Supported existing configuration

One canonical regular root-owned vhost in /etc/nginx/sites-available or
/etc/nginx/conf.d, loaded exactly once by nginx -T. One dedicated TLS server
for pandaolate.ru and a single location / with proxy_pass to 127.0.0.1:3000.
The lexer accounts for quotes, comments and nested blocks. It inserts exactly
three fixed includes without rewriting the existing bytes, certificates,
redirects, private location restrictions, limits, or proxy headers.
Shared vhosts, existing error_page/interception or fallback, unknown syntax
and permission failures require operator review. The read-only plan prints
only the SHA-256 of the existing file, never the configuration dump.

After deployment of the trusted root helpers (separate bootstrap), the root
operator selects the actual vhost and runs plan, then explicitly approves
that file hash and a full SHA of an already staged signed v2 bundle:

    python3 /usr/local/lib/tor-release/adopt_edge_vhost.py plan --vhost /etc/nginx/sites-available/pandaolate
    python3 /usr/local/lib/tor-release/adopt_edge_vhost.py adopt --vhost /etc/nginx/sites-available/pandaolate --sha FULL_RELEASE_SHA --expected-vhost-sha256 APPROVED_HASH

The example vhost path is illustrative, not a claim about the live VPS.
No new sudoers permission. The command uses the same overall release lock.
Ordinary updates after successful adoption do not require this procedure.

## Transaction

- Verify selected file SHA, exact configuration shape and loaded vhost.
- Capture real local HTTPS status/security headers; backend may be down.
- Preserve validated literal HSTS in each managed static location (Nginx
  1.24 header inheritance); store its value independently of release.
- Prepare signed edge assets and a root-only exact original config backup.
- Switch edge link and atomically replace the vhost with three includes.
- Run nginx -t, reload, verify actual HTTPS asset version and body hash,
  preserved catalog status, normal security headers and HSTS.
- On failure restore original bytes/mode, test/reload, verify catalog and
  old asset response, remove the new edge link only after verified rollback.
  Report unverified rollback if any recovery step fails. Keep backup and
  candidate for diagnosis. No stopping the application or migrations.

This is not crash-atomic across a power loss; persistent transaction journal,
combined app/edge rollback, trusted helper bootstrap and real Nginx adoption
tests remain separate steps. A baseline 502 is permitted and is not proof
the Rust application is healthy. No production VPS changes in this PR.
The fixture tests use real Ed25519 bundle signatures and mocked Nginx boundary.
