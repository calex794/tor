# DROP-07f — bounded operator scans and comparable reports

Run this example only on a trusted operator host using a dedicated database
role and S3 credentials restricted to LIST/GET in one private bucket. Supply
`TOR_DROP_AUDIT_DATABASE_URL`, `TOR_DROP_AUDIT_BUCKET`,
`TOR_DROP_AUDIT_S3_ENDPOINT`, `TOR_DROP_AUDIT_S3_REGION`,
`TOR_DROP_AUDIT_S3_ACCESS_KEY` and `TOR_DROP_AUDIT_S3_SECRET_KEY`
through the operator environment. Report mode needs only the database and
bucket configuration.

```sh
cargo run -p tor-web --features s3-free --example drop_inventory_audit -- scan observed pack 8
cargo run -p tor-web --features s3-free --example drop_inventory_audit -- scan expected pack 8
cargo run -p tor-web --features s3-free --example drop_inventory_audit -- report expected pack
```

Each explicit scan is limited to 32 independently leased pages and stops
at completion. Running again after completion starts an independent cycle.
Report mode returns at most two recent completed cycles for the same
bucket, direction and namespace. Completed and failed evidence is immutable.

The operator sees aggregate counts only. Matching counts do not prove the
same object set or a simultaneous S3/PostgreSQL snapshot. A newer complete
cycle does not supersede a pinned edition or off-site retention obligation.
Reports authorize no repair or deletion. A future GC decision requires
object-specific evidence, write fencing and independently protected backups.
