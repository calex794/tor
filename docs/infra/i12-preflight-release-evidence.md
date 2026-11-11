# I-12 / S061: require current preflight in release evidence

The integration workflow now executes `preflight`, one selector, fourteen domain suites and `ci-required` (17 jobs), while the older release auditor accepted the former 16-job shape and rejected the actual preflight as unknown. A green unit fixture with the same outdated shape concealed that mismatch.

The auditor now requires a completed successful job named exactly `preflight`. It does not filter the job away or accept arbitrary extra jobs. Missing, failed, skipped, cancelled, neutral, timed-out, pending, renamed and duplicate preflight evidence blocks approval even when the workflow wrapper says success. The suite count remains fourteen. Job order does not matter.

The existing sixteen regression tests now use the current shape and four additional tests cover preflight failures and counting. Existing checks for the exact current main SHA before and after collection, complete pagination, required suites, selector, final gate and both registered workflows remain unchanged. No workflow or branch protection is weakened. The report's `release_approved` remains false; this is only a CI-evidence check, not deployment authorization.

This implementation also corrects the diagnostic external-gate description to the approved 60-scenario scope and describes branch protection as something to verify, not automatically change. Independent staging, backup/restore, distinct-version rollback, S059 background trace correlation and final operational REL gates remain separate.

Acceptance: `python3 -m unittest discover -s tests/architecture -p 'test_release_candidate_audit.py' -v`, Repository rules and exact-head Integration CI. No PostgreSQL data or UI behavior changes; no deployment or production access. Full runtime testing remains covered by the unchanged required integration jobs.
