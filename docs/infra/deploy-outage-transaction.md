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
