# FLOW rebuild development validation

This temporary WIP workflow permits actual compilation and isolated PostgreSQL
testing while the original local execution environment is unavailable. It runs
only on wip/flow-rebuild-* branches and uses PostgreSQL 18 at loopback with fresh
validation-only roles and tor_ci database. No production account or deployment
is used.

It is a development gate, not full repository/application acceptance. Missing
test files are explicitly printed as FLOW_PENDING_SOURCE because independent
owner slices are assembled incrementally. Final Integration CI must require
the completed new tests without these optional checks, plus real browsers,
repository rules and post-merge acceptance.

Rustfmt emits exact UTF-8 candidate files as base64 with their Git blob hashes.
These contain only changed tracked Rust source, never environment or credentials.
The job still fails if formatting differs from the committed source; the candidate
must be committed and tested again. An earlier or formatted working-tree PASS
must not be claimed for a different published revision.

Base: 406df4ee34a4ca7ea4dbf8bf5ebad12bf5031fb4 (editorial + observation + CI repair).
New implementation policy and source replace missing unpublished work; this
does not reconstruct dd8/c5/cc418 byte-for-byte or carry over their old PASS results.
