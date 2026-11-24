# REUSE-05A — shared bounded S3 response-body read

Scope G, with B/C/D/FND authorization and placement contracts unchanged.
Five identical read patterns in encrypted SINGLE, FREE SINGLE, PACK range,
independent PACK/manifest verification and FREE expected inventory become
one crate-private AsyncRead helper. The caller provides its existing trusted
expected byte count; the helper reads at most that count + one sentinel
byte, preserves the existing small initial capacity and returns the bytes
actually observed. Checked addition rejects an impossible overflowing bound.
A transport/read failure is still StorageError::Unavailable.

Each caller retains its pre-GET size/locator validation, HTTP metadata
checks, exact byte-length check, SHA-256, UTF-8 checks where applicable,
PACK range and Zstd checks, proof generation, and its own mismatch result.
The helper is not a proof, an entitlement or a generic S3 gateway.
No change to signed URLs, conditional writes, provider configuration,
timeouts, retry policy, RIL or database. G-09 registered-body diagnostics
keep the different 4 MiB limit/read path, since the intermittent first GET
connection reset requires separate investigation. Versioned quarantine
streaming hash/delete behavior is not part of this refactor.

Four offline unit tests cover short/exact/empty response, bounded oversize
read with remaining bytes unconsumed, overflow and transport errors.
The module is compiled for default unit tests as well as the s3 feature,
so no new workflow or test gate is required. Full existing Rust, MinIO,
FREE inventory, PACK range/proof, paid delivery and post-merge CI remain
acceptance criteria. No deployment or persistent storage access.
