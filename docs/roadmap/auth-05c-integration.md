# AUTH-05c — combined session security acceptance

Integrate the exact AUTH-02, AUTH-03, AUTH-04 and AUTH-05 changes from
PRs #1295, #1298, #1299 and #1303 onto main 3c433d846e89bec21cc57b3398f760499932f745.
Do not replace newer unrelated commerce changes with the older AUTH-03 tree.

Owner bulk revoke and its count are recorded atomically, with no event on a
zero-row retry. The account-visible 90-day timeline projects the existing
administrator audit, without exposing the administrator ID or session UUID.
A revoked session cannot renew. The frozen legacy analytics fixture tests
seed revocation directly on their deliberately historical schema rather than
calling the current audit-writing command; production never bypasses the new
audit table. Preserve all historical migration checksums and retirement guards.

The combined CI is the acceptance gate; do not deploy or merge independent
drafts on the strength of their isolated tests. AUTH-06/07/08 remain subsequent
critical-operation, performance and restart/restore acceptance work.
