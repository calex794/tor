-- MAIL-RESET-02a: one account-owned proof per mail source, independent of source TTL.
ALTER TABLE password_reset_tokens ADD COLUMN password_reset_event_id BIGINT
    UNIQUE CHECK (password_reset_event_id > 0);

-- Called only inside the common mail preparation transaction, with budget
-- guard, account and outbox locks held. The caller must pass the existing
-- cost/lease/suppression gate; a source alone never authorizes proof issuance.
CREATE FUNCTION account_issue_password_reset_mail_proof(
    p_user BIGINT, p_event BIGINT, p_outbox BIGINT, p_generation INTEGER, p_digest BYTEA
) RETURNS BOOLEAN LANGUAGE plpgsql AS $$
DECLARE
    written BIGINT;
BEGIN
    WITH stamp AS MATERIALIZED (SELECT clock_timestamp() AS at)
    INSERT INTO password_reset_tokens
        (token_hash,user_id,credential_fingerprint,created_at,expires_at,password_reset_event_id)
    SELECT p_digest,u.id,s.credential_fingerprint,stamp.at,stamp.at+interval '30 minutes',s.id
    FROM account_password_reset_mail_requests s
    JOIN users u ON u.id=s.user_id
    JOIN mail_outbox m ON m.id=p_outbox AND m.generation=p_generation
    JOIN mail_send_reservations r ON r.outbox_id=m.id AND r.generation=m.generation
    JOIN mail_feedback_attempts f ON f.outbox_id=m.id AND f.generation=m.generation
    CROSS JOIN mail_send_budget_guard g
    CROSS JOIN stamp
    WHERE s.id=p_event AND s.user_id=p_user AND u.account_state='active'
      AND s.created_at>stamp.at-interval '10 minutes' AND s.created_at<=stamp.at
      AND s.credential_fingerprint=sha256(convert_to(u.password_hash,'UTF8'))
      AND m.purpose='password_reset' AND m.event_id=s.id AND m.recipient_user_id=u.id
      AND m.status='leased' AND m.lease_until>stamp.at
      AND r.purpose=m.purpose AND r.recipient_key=s.recipient_key
      AND r.registration_ip_key IS NOT DISTINCT FROM s.request_ip_key
      AND r.reserved_at>stamp.at-interval '120 seconds' AND r.reserved_at<=stamp.at
      AND date_trunc('month',r.reserved_at AT TIME ZONE 'Europe/Moscow')
          =date_trunc('month',stamp.at AT TIME ZONE 'Europe/Moscow')
      AND g.singleton AND g.key_binding=s.key_binding
      AND f.key_binding=s.key_binding AND f.recipient_key=s.recipient_key
      AND f.reserved_at=r.reserved_at
      AND NOT EXISTS (SELECT 1 FROM mail_recipient_suppression x
          WHERE x.recipient_key=s.recipient_key
            AND (x.suppressed_until IS NULL OR x.suppressed_until>stamp.at))
    ON CONFLICT (password_reset_event_id) DO NOTHING;
    GET DIAGNOSTICS written = ROW_COUNT;
    RETURN written=1;
END;
$$;

-- Revoke outstanding links on changes to the recovery destination or account
-- credentials/state. No resurrection after an email/state change and reversal.
CREATE OR REPLACE FUNCTION invalidate_password_reset_mail_request() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.email_normalized IS DISTINCT FROM NEW.email_normalized
        OR OLD.password_hash IS DISTINCT FROM NEW.password_hash
        OR OLD.account_state IS DISTINCT FROM NEW.account_state THEN
        DELETE FROM account_password_reset_mail_requests WHERE user_id=NEW.id;
        UPDATE password_reset_tokens SET consumed_at=clock_timestamp()
            WHERE user_id=NEW.id AND consumed_at IS NULL;
    END IF;
    RETURN NEW;
END;
$$;
