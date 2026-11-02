-- E-08/S057: auditable administrator reads of legacy direct mail.
-- No admin-facing HTTP endpoint is introduced by this migration.
-- Store only actor, operation, numeric target and row count, never messages.
-- Historical audit IDs intentionally do not foreign-key users or conversations:
-- deletion of an account must not silently erase administrator access records.
CREATE TABLE social_mail_admin_access_audit (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    admin_user_id BIGINT NOT NULL CHECK (admin_user_id > 0),
    action TEXT NOT NULL CHECK (
        action IN ('search_mail_users', 'list_user_mailbox', 'read_direct_conversation')
    ),
    target_user_id BIGINT CHECK (target_user_id IS NULL OR target_user_id > 0),
    conversation_id BIGINT CHECK (conversation_id IS NULL OR conversation_id > 0),
    result_count INTEGER NOT NULL CHECK (result_count BETWEEN 0 AND 50),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK (
        (action='search_mail_users' AND target_user_id IS NULL AND conversation_id IS NULL)
        OR (action='list_user_mailbox' AND target_user_id IS NOT NULL AND conversation_id IS NULL)
        OR (action='read_direct_conversation' AND target_user_id IS NULL AND conversation_id IS NOT NULL)
    )
);
CREATE INDEX social_mail_admin_audit_actor
    ON social_mail_admin_access_audit (admin_user_id, id DESC);

-- An app account may insert through the guarded adapter, not rewrite history.
CREATE FUNCTION social_mail_admin_audit_no_mutation()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'admin mail access audit is immutable' USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER social_mail_admin_access_immutable
    BEFORE UPDATE OR DELETE ON social_mail_admin_access_audit
    FOR EACH ROW EXECUTE FUNCTION social_mail_admin_audit_no_mutation();
