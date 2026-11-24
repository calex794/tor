# REUSE-07A — share real-PG FLOW HTTP fixtures

H/I test-only reuse. Compare the FLOW-02 visits and FLOW-03 funnel HTTP
tests: their browser/cookie/registration/SQL fixture implementation is
identical after normalizing visibility and the Value import. Their paid
and Layer entitlement scenario differs only by the test function name.

Move the fixture to tests/support/flow_http_fixture.rs and the common
scenario to tests/support/flow_paid_scenario.rs. Preserve both existing
ignored Tokio test entry points, independent Cargo targets and execution
order. Neither test is removed, no test count reduction, no new runtime
dependency, no schema or production source change. Preserve the existing
loopback tor_ci SQL restriction, statement/lock timeout, cookie handling,
real buyer/outsider/account flows, wallet/ledger assertions, age and
expired-session denial, disabled optional collection and financial cleanup.
The shared scenario deliberately retains a rejecting fake storage port:
this is not evidence of actual S3 download/decryption.

The separate legacy and current analytics databases remain isolated.
The existing CI lane mapping explicitly includes both shared modules and
both wrapper files; no new workflow or skipped gate. Acceptance requires
the exact old scenario assertions, full Rust/clippy/format, both actual
PG test binaries in pg-reading-analytics and complete required CI,
then post-merge. No deployment or live database access.
