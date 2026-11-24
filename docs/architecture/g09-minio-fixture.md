# G-09: pin corrected upstream MinIO in provider CI (#533)

Only the mandatory s3-provider job and its manual wrapper use the
corrected source fixture. No production adapter, deployment, credentials,
SDK retries, HTTP connection policy, storage semantics or test assertions
change. Other suites deliberately retain their original fixtures in this
narrow change; they are not claimed to have been requalified on this build.

Upstream minio/minio#21633 reports duplicate HTTP responses causing
incorrect connection drops. minio/minio#21651 fixed it on 2025-10-24.
Pin the exact fix 52eee5a2f19ea8ef06bf2dd5ab64ab41424cb591 (immediate
parent c6d3aac5c436d8468cf5a807498b6a97db421979), Go 1.24.8, read-only
module resolution and verified checkout; log the embedded commit ID and
binary digest. No floating upstream branch or fallback to the old binary.

Controlled evidence, same TOR binary per run, one attempt, no intervening
request between rejected immutable PUT and first GET:

| Fixture | First experiment pooled | Repeat pooled | Combined close |
| --- | ---: | ---: | ---: |
| September release | 1/1000 | 10/10000 | 0/11000 |
| Immediate parent | 3/1000 | 6/10000 | 0/11000 |
| Upstream fix | 0/1000 | 0/10000 | 0/11000 |

All 66000 objects passed byte-integrity verification. First experiment
job 138687355866 (workflow failed on a libtest-prefix parser error, all
samples completed); repeat job 138691891746 passed with validated counters
and reversed arm order. Independent strict job 138694742418 passed all
13 selected S3 tests plus storage/imports clippy, including G-09 pooled
isolated and parallel and close control, reconciliation, FREE/PACK,
scope, cleanup, S023. This supports the upstream explanation, not a proof
of zero future transport failures or external provider compatibility.

Evidence:
- https://github.com/calex794/tor/actions/runs/45462153592
- https://github.com/calex794/tor/actions/runs/45463432361
- https://github.com/calex794/tor/actions/runs/45464240996
- https://github.com/minio/minio/pull/21651

Acceptance requires the original mandatory s3-provider job, unchanged
first GET checks, full exact-head CI, then post-merge CI. Until that
succeeds issue #533 remains open. Rollback is a reviewed revert of this
fixture change, not a silent fallback or weakening of G-09.
