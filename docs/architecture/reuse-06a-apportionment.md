# REUSE-06A — common integer largest-remainder apportionment

D ownership. Share only the pure arithmetic already implemented separately
for chapter creator allocations and weighted/non-posting Layer previews.
Both use integer floors, distribute remaining minor units by descending
fractional remainder and break ties by ascending immutable recipient ID.
The new private helper returns amounts in input order; callers retain
their existing sorting, validation, typed amounts and public error types.

The purchase path still validates gross, approved versioned policy, unique
recipient IDs and exact 100% share totals before allocation, computes the
same platform fee using HALF_UP basis points, verifies conservation, and
uses the persisted refund deadline for holds. Layer still requires explicit
approved financial inputs and verified frozen snapshots; LegacyEqualLast
remains separate and unchanged. The preview does not post money. No fees,
default percentages, policy version, weight, refund window, wallet, SQL,
transactions, idempotency, routes or access rights are changed.

Acceptance: keep all D-01/D-08 and Layer/PG finance tests, add an independent
two-recipient integer oracle for 20,196 ordered combinations, tests for
three-way ties, zero pool, i64::MAX and invalid arithmetic, and a cross-path
comparison of 64 fee/gross/schedule combinations including zero creator pool.
Required Rust, real PG checkout/finance/Layer, full CI and post-merge.
The new math helper is not a policy approval or permission to enable a
new payment or earning flow. No deployment.
