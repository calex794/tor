# FLOW CI network and install diagnostic

Initial read-only probe:
https://github.com/calex794/tor/actions/runs/43038921078/job/130585159742

On that Ubuntu 24.04 runner, GitHub, npm and kernel.org DNS resolved; all three
stock Ubuntu mirror hostnames timed out. Kernel.org returned HTTP 200 for Noble
InRelease and gpgv verified the stock Ubuntu archive signing key. This is evidence
from one runner, not a claim about all networks or a global service outage.

This branch now validates a candidate CI-only helper. Only this exact WIP branch
triggers the diagnostic workflow. The normal acceptance workflows are unchanged.

The helper retains the existing mirrorlist when any stock Ubuntu host resolves.
Only when all three bounded DNS probes fail does it verify HTTPS InRelease for
noble, noble-updates, noble-backports and noble-security with the installed Ubuntu
archive keyring, check each Suite, then prepend Kernel.org with TAB priority:0.
The other mirrors, ubuntu.sources, suites, components and signing keys remain.
APT keeps its normal signature, hash and freshness checks; no fixed IP or custom
DNS resolver is introduced.

Validation checks shell syntax, unchanged sources/keyring and idempotence, then
installs Playwright 1.56.1 with all Chromium system dependencies and fonts. It
launches both the headless shell and full Chromium. Any failure is a real failure.
This remains dependency/install validation; application, database, browser journey
and full Integration CI acceptance still require their ordinary jobs.

No production credentials, VPS, database or deployment are involved.
