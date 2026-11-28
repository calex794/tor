# MAIL 4 — canonical dispatch integration

Consolidate #1364/#1367 and the alternative implementation in #1368. The only migration sequence is 202999100054 (claim-bound launch intent), 202999100055 (distinct deferred state and due time), and additive 202999100056 (redacted defer reason). Do not apply the alternative migrations with the same version numbers. The previous branch remains available for review; do not merge it independently.

Retain the typed DispatchGrant, PreparedDispatch and DispatchStart witness, exact current activation/profile/budget/recipient binding, and one serialized worker. Recovery links use the existing account-owned source and 10-minute eligibility bound. Other product purposes stay unclaimed until trusted sources are implemented. The common scheduler uses at most four pre-charge claims, 30/60/120-second DNS/transport/activation backoff, five seconds for pacing, 120 seconds for recipient cooldown, and a 24-hour upper bound for other purposes. No post-charge replay. Diagnostic reasons contain only a fixed enum. This adopts useful backoff, diagnostics and recipient validation from #1368 without adopting its conflicting schema or 24-hour retry for expired recovery sources.

No external SMTP, startup registration or production activation until integrated acceptance. Existing migrations already present in main are unchanged.
