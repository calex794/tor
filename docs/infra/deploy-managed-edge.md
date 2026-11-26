# DEPLOY-02–03 / I-11 — управляемый Nginx и FALLBACK

Stacked on DEPLOY-01 (#1263); actual signed asset templates are in #1262.
This is the isolated edge transaction engine, not a production activation.

## Layout and trust

- /opt/tor/releases/<sha> — previously staged, root-owned signed bundle v2.
- /var/lib/tor-edge/releases/<sha> — immutable edge assets and rendered snippets.
- /var/lib/tor-edge/current — relative symlink to releases/<sha>.
- /var/lib/tor-edge/settings/config.json — operator-owned public Telegram setting;
  created once from a validated signed default, not overwritten on update or rollback.
- TLS, existing vhost settings, root-only env, PostgreSQL, S3, SSH and backups stay untouched.

The fixed root helper verifies the complete v2 signature and payload before
preparing anything. The signed server snippet is rendered deterministically:
only its five exact aliases and a public release SHA header on game.js are
changed. Old workers retain absolute paths into the old immutable release.
The public settings file is shared across releases. No arbitrary paths,
archives, Nginx commands or user-supplied hostnames are accepted.

## One-time adoption — separate next task

The current real VPS vhost is not stored in Git. The operator must review it,
preserve TLS, HSTS, restrictions, limits and route contracts and adopt three
stable includes in their correct contexts:

* http{}: include /var/lib/tor-edge/current/nginx-http.conf;
* existing Pandaolate TLS server{}: include /var/lib/tor-edge/current/nginx-server.conf;
* existing public proxy location: include /var/lib/tor-edge/current/nginx-proxy.conf;

First adoption needs a separate guarded bootstrap with config backup,
candidate staging, nginx -t, reload, HTTPS verification and rollback.
It must handle existing add_header inheritance on Ubuntu Nginx 1.24.
This PR deliberately refuses to activate without a valid adopted current
symlink and a verified live marker. It does not edit existing vhost files.

## Normal edge transaction after adoption

Trusted root caller under the existing overall release lock:

1. Stage/verify the v2 bundle in a trusted root-owned release directory.
2. Run the trusted installed Python helper with prepare <sha>; this does not
   modify current or reload Nginx. Same SHA retries verify identical bytes.
3. Run activate <sha>. It checks baseline nginx -t and the actual HTTPS
   asset/marker for the previous release. Atomically switch current, test
   the full live configuration, reload (not restart), verify new HTTPS marker
   and SHA-256 of game.js. A failed candidate switches back, tests/reloads
   previous config and verifies the previous marker and asset.
4. Caller records TOR_EDGE_PREVIOUS for coordinated application rollback.
   Re-activating that SHA uses the same checks and is idempotent.
   The fixed CLI is not a new sudoers permission. No root actions are invoked
   by PR CI, and it is not connected to one-click workflow yet.

Rollback failure reports EDGE_ROLLBACK_UNVERIFIED, never success.
No cross-component power-loss journal, automatic pruning, first adoption,
full vhost generation, systemd changes or combined Rust/edge rollout yet.
These remain separate deployment stages. An ordinary update is still v1
until the root executor, workflow and full integrated tests are ready.

## Tests

Existing Repository rules discover test_managed_edge_release.py, covering
real fixture signatures, deterministic prepare, persistent settings,
invalid paths, and injected syntax/reload/probe/rollback failures.
Nginx command boundary is mocked for these fault tests. Actual Nginx with
the merged real FALLBACK templates and integrated update/reload/rollback
must be tested on an isolated runner before enabling production. No PG
migration or database code changes in this slice.
