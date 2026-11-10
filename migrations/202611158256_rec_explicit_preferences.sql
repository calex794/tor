-- REC/FLOW v2 P1: explicit choices are account-owned business preferences.
-- Existing consent epochs and inferred reading/visit history are not copied.
-- SQLx applies the fence, snapshot and cutover in one transaction. Its trigger
-- DDL waits for earlier legacy writers; later legacy writes fail closed.
CREATE FUNCTION rec_reject_legacy_preference_mutation()
RETURNS trigger LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $$
BEGIN
    RAISE EXCEPTION USING
        ERRCODE = '55000',
        MESSAGE = 'legacy recommendation preferences are read-only; use the current runtime';
END;
$$;

CREATE TRIGGER rec_legacy_feedback_mutation_fence
    BEFORE INSERT OR UPDATE ON rec_feedback_state
    FOR EACH STATEMENT EXECUTE FUNCTION rec_reject_legacy_preference_mutation();
CREATE TRIGGER rec_legacy_command_mutation_fence
    BEFORE INSERT OR UPDATE ON rec_preference_commands
    FOR EACH STATEMENT EXECUTE FUNCTION rec_reject_legacy_preference_mutation();

CREATE TABLE rec_preference_accounts (
    user_id bigint PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    feedback_epoch bigint NOT NULL DEFAULT 0
        CHECK (feedback_epoch BETWEEN 0 AND 1000000)
);
CREATE TABLE rec_book_preferences (
    user_id bigint NOT NULL REFERENCES rec_preference_accounts(user_id) ON DELETE CASCADE,
    book_id bigint NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    hidden boolean NOT NULL,
    revision bigint NOT NULL CHECK (revision BETWEEN 1 AND 999999999),
    expires_at timestamptz NOT NULL DEFAULT now() + interval '30 days',
    PRIMARY KEY (user_id, book_id)
);
CREATE INDEX rec_book_preferences_expiration
    ON rec_book_preferences(expires_at, user_id);

-- Keep one command namespace for explicit feedback and both old consent APIs.
-- This is a bounded command receipt, not a behavioral event or a new profile.
CREATE TABLE rec_user_preference_commands (
    user_id bigint NOT NULL REFERENCES rec_preference_accounts(user_id) ON DELETE CASCADE,
    command_key bytea NOT NULL CHECK (octet_length(command_key) = 32),
    payload_hash bytea NOT NULL CHECK (octet_length(payload_hash) = 32),
    result_revision bigint NOT NULL CHECK (result_revision > 0),
    command_type text NOT NULL CHECK (command_type IN ('consent', 'feedback')),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, command_key)
);
CREATE INDEX rec_user_preference_commands_expiry
    ON rec_user_preference_commands(created_at);
CREATE INDEX rec_user_preference_commands_budget
    ON rec_user_preference_commands(user_id, created_at);

-- Preserve public CAS values even for disabled subjects and empty choices.
INSERT INTO rec_preference_accounts(user_id, feedback_epoch)
SELECT user_id, feedback_epoch FROM rec_subjects;
INSERT INTO rec_preference_accounts(user_id)
SELECT DISTINCT user_id FROM rec_preference_commands
ON CONFLICT (user_id) DO NOTHING;

-- Copy exact expiry and revision, including already expired rows awaiting the
-- existing bounded purge. Expired choices are never visible; the purge advances
-- the independent account epoch before discarding their CAS history.
INSERT INTO rec_book_preferences(user_id, book_id, hidden, revision, expires_at)
SELECT s.user_id, f.book_id, f.hidden, f.revision, f.expires_at
FROM rec_feedback_state f JOIN rec_subjects s ON s.subject_id = f.subject_id;
INSERT INTO rec_user_preference_commands(
    user_id, command_key, payload_hash, result_revision, command_type, created_at
)
SELECT user_id, command_key, payload_hash, result_revision, command_type, created_at
FROM rec_preference_commands;

-- Fail the whole migration if any source row would change during cutover.
DO $$
BEGIN
    IF EXISTS (
        SELECT user_id, feedback_epoch FROM rec_subjects
        EXCEPT SELECT user_id, feedback_epoch FROM rec_preference_accounts
    ) OR EXISTS (
        SELECT s.user_id, f.book_id, f.hidden, f.revision, f.expires_at
        FROM rec_feedback_state f JOIN rec_subjects s ON s.subject_id = f.subject_id
        EXCEPT SELECT user_id, book_id, hidden, revision, expires_at FROM rec_book_preferences
    ) OR EXISTS (
        SELECT user_id, command_key, payload_hash, result_revision, command_type, created_at
        FROM rec_preference_commands
        EXCEPT
        SELECT user_id, command_key, payload_hash, result_revision, command_type, created_at
        FROM rec_user_preference_commands
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = 'explicit preference migration did not preserve the legacy snapshot';
    END IF;
END;
$$;

-- One authoritative store. Old programs fail closed for preference mutations;
-- retaining their empty tables allows unrelated consent erasure/TTL deletes.
DELETE FROM rec_feedback_state;
DELETE FROM rec_preference_commands;

COMMENT ON TABLE rec_preference_accounts IS
    'F-owned explicit preference CAS fence. Independent of analytics consent; no behavioral history.';
COMMENT ON TABLE rec_book_preferences IS
    'F-owned account/book choice, not anonymous data. Exact existing 30-day expiry; no implicit analytics consent.';
COMMENT ON TABLE rec_user_preference_commands IS
    'F-owned shared explicit-command namespace. Original created_at retained; bounded 30-day replay receipts.';
