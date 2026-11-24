# Admin entry: /odmen (ASCII)

User-facing administrative entry:
https://pandaolate.ru/odmen

The route is ASCII and does not require URL encoding. Only authenticated
administrators may see this page; anonymous visitors and non-admins
receive the same 404.
The account page adds a private /odmen entry while retaining the three
existing deep links required by browser tests and workflows. This is NOT a
full masking of /admin/*; do not claim that it hides the namespace.

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
