-- MAIL 5a: account-owned proof issuance for an exact, already charged
-- registration attempt. The canonical mail repository must hold account,
-- quota and outbox locks and validate the active DispatchGrant. It inserts
-- correlation and calls this function in ONE transaction, rolling back
-- correlation if this function returns false. This is not a send permit.
CREATE FUNCTION account_issue_registration_mail_proof(
    p_user BIGINT, p_event BIGINT, p_outbox BIGINT, p_generation INTEGER,
    p_revision BIGINT, p_marker TEXT, p_digest BYTEA
) RETURNS BOOLEAN LANGUAGE plpgsql AS $$
DECLARE
    written BIGINT;
BEGIN
    IF p_digest IS NULL OR octet_length(p_digest)<>32
       OR p_digest=decode(repeat('00',32),'hex') THEN
        RETURN false;
    END IF;
    -- The same account-first order used by account invalidation and reset.
    PERFORM id FROM users WHERE id=p_user AND account_state='active'
        AND mail_ownership_revision=p_revision FOR UPDATE;
    IF NOT FOUND THEN RETURN false; END IF;
    PERFORM singleton FROM mail_send_budget_guard WHERE singleton FOR UPDATE;
    IF NOT FOUND THEN RETURN false; END IF;
    PERFORM id FROM mail_outbox WHERE id=p_outbox AND generation=p_generation
        AND purpose='registration_confirmation' AND event_id=p_event
        AND recipient_user_id=p_user AND status='leased' FOR UPDATE;
    IF NOT FOUND THEN RETURN false; END IF;

    WITH stamp AS MATERIALIZED (SELECT clock_timestamp() AS at)
    INSERT INTO account_email_ownership
        (user_id,address_sha256,mailbox_revision,outbox_id,generation,
         challenge_sha256,issued_at,expires_at)
    SELECT u.id,sha256(convert_to(u.email_normalized,'UTF8')),u.mail_ownership_revision,
           m.id,m.generation,p_digest,stamp.at,stamp.at+interval '30 minutes'
    FROM users u
    JOIN account_registration_mail_sources s ON s.user_id=u.id
    JOIN mail_outbox m ON m.id=p_outbox AND m.generation=p_generation
    JOIN mail_send_reservations r ON r.outbox_id=m.id AND r.generation=m.generation
    JOIN mail_feedback_attempts f ON f.outbox_id=m.id AND f.generation=m.generation
    CROSS JOIN mail_send_budget_guard g
    CROSS JOIN mail_cost_policy policy
    CROSS JOIN stamp
    WHERE u.id=p_user AND p_event=u.id AND u.account_state='active'
      AND u.mail_ownership_revision=p_revision
      AND s.forwarded_at IS NOT NULL AND s.forwarded_at<=stamp.at
      AND s.created_at>stamp.at-interval '24 hours' AND s.created_at<=stamp.at
      AND m.purpose='registration_confirmation' AND m.event_id=u.id
      AND m.recipient_user_id=u.id AND m.status='leased'
      AND m.lease_until>stamp.at+interval '25 seconds'
      AND r.purpose=m.purpose AND r.recipient_key=s.recipient_key
      AND r.registration_ip_key IS NOT DISTINCT FROM s.registration_ip_key
      AND r.reserved_at>stamp.at-interval '120 seconds' AND r.reserved_at<=stamp.at
      AND date_trunc('month',r.reserved_at AT TIME ZONE 'Europe/Moscow')
          =date_trunc('month',stamp.at AT TIME ZONE 'Europe/Moscow')
      AND f.marker=p_marker AND f.provider_message_id IS NULL
      AND f.key_binding=s.key_binding AND f.recipient_key=s.recipient_key
      AND f.reserved_at=r.reserved_at
      AND g.singleton AND g.key_binding=s.key_binding
      AND policy.singleton AND policy.enabled
      AND NOT EXISTS (SELECT 1 FROM mail_recipient_suppression x
          WHERE x.recipient_key=s.recipient_key
            AND (x.suppressed_until IS NULL OR x.suppressed_until>stamp.at))
    ON CONFLICT DO NOTHING;
    GET DIAGNOSTICS written = ROW_COUNT;
    RETURN written=1;
END;
$$;
