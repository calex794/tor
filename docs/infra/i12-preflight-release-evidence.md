# I-12 / S061: require current preflight in release evidence

The integration workflow now executes `preflight`, one selector, fourteen domain suites and `ci-required` (17 jobs), while the older release auditor accepted the former 16-job shape and rejected the actual preflight as unknown. A green unit fixture with the same outdated shape concealed that mismatch.

The auditor now requires a completed successful job named exactly `preflight`. It does not filter the job away or accept arbitrary extra jobs. Missing, failed, skipped, cancelled, neutral, timed-out, pending, renamed and duplicate preflight evidence blocks approval even when the workflow wrapper says success. The suite count remains fourteen. Job order does not matter.

The existing sixteen regression tests now use the current shape and four additional tests cover preflight failures and counting. Existing checks for the exact current main SHA before and after collection, complete pagination, required suites, selector, final gate and both registered workflows remain unchanged. No workflow or branch protection is weakened. The report's `release_approved` remains false; this is only a CI-evidence check, not deployment authorization.

This implementation also corrects the diagnostic external-gate description to the approved 60-scenario scope and describes branch protection as something to verify, not automatically change. Independent staging, backup/restore, distinct-version rollback, S059 background trace correlation and final operational REL gates remain separate.

Acceptance: `python3 -m unittest discover -s tests/architecture -p 'test_release_candidate_audit.py' -v`, Repository rules and exact-head Integration CI. No PostgreSQL data or UI behavior changes; no deployment or production access. Full runtime testing remains covered by the unchanged required integration jobs.

## Integration evidence, 2026-11-11

On original head `45ff4ad1582ebf33224fdf0cf91754a4a988548e`, Repository rules passed, but Integration run 43035329013, including its retry, did not pass. Browser setup failed before the corresponding browser scenarios could execute. The PostgreSQL/HTTP and isolated cutover checks passed; this does not substitute for the missing browser evidence.

The current main includes `tools/ci_prepare_ubuntu_mirror.sh` and the browser-job wiring that prepares a signature-verified Ubuntu mirror fallback. Revalidate this PR against that current main rather than repeatedly rerunning the obsolete merge checkout or weakening browser installation/tests. A prepared fallback, successful individual job, ready-for-review status, or green check from another SHA is not release approval.

After merging, collect both registered push workflows and every required job again for the exact resulting main SHA. Record a moved main, incomplete run or failed job as a blocker. Independent infrastructure and operational gates remain pending unless separately executed and evidenced.
