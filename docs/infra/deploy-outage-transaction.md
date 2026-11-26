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

## DEPLOY-08d-c: full signed-shell hosted acceptance

The existing guarded Ubuntu 24.04 test gains a separate outage phase after
the synthetic normal-release fixture. Four immutable historical main SHAs
serve only as identities for lab-signed copies of the current checkout, never
as assertions about historical build contents. The public forward ancestry
proof is checked by the actual installed module and the signed go-live shell
without any production guard bypass. The phase installs private helper modules
and a fixture marker only; actual v1 bootstrap and SSH transport remain
separate tests.

Required evidence: healthy refusal without consuming approval, stopped normal
update refusal, absent/wrong approval refusal, signed startup failure after
actual recovery migration and readable pg_dump, terminal stopped restoration,
replay refusal, successful real signed repair with retained book/credential
witnesses, interruption after actual signed cutover, root journal recovery
without claiming a healthy rollback, and fresh approval followed by retry.
The GitHub workflow requires all four DEPLOY08D markers. No production
credentials, production deployment, or v2 helper rollout is performed.

### Hosted API quota correction

Run 45879951473 failed before outage mutation on anonymous GitHub API HTTP
403 (rate limit exceeded). The VM now uses the job's ephemeral contents:read
Actions token solely for real GitHub main/ref and comparison requests, via
a private 0600 transient lab file. The token is removed from the process
environment before subprocess execution, is not logged or placed in the
bundle, and is removed in finally. The private fixture copy of the ancestry
module adapts only Request construction; the verifier, live API response
validation, installed CLI and signed go-live shell remain active. This is
not a production authentication change. The production anonymous API can
still rate-limit; fail-closed is retained. The authenticated marker is
required in CI. A separate operational API availability improvement is
needed if anonymous rate limits become a recurring production issue.
