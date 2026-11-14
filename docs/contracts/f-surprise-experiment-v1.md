# F-SURPRISE-04B: account-level reading experiment V1

Owner: F discovery; C owns canonical reading; I owns the SQL bridge and process wiring.
Source: https://app.notion.com/p/41432a84beec815b8b31d9280272a9ba
Predecessor: PR #834. Implementation/acceptance: PR #837.

This contract does not activate a live experiment, deploy TOR, apply production
migrations, or establish a real-world reading benefit. The UI and database flags
remain OFF by default. Passing CI proves the measurement mechanism, not uplift.

## Frozen protocol

An administrator explicitly starts a run with a fixed appearance probability.
Enrollment lasts 336 hours, followed by 192 hours of follow-up for the last
participant. Each participant is observed for exactly 192 hours. No efficacy
result is declared until the fixed enrollment and follow-up horizon has ended.
The independently randomized groups are control (20%, no gifts), gentle (40%,
half the configured appearance chance), and standard (40%, configured chance).
The run ID has its own HMAC assignment domain. Assignment is persisted before
the daily appearance draw. Reloads, devices, clicks, non-selection, later
disablement, and key rotation cannot reassign an already enrolled account.
A daily receipt from another run cannot be reused; a concurrent run change
rejects an assignment calculated for the old run.

The target population is eligible signed-in accounts requesting a plan while
the mechanism and experiment are enabled. Existing opt-outs are not enrolled.
Every account enrolled in a run stays in that run's denominator, including
non-openers, later opt-outs and account deletion. Deletion removes the identity
link but not its anonymous count. Do not restrict outcomes to gift recipients.

Changing probability or switching off the experiment stops the run and turns
all gifts OFF. It never puts control into ordinary gifts. A later start creates
a new run, with separate receipts and frozen results. There is no automatic
winner rollout or repeated extension until a desirable result appears.

## Canonical outcomes

The primary binary outcome is at least one C-verified chapter completion in the
first 168 hours for a book with no pre-enrollment canonical chapter start or
completion. This is a stricter operational definition than a one-character
start. It is not proof of attention or knowledge, and cannot reconstruct reading
before canonical C records existed. Its name in the interface is explicit.

Completion requires the existing C entitlement/edition/monotone-CAS validation
and matching durable `ChapterCompletedV1` outbox authority. A gift open, URL
click, page render, forged event, or positive cursor alone cannot create this
outcome. The same bridge accepts free and purchased canonical completions; it
never grants access or copies paid chapter text. Paid acceptance uses an actual
pinned V1 after publication of V2 and rejects foreign and revoked purchases.

Guardrails are binary per enrolled account:

* Completion within 168 hours of the most recent unfinished chapter with positive
  canonical progress in the 24 hours BEFORE assignment. Only this pre-treatment
  eligible subset is used as its denominator.
* D1 positive canonical reading at elapsed hours [24,48); D7 at [168,192).
* Disabling gifts within [0,192), retained even if later re-enabled.

The C projection writes only sticky outcome bits in one bounded F receipt,
not a new click, visit, device, book-history or raw reading-event stream. No new
consent screen or external analytics collector is added. Legacy individual FLOW
collectors stay retired. C remains authoritative; F does not manufacture reads.

## Interpretation and stopping

All displayed rates use complete 192-hour observation windows. Fewer than 20
accounts in any arm suppresses the comparison; small guardrail denominators are
also hidden. At least 1000 mature participants in EVERY arm are required for an
inferential result. This is only a floor, NOT a power calculation or a promise
that 5000 total participants establish a commercially relevant effect.

For a binary rate with n observations, the fixed-horizon radius is
`sqrt(log(2*18/0.05)/(2*n))`, clipped to 1. Eighteen simultaneous proportions cover
three arms times six metrics (primary, chapter completion, D1, D7, disablement,
prior-reader baseline). A treatment-control interval uses the point difference
plus/minus the sum of its two rate radii, clipped to [-1,1]. These conservative
Hoeffding/union-bound intervals do not assume a nonzero success rate or use a
fragile normal approximation at zero events.

A supported treatment requires a primary lower bound above zero AND lower
bounds for all reading guardrails at least -3 percentage points AND a disable
upper bound no greater than +5 points. Missing chapter eligibility cannot prove
non-inferiority. Baseline prior-reader imbalance invalidates interpretation.
The default 3/5-point tolerances are V1 protocol settings, not empirical claims.

Interim safety checks use error allocation `0.01/(18*n*(n+1))` per proportion;
the series over n sums to one. Allocation checks similarly use
`0.001/(3*N*(N+1))` over the three marginal arm shares. Thus repeated dashboard
checks cannot repeatedly spend the same fixed-horizon significance budget.
There is no interim efficacy claim. Uncertainty is reported as uncertainty,
not as no effect. Independent account-level sampling and an intact randomized
protocol remain assumptions; no finite CI can repair contamination.

Automatic stopping also covers capture failure, invalid counters, assignment
mismatch, expired horizon, missing heartbeat over two minutes, and disabled
server delivery. Capture faults immediately fence new experimental delivery,
independently of the next worker tick. The worker runs about once per minute,
even when collection is OFF. A failed optional capture has a 25ms lock budget,
increments a nontransactional fault counter and invalidates the run rather than
breaking an otherwise valid C write or silently counting an unmeasured zero.

Captures hold a KEY SHARE run fence, compatible with ordinary non-key heartbeat
updates. Every transition out of running upgrades to FOR UPDATE in a database
trigger, before freezing totals. This includes disabling gifts, so a concurrent
opt-out cannot silently disappear from final counts. The locking semantics are
defined by https://www.postgresql.org/docs/18/explicit-locking.html .
If a broken/oversized aggregate prevents normal maintenance, independent bounded
recovery seals the stale run as invalid and purges expired receipts without
re-running the failed aggregate. Its last measured snapshot is not a result.

## Lifecycle and administration

Only a live administrator with valid session, Origin and CSRF may change the
policy. The audit records the actual guarded state returned by PostgreSQL.
The report is private/no-store and exposes no participant identities. A failure
of the new reading report does not remove the administrative policy controls.

Run receipts and old daily decisions are purged after 720 hours / 30 UTC days
in batches of at most 1000 each, with SKIP LOCKED. Closed-run aggregate totals
remain. User disable/cooldown settings are not deleted by analytics retention.
A backlog or failed maintenance tick emits a bounded operational warning, not
raw account information. Long downtime fences delivery; recovery does not
silently resume a contaminated experiment.

## Required acceptance evidence

* Pure service: `cargo test -p tor-recommendations surprise_experiment`.
* Current migrated disposable database only:
  `cargo test -p tor-web --test surprise_experiment_pg -- --ignored --test-threads=1`.
  Five scenarios call the real free/paid C CAS and exercise proof authority,
  control/non-opener inclusion, D1/D7, disablement, deletion, lock-contention
  failure, heartbeat compatibility, independent cleanup, insufficient sample,
  server-gate shutdown and automatic guardrail stop.
* Chromium: `tools/surprise_experiment_pg_browser.cjs` through
  `tools/rec13_browser_acceptance.sh`, covering live admin HTTP, desktop/mobile,
  suppression, forged form field rejection, all-gifts OFF and role revocation.
* Existing surprise HTTP/PG and gift Chromium regression, SQLx migrations,
  Repository rules, and full Integration CI on the exact final HEAD; repeat
  main validation after merge. Never infer that skipped tests passed.

Live sample collection, business impact and activation are separate owner
choices after technical acceptance. Complaints, layout shift and long-task
telemetry are not inferred from reading counters; existing browser accessibility
and non-obstruction acceptance remains mandatory for the gift UI.
