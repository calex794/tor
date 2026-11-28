-- MAIL-RESET-01b: short-lived account-owned references, never reset secrets.
CREATE TABLE account_password_reset_mail_requests (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id BIGINT NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    credential_fingerprint BYTEA NOT NULL CHECK (octet_length(credential_fingerprint)=32),
    recipient_key BYTEA NOT NULL CHECK (octet_length(recipient_key)=32),
    request_ip_key BYTEA CHECK (octet_length(request_ip_key)=32),
    key_binding BYTEA NOT NULL CHECK (octet_length(key_binding)=32),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    forwarded_at TIMESTAMPTZ,
    CHECK (forwarded_at IS NULL OR forwarded_at >= created_at)
);
CREATE INDEX account_password_reset_mail_pending
    ON account_password_reset_mail_requests(id) WHERE forwarded_at IS NULL;
CREATE INDEX account_password_reset_mail_expiry
    ON account_password_reset_mail_requests(created_at,id);

-- An account change must not let an old request revive after reactivation
-- or restoration of a previous email spelling. Lock order: account -> source.
CREATE FUNCTION invalidate_password_reset_mail_request() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.email_normalized IS DISTINCT FROM NEW.email_normalized
        OR OLD.password_hash IS DISTINCT FROM NEW.password_hash
        OR OLD.account_state IS DISTINCT FROM NEW.account_state THEN
        DELETE FROM account_password_reset_mail_requests WHERE user_id=NEW.id;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER account_password_reset_mail_invalidation
    AFTER UPDATE OF email_normalized,password_hash,account_state ON users
    FOR EACH ROW EXECUTE FUNCTION invalidate_password_reset_mail_request();
