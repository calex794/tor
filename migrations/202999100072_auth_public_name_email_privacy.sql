-- AUTH-PROFILE-03a: remove legacy own-mailbox public names without changing
-- account identity, credentials, handle, entitlements or authored records.
-- The account table is locked before the cleanup, so no concurrent writer can
-- introduce another leaking name between the backfill and constraint.
LOCK TABLE users IN SHARE ROW EXCLUSIVE MODE;

WITH repaired AS (
    UPDATE users
    SET display_name = 'Читатель №' || replace(gen_random_uuid()::text, '-', '')
    WHERE strpos(lower(display_name), email_normalized) > 0
    RETURNING id
)
INSERT INTO account_settings(user_id)
SELECT id FROM repaired
ON CONFLICT(user_id) DO UPDATE
SET revision = account_settings.revision + 1,
    updated_at = now();

ALTER TABLE users ADD CONSTRAINT users_public_name_excludes_own_email
    CHECK (strpos(lower(display_name), email_normalized) = 0);
