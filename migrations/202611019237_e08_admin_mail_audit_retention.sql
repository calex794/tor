-- E-08/S057: explicit, operator-initiated lifecycle for admin mail audit.
-- No automatic schedule or retention duration is set by this migration.
-- Only the dedicated tor_migrator database role may purge before a supplied
-- past timestamp. Direct DELETE/UPDATE remains forbidden by row triggers.

CREATE TABLE social_mail_admin_audit_retention_runs (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    cutoff_before TIMESTAMPTZ NOT NULL,
    removed_count BIGINT NOT NULL CHECK (removed_count >= 0),
    executed_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
REVOKE ALL ON public.social_mail_admin_audit_retention_runs FROM PUBLIC;
REVOKE ALL ON SEQUENCE public.social_mail_admin_audit_retention_runs_id_seq FROM PUBLIC;

-- The PostgreSQL CI and deployment migration role grants runtime default
-- DML privileges on new tables and sequences. Remove them on this receipt.
DO $e08$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'tor_app') THEN
        REVOKE ALL ON public.social_mail_admin_audit_retention_runs FROM tor_app;
        REVOKE ALL ON SEQUENCE public.social_mail_admin_audit_retention_runs_id_seq FROM tor_app;
    END IF;
END
$e08$;

-- Replaces the original audit trigger function without changing the trigger.
-- A custom GUC is NOT authorization: current_user must be tor_migrator too.
CREATE OR REPLACE FUNCTION public.social_mail_admin_audit_no_mutation()
RETURNS trigger LANGUAGE plpgsql AS $e08$
BEGIN
    IF TG_OP = 'DELETE'
        AND current_user = 'tor_migrator'
        AND current_setting('tor.e08_mail_audit_purge', true) = 'enabled'
    THEN
        RETURN OLD;
    END IF;
    RAISE EXCEPTION 'admin mail access audit is immutable' USING ERRCODE = '23514';
END;
$e08$;

-- Receipts are immutable even when the privileged purge marker is active.
CREATE FUNCTION public.social_mail_admin_retention_receipt_immutable()
RETURNS trigger LANGUAGE plpgsql AS $e08$
BEGIN
    RAISE EXCEPTION 'admin mail audit retention receipt is immutable'
        USING ERRCODE = '23514';
END;
$e08$;
CREATE TRIGGER social_mail_admin_retention_receipt_immutable
    BEFORE UPDATE OR DELETE ON public.social_mail_admin_audit_retention_runs
    FOR EACH ROW EXECUTE FUNCTION public.social_mail_admin_retention_receipt_immutable();

-- Invoker rights (not SECURITY DEFINER). No runtime role or public EXECUTE.
-- Purge and immutable receipt are atomic in the operator's transaction.
CREATE FUNCTION public.social_mail_admin_audit_purge_before(p_cutoff TIMESTAMPTZ)
RETURNS BIGINT
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $e08$
DECLARE
    removed BIGINT;
BEGIN
    IF current_user <> 'tor_migrator' THEN
        RAISE EXCEPTION 'admin mail audit purge requires tor_migrator'
            USING ERRCODE = '42501';
    END IF;
    IF p_cutoff IS NULL OR p_cutoff >= clock_timestamp() THEN
        RAISE EXCEPTION 'admin mail audit purge cutoff must be in the past'
            USING ERRCODE = '22023';
    END IF;

    PERFORM set_config('tor.e08_mail_audit_purge', 'enabled', true);
    DELETE FROM public.social_mail_admin_access_audit
     WHERE created_at < p_cutoff;
    GET DIAGNOSTICS removed = ROW_COUNT;
    PERFORM set_config('tor.e08_mail_audit_purge', 'disabled', true);

    INSERT INTO public.social_mail_admin_audit_retention_runs
        (cutoff_before, removed_count)
    VALUES (p_cutoff, removed);
    RETURN removed;
END;
$e08$;
REVOKE ALL ON FUNCTION public.social_mail_admin_audit_purge_before(TIMESTAMPTZ) FROM PUBLIC;
DO $e08$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'tor_app') THEN
        REVOKE ALL ON FUNCTION public.social_mail_admin_audit_purge_before(TIMESTAMPTZ) FROM tor_app;
    END IF;
END
$e08$;
COMMENT ON FUNCTION public.social_mail_admin_audit_purge_before(TIMESTAMPTZ) IS
    'Operator-only manual deletion of admin mail audit rows before a chosen past cutoff with an atomic metadata-only receipt. No scheduled retention period.';
