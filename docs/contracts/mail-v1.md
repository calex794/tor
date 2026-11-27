# MAIL — outbound system email

Decision: 26 November 2026. Keep password-based registration and sessions. No email-code login. This is a new outbound channel, not the existing internal /mail conversations.

## MAIL-01 (I/G): durable reference queue

Registration confirmation, support reply notification and project news are the only purposes. A single trusted event/recipient/purpose is unique. The queue stores numeric references, no addresses, message text, tokens or secrets. Claim is atomic, bounded and prioritized; no database transaction stays open during network I/O. A stale generation or expired claim cannot finalize a row. SMTP acceptance is not delivery. Expired claims become uncertain and cannot be blindly replayed. No live dispatch or HTTP integration in this change.

The first isolated PostgreSQL test is `cargo test -p tor-mail --test slices_pg -- --ignored` with migrated disposable local tor_ci, plus `cargo test -p tor-mail -p tor-contracts`. The dedicated test must be wired into the applicable PG CI before acceptance.

## Subsequent acceptance gates

- MAIL-02: secure SMTP adapter with a maintained MIME/TLS library; first provider Yandex Cloud Postbox, fixed postbox.cloud.yandex.net:465 (SMTPS) or 587 (STARTTLS required). No plaintext fallback; bounded timeouts, static/redacted errors; local mock SMTP acceptance/rejection/ambiguity tests; safe retry only after definitive non-acceptance. Operator controls both activation and quotas. Respect 200/day and 1/sec until provider approves higher quotas.
- MAIL-03: /odmen email provider configuration, admin authorization, Origin/CSRF, encrypted credentials with versioned recovery key, test-send separate from activation; no secrets in HTML, logs, GitHub or database dumps without encryption. Reuse established S3-admin security patterns without reusing its key.
- MAIL-04: worker startup opt-in, source-event integration, retry scheduling, limits and bounded retention, provider failure independent of the website. The renderer must recheck account/source ownership and news subscription immediately before SMTP; pending references must not bypass that check.
- MAIL-05: registration confirmation via one-time verification link (not authentication); preserve existing password and session behavior. Token storage and expiry belong to accounts. Do not force existing accounts to re-register. A future activation-policy change needs a separate decision.
- MAIL-06: outgoing support reply notification, no private conversation text in email; authenticated link back to the site. Guest support remains functional without a mailbox. Receiving replies at support@ requires separate inbound email routing/mailbox; Postbox alone is outbound only.
- MAIL-07: opt-in site news, explicit unsubscribe, send once per publication, recheck consent at dispatch, no automatic subscription on registration. News cannot exhaust capacity reserved for transactional messages; batch/checkpoint and abuse controls.
- MAIL-08: real VPS/Postbox SMTP acceptance and inbox delivery, monitoring, restore credentials and queue from backup, manual production deployment from checked main. No automatic deploy or real emails in CI.

Sources: existing TOR decisions-v1/api-v1/data-and-events, account registration and support service; Yandex SMTP documentation https://yandex.cloud/ru/docs/postbox/operations/send-email . DNS and live credentials are not part of the repository.
