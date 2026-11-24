# REUSE-04A — shared TOC keyboard and dismissal behavior

H/C/D. Extract the repeated Escape, Tab boundary/focus trap, close and
backdrop handling from the free, paid and archived reader TOC assets.
One shared function is inserted into a lexical IIFE in each existing
static asset bundle (no global helper, extra network request, new route
or CSP change). Existing asset URLs and cache headers remain unchanged.

Free keeps its existing focus sequence (close, links); paid and archived
keep their scroll viewport between close and links. Hidden nodes are
excluded. Closing restores the corresponding trigger focus. The existing
per-reader toggle handlers, scroll and overflow restoration, paid multi-TOC
coordination, current chapter positioning, cursor validation, endpoints,
metadata parsing, access states, purchase/version IDs and rendered links
remain unchanged. The common helper never accesses the network or decides
reading rights.

Acceptance: existing Repository rules executes the new Node tests against
all three real scripts with a controlled DOM; unit-level bundle proof and
the full existing Rust/PG/reader browser CI remain mandatory. No new job,
dependency, route, release or deployment. Further pagination refactoring
requires a separate equivalence review; these APIs are not interchangeable.

## Chromium acceptance correction

The first exact-head paid-private-e2e run failed at "desktop: footer TOC
does not close" because the ephemeral Node HTTP fixture manually appended
the paid TOC source without its new lexical helper. Repository rules and
all seven controlled-DOM tests passed; those tests did not cover this
separate bundle consumer. The Rust PG fixture now captures the actual
`/assets/newboo-paid-reader.mjs` route into its existing mode-0600 JSON
and both direct and Layer Chromium fixture servers use those exact bytes.
No permissive fallback to the raw source, no assertion removal, no change
to grants, lifetime, authorization, or production route. Require new
exact-head paid-private-e2e and all CI; the live browser result is pending.
