# AUTH-07a — measured session renewal under bounded PostgreSQL concurrency

Keep the existing 300 ms request budget, 200 ms statement budget,
25 ms lock budget and 24-hour minimum write interval. Do not increase
the pool or introduce a cache before measurement.

The real PostgreSQL credentials_pg suite now includes a deliberately
skewed 100,000-session fixture (one synthetic account; approximately half
of the background rows expired). With a dedicated runtime pool of three
connections and six workers, it measures the existing authority lookup,
the first mixed eligible/fresh renewal wave and subsequent fresh renewals.
It emits AUTH07 request counts, failures, p50/p95/max microseconds and
elapsed milliseconds. These are actual execution measurements, not
estimated production throughput or 100,000 concurrent HTTP clients.

Acceptance: all scheduled operations succeed; only eligible rows change
on the first wave, verified by expiry and PostgreSQL xmin; subsequent
renewals never rewrite fresh rows. A deliberately locked eligible session
fails within the existing budget while an independent authority lookup
succeeds. Release the lock and verify renewal resumes. Delete the isolated
fixture on success. CI already runs credentials_pg, so no additional
workflow or runtime dependency is introduced.

Do not treat these results as production capacity. Browser/HTTP end-to-end
load, mixed workloads with real readers, and restart/restore acceptance
remain follow-up work. Optimize the renewal path only against measured
evidence. No VPS changes or deployment.
