# Admin entry: /одмен (Cyrillic)

User-facing administrative entry:
https://pandaolate.ru/одмен

The browser encodes the route as
/%D0%BE%D0%B4%D0%BC%D0%B5%D0%BD. Only authenticated administrators
may see this page; anonymous visitors and non-admins receive the same 404.
The existing account page shows a single private administrative entry link
instead of three deep links.

**This alias is NOT a security boundary.** Other administrator-only pages
such as /admin/audience, /admin/support and /admin/finance/refunds
**remain accessible** at their original addresses, subject to their existing
authorization policies. This PR intentionally does not change their paths,
form POST handlers, API routes or route intelligence registry.
It provides a private, recognizable starting page, not a complete obscuring
rename of the administrative namespace. Renaming the entire namespace is a
separate change requiring form/redirect/asset/route-registry acceptance.
Do not falsely represent /admin/* as absent, disabled or hidden.

All authentication, CSRF, session, administrator role, finance approval
and abuse controls retain their original semantics. The URL is not a substitute
for strong credentials, second-factor authentication, access logging or
network-layer protection. No VPS deployment or Nginx edit in this PR.
