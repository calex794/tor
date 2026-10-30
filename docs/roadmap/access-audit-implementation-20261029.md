# ACCESS-AUDIT-01/02 — implementation slice

Source: NEwBOO user_access_audit_service.py. Spec: ACCESS-AUDIT-01…06 in the anti-bot roadmap branch.

Registration and password login create session and private audit event atomically. The IP is either a verified socket/Nginx client address or NULL. User-Agent is reduced to a bounded browser family. The migration preserves old token hashes. No password, cookie or raw User-Agent is stored in the event. Test: cargo test -p tor-web --test account_access_audit_pg -- --ignored --test-threads=1. GeoIP, last-seen, known IP projection, retention and /me/security are out of scope. Do not deploy raw IP storage to the VPS before retention and privacy controls are added.
