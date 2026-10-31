-- E-11 / S050-S051: capability-scoped guest support, separate from social mail.
CREATE TABLE support_conversations (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    requester_user_id BIGINT UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    guest_identity_hash BYTEA UNIQUE CHECK(octet_length(guest_identity_hash)=32),
    guest_alias TEXT NOT NULL DEFAULT '',
    expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK((requester_user_id IS NULL AND guest_identity_hash IS NOT NULL AND expires_at IS NOT NULL)
       OR (requester_user_id IS NOT NULL AND guest_identity_hash IS NULL AND expires_at IS NULL))
);
CREATE INDEX support_guest_expiry ON support_conversations(expires_at,id) WHERE expires_at IS NOT NULL;
CREATE TABLE support_messages (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    conversation_id BIGINT NOT NULL REFERENCES support_conversations(id) ON DELETE CASCADE,
    sender_kind TEXT NOT NULL CHECK(sender_kind IN('guest','user','admin')),
    sender_scope TEXT NOT NULL,
    sender_user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
    body TEXT NOT NULL CHECK(char_length(body) BETWEEN 1 AND 4000),
    client_message_id TEXT CHECK(char_length(client_message_id) BETWEEN 1 AND 128),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(conversation_id,sender_scope,client_message_id)
);
CREATE INDEX support_messages_page ON support_messages(conversation_id,id DESC);
CREATE TABLE support_admin_reads (
    conversation_id BIGINT NOT NULL REFERENCES support_conversations(id) ON DELETE CASCADE,
    admin_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    last_message_id BIGINT NOT NULL CHECK(last_message_id>0),
    PRIMARY KEY(conversation_id,admin_user_id)
);
-- A consumed guest capability can be replayed by its claimant, never reassigned.
CREATE TABLE support_guest_claims (
    guest_identity_hash BYTEA PRIMARY KEY CHECK(octet_length(guest_identity_hash)=32),
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    conversation_id BIGINT NOT NULL REFERENCES support_conversations(id) ON DELETE CASCADE,
    claimed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- I-owned common administrative journal, first writer: support replies/read markers.
-- No FK cascade: deleting an account must not erase its administrative history.
CREATE TABLE admin_action_audit (
    audit_id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_id BIGINT NOT NULL CHECK(actor_id>0),
    action TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    resource_id BIGINT NOT NULL CHECK(resource_id>0),
    request_id TEXT NOT NULL CHECK(char_length(request_id) BETWEEN 1 AND 128),
    command_key TEXT,
    before_revision BIGINT,
    after_revision BIGINT,
    outcome TEXT NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX admin_support_reply_once ON admin_action_audit(resource_id)
    WHERE action='support.reply' AND outcome='success';
CREATE FUNCTION protect_admin_action_audit() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'administrative audit is append-only' USING ERRCODE='42501'; END;
$$;
CREATE TRIGGER admin_action_audit_immutable BEFORE UPDATE OR DELETE ON admin_action_audit
    FOR EACH ROW EXECUTE FUNCTION protect_admin_action_audit();
