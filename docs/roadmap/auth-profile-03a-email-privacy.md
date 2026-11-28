# AUTH-PROFILE-03a — public identity privacy

A-02 / S002, A-06 / S004, E-03 / E-08 public projections.

The account repository returns a minimal public identity and the ordinary profile form rejects
the user's own email in a public name. However older accounts may already have stored it.
Comment mention candidates and public profile lookup read display_name directly.

The additive migration repairs only names containing the account's normalized email
(case-insensitive) to a neutral, independent random reader label. It keeps user ID,
email, password hash, claimed handle and relations unchanged, and advances settings
revision for repaired accounts. Valid names are not modified. The old email-bearing
display-name URLs must no longer resolve; no redirect preserves the private address.

A validated database constraint blocks reintroduction through any users-table writer.
The profile adapter maps that specific constraint to the existing private-email error.
Fresh and installed-upgrade PostgreSQL fixtures cover direct inserts/updates,
existing handles/revisions, identity and authored book retention.

Scope: this does not prove that historical cached notification payloads, user-authored
free text, request logs, admin exports or previously indexed URLs contain no email.
Those surfaces require separate review. No production migration or deployment is
authorized by merging the source PR.
