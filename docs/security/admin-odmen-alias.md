# TOR administrative URL cutover

**User entry:** https://pandaolate.ru/odmen (ASCII).
This page requires a verified active administrator session. Anonymous users
and authenticated non-administrators receive 404 with noindex and private
cache-control behavior. All child handlers retain their existing server-side
administrator/owner checks, Origin, CSRF and financial approval invariants.

**All old /admin HTML and form routes are no longer registered** in the
application. A request to /admin or /admin/... must return 404, not a
redirect to a discovered admin address. Existing admin navigation, action
forms and redirects now use /odmen/....

The administrator-only API path family formerly at /api/v1/admin/...
is named /api/v1/odmen/...; the privileged chapter read path formerly
/read/admin/{id} is /read/odmen/{id}. The remaining administrative
chapter API action segments use odmen-unsealed, odmen-history and
odmen-sealed-history. These are route changes, **not permission changes**.
No old aliases are installed.

**NOT a security boundary.** Changing URL spelling does not conceal
publicly linked actions nor defeat a determined scanner. Session
authorization, CSRF enforcement, independent role proof, rate limits,
monitoring and safe Nginx configuration remain mandatory. Avoid assuming
the admin interface is protected merely because the route is unfamiliar.

Rollout: apply source and tests together; no redirect or old route fallback.
This change requires a normal pinned-main release and a separate approval
for touching the working VPS. It does not deploy or change Nginx.
