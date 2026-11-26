# DEPLOY-06c — admit one IPv4 interface, connect fresh VPS to GitHub

Requires DEPLOY-06b local_ready, the exact signed release and independently
approved machine ID and manifest pin. Do not use on existing/restore host.
First choose an actual locally assigned non-loopback IPv4 (may be behind
NAT). Review firewall and provider network settings separately.

    python3 tools/admit_clean_install.py --target-machine-id TARGET_ID --sha RELEASE_SHA --manifest-sha256 PIN --bind-ipv4 ADDRESS --admit-ipv4

The root-only helper uses the same global and edge locks. It adds only
four listen lines to the signed local template, preserves loopback,
TLS, noindex, HSTS, route restrictions, forwarded headers and buffering.
No wildcard/IPv6 admission, DNS, firewall or provider API changes.
Checks actual interface, Nginx syntax, worker generation, listener set,
real Rust process, TLS catalog, redirect, fallback marker and denial of
readiness/metrics from the selected non-loopback source address.
This is local interface acceptance, NOT independent internet/DNS proof.

Private durable public-admission.json precedes config replacement.
Failure restores the exact local vhost and verifies local listeners/edge;
unverified rollback stays pending. An interrupted attempt requires
explicit --restore-local using the same approved inputs. No DB changes.

## Connect the existing one-click workflow to the fresh host

Provision the restricted tor-deploy SSH public key independently, matching
the already pinned release signer. Do not copy signing/private SSH keys
to the VPS. Set GitHub host/known_hosts only after independent SSH host-key
verification. The unprivileged account must have no supplementary groups
and no broader sudo rules.

Prepare a canonical root-private directory (0700) containing the exact
seven reviewed helper modules listed by install_release_v2.MODULES and
one-click-release-v2-wrapper.sh as wrapper.sh from the approved immutable
main commit. Do not use code from the incoming SSH-writable directory.

    python3 tools/connect_clean_release.py --target-machine-id TARGET_ID --sha RELEASE_SHA --manifest-sha256 PIN --bind-ipv4 ADDRESS --helper-source /root/approved-v2-helpers --connect-github

Verify local interface acceptance again, matching signer/restricted
authorized_keys, effective sudo permissions, exact helper bytes and
current main. Install only fixed SHA-validating wrapper and private
modules; create restricted incoming directory and one exact sudoers
rule, validate with visudo and sudo -l. Publish v2 marker last. A failure
before marker leaves the wrapper fail-closed. Identical partial setup can
be retried, unknown files are refused. No actual Rust release, private key
generation or SSH config modification. Set TOR_REQUIRE_RELEASE_V2=1 only
after marker and independently verifying remote SSH. A later approved
main SHA can use the existing one-click update.

Still required: offline signed bundle export for initial installation
without an existing deployment host, real combined systemd/PG/Rust VM
acceptance, external DNS/TLS/firewall proof, operator approval.
Local install remains non-S3 until the independent provider/profile gate.
No VPS changes in this PR.

The isolated Nginx lab now adds an extra IPv4 to its private loopback
namespace, exercises the signed clean vhost render with temporary TLS/edge
paths, checks actual route status/HSTS/redirect/asset, and proves the
extra listener disappears on restoration. No external traffic. Fourteen
new architecture tests cover admission journal/failures, address guards,
SSH signer and sudo/file restrictions. CI results are not yet confirmed.
