# DEPLOY-06b / I-11 — first local startup, INSTALL_EMPTY

After DEPLOY-06a preparation, use an independently approved dedicated
Ubuntu 24.04 host. Install trusted OS packages for PostgreSQL 18, Nginx,
OpenSSL/OpenSSH and Python via the operator-approved package path.
Configure the newly created PG18/main cluster on 127.0.0.1:5432, SCRAM,
no user database, no TOR roles; provision unprivileged tor-deploy account.
An active empty Nginx configuration (no server blocks, no enabled default
site) and independently provisioned valid TLS certificate/key are required.
No automated package repository changes, TLS issuance or DNS changes here.

With the reviewed trusted helper checkout, repeat independently approved
machine ID, release SHA, signer and manifest pin from preparation:

    python3 tools/activate_clean_install.py --target-machine-id TARGET_ID --sha RELEASE_SHA --manifest-sha256 PIN --signers /root/approved-signers --activate-local-empty

Root-only, existing global release lock. Refuse any prior TOR/restore
installation and any activation attempt, even incomplete. Reverify
signature/pin before staging and execution. Journal private mode 0600
is written before every phase. Init/migrate scripts are signed; explicit
--install-empty requires the local activation journal, absent current
pointer and PG_ACTIVATED, inactive web and no previous data/roles/schema.
Legacy active-web path remains unchanged.

Generate new database passwords locally using canonical init script;
restricted app/migrator roles, local backup, SQLx and read-only SQLx
history permissions. S3/KEK/provider settings are not created or activated.
Publish only loopback Nginx vhost and independent fallback first, verify
offline HTTPS 502 and asset. Install signed full unit and managed drop-in,
start Rust directly in PostgreSQL/auth mode. Verify actual binary inode,
HTTP/HTTPS readiness, auth CSRF, catalog, effective systemd policy, loopback
listeners and fallback. Enable app and write PG_ACTIVATED only after
acceptance. No release-format marker and no public admission.

Failure leaves the durable journal, DB and all credentials for inspection;
never reinitialize or delete existing data. Stop/disable unaccepted web
if startup may have run. Nginx failed initial config is removed only if
its bytes/link match the newly installed candidate. Successful edge is
left available if app startup fails. A killed process leaves active
journal and blocks a second clean initialization; explicit operator
recovery is required. This is not full power-loss recovery.

First public admission, trusted one-click helper on new host, deploy
SSH onboarding and real VM acceptance remain separate DEPLOY-06c/08.
Recovery of old data uses RECOVER only. Current tests use real filesystem
journal, signed template fixtures and injected operation boundary,
not a real clean VPS/PG/systemd start. No production deployment.

## Safe database diagnostics

The app/migrator TCP authorization checks use three unaligned psql columns
with an explicit colon separator. A standalone PostgreSQL boolean field
is t/f, unlike a boolean coerced to text in concatenation (true/false).
The expected role, database and CREATE permission are still checked exactly.
For signed --install-empty only, return fixed stage codes 10–19 from the
init script. The controller reports only a fixed step name and numeric exit
status in FIRST_START_INCOMPLETE, without ever forwarding SQL, stderr,
stdout, environment or passwords. Legacy standalone behavior is preserved.
No failed initialization is automatically retried or deleted.
