# DROP-07 — verify a PACK retention snapshot

Export the PostgreSQL retention CSV for one registered PACK with
`tools/drop07_export_pack_retention.sql`. Keep the pack ID and the private
bucket and key from the trusted object inventory. Independently fetch the
exact PACK and manifest bytes to regular files on the operator host.

```sh
cargo run -q -p tor-storage --example drop_pack_retention_verify -- \
  123 pack-123-retention.csv pack-123.zst pack-123.manifest.json \
  tor-private-storage private/free-packs/example/source-pack.zst
```

The command verifies both SHA-256 digests, the complete seek table, decoded
edition hashes and sizes, and all frame offsets and encoded lengths in
PostgreSQL order. A successful match outputs aggregate counts. Treat a
nonzero exit as a refusal and discard the output of a failed export.

The report can contain `RETAIN` and `UNKNOWN`. Both states keep the
associated bytes while retention decisions are pending. This verification
matches the supplied local files to a snapshot. Before changing placements,
separately establish eligibility from retention policy and independent
backup evidence and revalidate the live PostgreSQL references under a write
fence.
