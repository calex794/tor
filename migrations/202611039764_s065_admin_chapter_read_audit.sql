-- S065 / C-12: immutable evidence of successful site-administrator
-- chapter content deliveries. No plaintext, S3 URLs, DEKs or session tokens.
-- No foreign keys: deletion of content/users must not erase the audit.
CREATE TABLE reader_admin_chapter_access_audit (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    admin_user_id BIGINT NOT NULL CHECK (admin_user_id > 0),
    chapter_id BIGINT NOT NULL CHECK (chapter_id > 0),
    version_id BIGINT NOT NULL CHECK (version_id > 0),
    action TEXT NOT NULL CHECK (
        action IN ('unsealed_body', 'sealed_chapter_grant', 'sealed_asset_grant')
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX reader_admin_chapter_audit_actor_recent
    ON reader_admin_chapter_access_audit (admin_user_id, id DESC);
CREATE INDEX reader_admin_chapter_audit_chapter_recent
    ON reader_admin_chapter_access_audit (chapter_id, id DESC);

-- The runtime may insert only after an A-authorized read. No runtime
-- transaction may mutate or delete an established audit entry.
CREATE FUNCTION reader_admin_chapter_audit_immutable()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'administrator reader audit is immutable'
        USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER reader_admin_chapter_audit_immutable_trigger
    BEFORE UPDATE OR DELETE ON reader_admin_chapter_access_audit
    FOR EACH ROW EXECUTE FUNCTION reader_admin_chapter_audit_immutable();
