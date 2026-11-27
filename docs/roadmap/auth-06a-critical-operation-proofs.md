# AUTH-06a — critical operation proofs and rolling sessions

Retain the existing one-time, exact-operation-bound step-up architecture.
Refunds, internal creator payout posting, book ownership transfer and
book revenue-share changes already use a password capability (30 seconds)
and a separate one-use proof (at most 5 minutes). A 30-day ordinary session
must never mint, extend or reset these proofs by mere activity.

Match the book proof issuer's fresh-password recheck after acquiring SQL
locks in the finance issuer. No new proof store, generic privileged cookie
or change to money/ownership business rules. Existing owner and admin
session revocation already verify the password immediately before mutation.

The combined credentials_pg test exercises all four proof scopes, compares
all stored proof fields across ordinary renewal, including expired unused
and expired consumed states, and checks that an owner-revoked session cannot
renew or obtain a new proof. Existing finance/book consumption tests remain
mandatory; no claim of acceptance until combined CI succeeds.

Follow-up security review: S3 connection changes currently check live admin
role and CSRF but do not ask for the administrator password. Review that
write separately without expanding provider scope or logging credentials.
Separate admin session lifetime and absolute normal-session limit have not
been approved and are not introduced here. No deployment.
