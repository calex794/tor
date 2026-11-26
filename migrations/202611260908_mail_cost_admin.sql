-- MAIL-01h2: optimistic operator settings and auditable, stop-only controls.
ALTER TABLE mail_cost_policy ADD COLUMN revision BIGINT NOT NULL DEFAULT 1 CHECK (revision>0);
CREATE TABLE mail_cost_admin_audit (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_user_id BIGINT NOT NULL CHECK(actor_user_id>0),
    action TEXT NOT NULL CHECK(action IN ('limits','stop')),
    old_revision BIGINT NOT NULL CHECK(old_revision>0),
    new_revision BIGINT NOT NULL CHECK(new_revision=old_revision+1),
    old_enabled BOOLEAN NOT NULL,
    new_enabled BOOLEAN NOT NULL,
    old_warning_1 BIGINT NOT NULL,
    old_warning_2 BIGINT NOT NULL,
    old_news_pause BIGINT NOT NULL,
    old_monthly_limit BIGINT NOT NULL,
    new_warning_1 BIGINT NOT NULL,
    new_warning_2 BIGINT NOT NULL,
    new_news_pause BIGINT NOT NULL,
    new_monthly_limit BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK(NOT new_enabled OR old_enabled)
);
-- Historical receipt must survive user deletion and cannot be rewritten.
CREATE FUNCTION mail_cost_admin_audit_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'mail cost audit is append-only';
END;
$$;
CREATE TRIGGER mail_cost_admin_audit_immutable
BEFORE UPDATE OR DELETE ON mail_cost_admin_audit
FOR EACH ROW EXECUTE FUNCTION mail_cost_admin_audit_immutable();
