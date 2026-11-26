# DEPLOY-06a / I-11 — clean-host preparation

INSTALL_EMPTY is not UPDATE or RECOVER. Do not run it on a working host,
a failed TOR host, a host containing its PGDATA or a disaster recovery
workspace. Use the independent Recovery procedure for previous data.

New install-only signed payload entries: dedicated Pandaolate Nginx
vhost, existing full tor-web service example, and a canonical PostgreSQL
drop-in. Existing normal v2 update does not write these base files.
The new vhost assumes a dedicated single-purpose server, a real approved
TLS certificate at the existing standard path, and no conflicting
default server. Readiness and health over HTTPS are loopback-only; metrics
are denied. The operator must approve the 1-year HSTS and noindex policy
before the first public activation. Nginx is connected to the independent
managed edge include contract. Auth/PG are on; no RAM demo.

## Read-only verification and preparation

Before installing PostgreSQL packages (which may create PGDATA), on an
independently approved Ubuntu 24.04 host, obtain the signed release
bundle, trusted public signer, exact approved SHA and independently
retained SHA-256 of its manifest. Use trusted reviewed helper sources;
never import Python code from the incoming bundle. No private signing
key is needed.

    python3 tools/prepare_clean_install.py --target-machine-id TARGET_ID --sha RELEASE_SHA --bundle /root/approved-bundle --signers /root/approved-signers --manifest-sha256 PIN

Add --prepare to atomically create /srv/tor-install containing the
verified immutable bundle, candidate config files and a private plan.
Existing workspace, existing TOR/PGDATA/recovery markers, symlinks,
signature/hash/pin mismatch and wrong host are refused. No package
installation, database init, service restart, DNS, firewall change,
credential generation or TLS issuance occurs. Preparation is not
acceptance. Private config copies are not authoritative; future activation
must reverify the original signed bundle and exact destinations.

## Next activation slice

New empty PG18 instance and roles, root-only secret generation, backup/
SQLx on empty schema, root-owned application and edge directories,
Nginx/TLS configuration, trusted helper installation and initial
systemd start. Verify live local and HTTPS app identity, auth/CSRF and
catalog before setting PG_ACTIVATED and release-format markers.
Never create a temporary public demo server to satisfy the old script.
Existing update scripts assume healthy old application and are not
called by this preparation. Real clean-machine and failure acceptance
are still required. No operational install or S3 provider activation.

DEPLOY-06b: first vhost listens on 127.0.0.1 only. Public bind and firewall
admission require a separate operator decision. The empty install controller
requires the independently prepared workspace, trusted OS packages and an
empty loopback PostgreSQL 18/main. It deliberately does not provision apt,
DNS, certificates or a deployment account. See deploy-empty-first-start.md.
