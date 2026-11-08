# TOR A-07 / S004 — Telegram recovery: operator acceptance

**Status:** draft contract for PR #719. This is not production enablement.
Sources: [Notion security design](https://app.notion.com/p/41232a84beec816aaad1df91d45ccb2e), [OWASP Forgot Password](https://cheatsheetseries.owasp.org/cheatsheets/Forgot_Password_Cheat_Sheet.html), [Telegram Bot API](https://core.telegram.org/bots/api).

## Runtime boundaries

- Default: `TOR_TELEGRAM_RECOVERY_ENABLED` is absent or `0`. The existing password login and disabled email delivery remain unchanged.
- Opt-in test environment: `TOR_TELEGRAM_RECOVERY_ENABLED=1`, `TOR_AUTH_ENABLED=1`, PostgreSQL schema including the additive A-07 migration, valid HTTPS public origin, `TOR_TELEGRAM_BOT_USERNAME`, and `TOR_TELEGRAM_BOT_TOKEN` injected from an external secret store.
- **No secrets in GitHub, Notion, screenshots or support logs.** Never paste a live bot token in an issue or a command line. The bot-token URL is constructed only in process memory.
- `getMe` must return the numeric bot id encoded in the token and the configured username (case-insensitive). `getWebhookInfo.url` must be empty. The worker refuses to poll or dispatch callbacks without both checks.
- Only one Bot API long-polling consumer may operate per bot id. The new PostgreSQL table `account_telegram_poller_leases` grants one database-timed 90-second lease with monotonic `lease_epoch` fencing. The worker renews before polling, notification dispatch and update handling, stops after lease loss, and releases only its own generation. Multi-instance restarts cannot renew a stale lease. Very long VM pauses may briefly overlap network requests before an old owner detects lease loss; callback and recovery DB writes remain idempotent and fenced by request state. Use a dedicated test bot to verify failover and Telegram HTTP 409 handling.
- Feature flags control **capability**, not site availability. A missing optional schema, malformed bot config, unreachable Telegram, active webhook, or invalid bot token must keep the core HTTP listener available. No Telegram-derived reset proof is issued until a verified Bot API update has been accepted and the originating browser exchanges its own random HttpOnly cookie.
- Invalid local config keeps the optional worker disabled with a sanitized warning. Transient schema/Bot API errors cause supervised retries with bounded delay; after sustained polling failures the worker revalidates bot identity and webhook mode. Failure logs must not contain token, email, username or request body.

## Fault injection checklist

| Condition | Expected result |
| --- | --- |
| Flag absent / disabled | No bot traffic, no Telegram routes exposed. Existing password login remains functional. |
| Enabled but bot credentials absent or invalid | Website serves existing accounts and reading, recovery worker unavailable, no reset token issued. |
| Enabled, valid config but Telegram API blocked | Website remains available; pending requests cannot be approved; backup-code recovery remains independent when PostgreSQL is healthy. |
| Enabled, optional A-07 migration absent | Worker does not poll; unrelated authentication and account-retention paths still work on legacy schema. |
| Configured username does not match `getMe` | Worker never polls or sends a message; no reset token issuance. |
| Telegram webhook already installed | Worker refuses long polling until webhook is cleared out of band by an operator. It never calls `deleteWebhook` on its own. |
| Telegram temporarily unavailable while serving readers | Only Telegram recovery degrades. The HTTP process stays up. After recovery, the bot identity/webhook checks run before polling resumes. |
| Two TOR replicas start with one bot token | Only one obtains the PostgreSQL poller lease; the standby must not call `getUpdates`. After expiry or release, standby takes over with a higher lease epoch. |
| Expired lease after a suspended worker | The stale owner cannot renew, handle a Telegram update, or release the new owner's generation. |
| Duplicate callback or replayed `update_id` | No second reset proof, no second approval; idempotency recorded in PostgreSQL. |
| Telegram callback with wrong `from.id`, wrong chat, or group | Cannot approve another user's browser request. |
| Another browser, expired request, revoked binding, or role elevation | Cannot exchange for a reset token even with a valid Telegram approval. |
| Change of password | Prior sessions and consumed reset token are invalidated. |

## Acceptance gates

1. Final exact-HEAD CI: Repository rules, Rust/Clippy, migrations, PostgreSQL tests, browser tests and account security regression must all be SUCCESS. A successful older commit does not count.
2. Use a **dedicated test bot and disposable database** for manual tests of `getMe`, webhook rejection, callback delivery, restart/replay, and three fault scenarios above. Real Telegram network tests cannot be substituted by local JSON decoding tests.
3. Confirm cross-replica lease contention, lease-epoch fencing, sanitized logs, monitoring/alerting for `TOR_TELEGRAM_RECOVERY_UNAVAILABLE` and `TOR_TELEGRAM_POLL_UNAVAILABLE`, and an independent rollback by disabling only the Telegram flag.
4. Review owner/moderator/administrator and financial-rights policies. Telegram alone is insufficient for privileged recovery; this PR must not silently widen eligibility.
5. Independent security review must verify no user/account enumeration, no recovery token leaks, CSRF + Origin enforcement, claim/approval atomicity and no account-takeover on actor switching.
6. Only after acceptance should PR #719 be marked ready; deployment and migrations remain separate, manual actions from an explicitly approved `main` SHA.

**Operator note:** BotFather/token creation, DNS and VPS changes are intentionally out of scope for this draft.
