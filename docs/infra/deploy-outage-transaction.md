# DEPLOY-08d-a: durable outage transaction (library only)

Normal release rollback assumes that the previous site was healthy.
Do not reuse that assumption for an operator-authorized outage repair.

A separate internal coordinator route requires a stopped and identified
previous application, a distinct forward candidate, and an exact-approval
consumption callback. Approval is consumed after inert preparation and a
second stopped-baseline check, before durable intent or live changes.
The caller will hold the same application and edge locks. No CLI or sudo
route invokes this library entry point yet.

Normal records retain journal version 1 byte format. Outage records use
version 2 with mode=outage; malformed/cross-mode states are rejected.
Only candidate HTTPS readiness plus edge and unit verification can
commit a release. On failure/interruption, stop the candidate, restore
the previous pointer without restarting the previously broken binary,
restore edge/unit policy, and verify the stopped state. A verified
configuration restoration is terminal outage_stopped, not rolled_back
or restored website. Failed restoration stays rollback_pending and
blocks subsequent updates. Database migrations are never undone.

Remaining steps: implement root-only exact-approval and stopped service
boundary, validate actual signed go-live recovery script and forward
ancestry, exercise real Ubuntu failure/crash and retry, and separately
test production transport/bootstrap. No VPS change, production
activation, or new sudo permission is part of this step.

## DEPLOY-08d-b: explicit root executor (pending real acceptance)

The installed private executor now recognizes a distinct root-only
`--repair-outage SHA` request. The normal single-SHA sudo wrapper is unchanged
and cannot pass this option. It is not an automatic failover.

Root must have already stopped tor-web.service, inspected the outage, and
approved the exact previous and candidate SHAs in a root:root 0600 single-link
file /etc/tor/tor-release-v2-outage.approval (two newline-terminated SHA lines,
previous first). The 0700 parent, completed v2 marker, active PostgreSQL policy,
forward ancestry, signed immutable candidate, dead systemd unit, no TCP listener
on 3000 and actual local HTTPS 502 are required. Approval is rechecked and
durably consumed under the existing locks immediately before live mutation.

The existing signed go-live script runs with --recover --forward, preserving
its pre-migration backup and SQLx checks. Failed/crashed repair restores only
the stopped pointer and edge/unit configuration; it never restarts the known
broken predecessor. Success requires real application and edge verification.
This is an implementation candidate, not operational approval. Hosted VM must
still exercise the new production boundary and signed recovery shell; do not
install the helper or run a recovery on the live VPS on this evidence alone.
