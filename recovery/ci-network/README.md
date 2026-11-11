# FLOW CI network diagnostic

This branch is an isolated read-only diagnostic for repeated APT/DNS failures in
Integration CI runs 43033220748, 43033674756 and 43033924354 on 2026-11-11.

It does not install packages, modify APT sources or DNS configuration, check out
application source, access a database, deploy, read repository secrets, or change
the standard acceptance workflows. Only this exact WIP branch triggers the job.

The job checks DNS for public package sources and controls, then downloads bounded
public Ubuntu InRelease metadata over HTTPS and verifies it with the stock runner's
Ubuntu archive keyring. A mirror will not be adopted merely because DNS resolves:
its signed metadata and the actual package-install path must be proved first.

Any failed probe makes the diagnostic job fail after all bounded probes complete.
A successful diagnostic run is not application, browser, migration or CI acceptance.
The main branch and PR #825/#826 remain subject to their full existing gates.

Base main: 149e695eefd22465568378c0d15d9ba465bedfac.
