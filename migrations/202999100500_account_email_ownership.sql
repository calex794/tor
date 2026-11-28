-- MAIL 5/6: account-owned email-address proof, independent of login and news consent.
-- No legacy account is implicitly verified; only the current mailbox may qualify.
ALTER TABLE users ADD COLUMN mail_ownership_revision BIGINT NOT NULL DEFAULT 1
    CHECK (mail_ownership_revision>0);

-- A stale in-memory candidate must not become valid again after an
-- address or state changes away and back. Other account edits do not
-- alter this revision, and a direct revision update cannot rewind it.
CREATE FUNCTION advance_account_mail_ownership_revision() RETURNS trigger LANGUAGE plpgsql AS $
BEGIN
    NEW.mail_ownership_revision := CASE
        WHEN OLD.email_normalized IS DISTINCT FROM NEW.email_normalized
          OR OLD.account_state IS DISTINCT FROM NEW.account_state
        THEN OLD.mail_ownership_revision+1
        ELSE OLD.mail_ownership_revision
    END;
    RETURN NEW;
END;
$;
CREATE TRIGGER account_mail_ownership_revision
    BEFORE UPDATE OF email_normalized, account_state, mail_ownership_revision ON users
    FOR EACH ROW EXECUTE FUNCTION advance_account_mail_ownership_revision();

CREATE TABLE account_email_ownership (
    user_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    address_sha256 BYTEA NOT NULL CHECK (octet_length(address_sha256)=32),
    challenge_sha256 BYTEA CHECK (octet_length(challenge_sha256)=32),
    issued_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ,
    verified_at TIMESTAMPTZ,
    CHECK (
        (verified_at IS NULL AND challenge_sha256 IS NOT NULL AND expires_at IS NOT NULL
         AND expires_at>issued_at AND expires_at<=issued_at+interval '30 minutes')
        OR
        (verified_at IS NOT NULL AND verified_at>=issued_at
         AND challenge_sha256 IS NULL AND expires_at IS NULL)
    )
);
CREATE UNIQUE INDEX account_email_ownership_challenge_uq
    ON account_email_ownership(challenge_sha256)
    WHERE challenge_sha256 IS NOT NULL;

-- Invalidate both verified ownership and outstanding challenges atomically
-- even when an old address is later restored. Lock order: account, then proof.
CREATE FUNCTION invalidate_account_email_ownership() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM account_email_ownership WHERE user_id=NEW.id;
    DELETE FROM account_registration_mail_sources WHERE user_id=NEW.id;
    RETURN NEW;
END;
$$;
CREATE TRIGGER account_email_ownership_invalidation
    AFTER UPDATE OF email_normalized, account_state ON users
    FOR EACH ROW
    WHEN (OLD.email_normalized IS DISTINCT FROM NEW.email_normalized
          OR OLD.account_state IS DISTINCT FROM NEW.account_state)
    EXECUTE FUNCTION invalidate_account_email_ownership();

-- Only the original committed signup is eligible for the initial handoff.
-- Keep the existing seven-day retention; initial forwarding expires in 24h.
ALTER TABLE account_registration_mail_sources ADD COLUMN forwarded_at TIMESTAMPTZ;
CREATE INDEX account_registration_mail_sources_pending
    ON account_registration_mail_sources(created_at,user_id) WHERE forwarded_at IS NULL;
