-- E-08: a group must not lose its current creator when an account is
-- closed or hard-deleted. Handoff is an explicit group-member decision.
-- This is a new SQLx migration; prior migration checksums stay unchanged.
CREATE FUNCTION public.social_group_creator_account_guard()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $e08$
DECLARE
    requires_handoff BOOLEAN := FALSE;
BEGIN
    IF TG_OP = 'DELETE' THEN
        requires_handoff := TRUE;
    ELSIF TG_OP = 'UPDATE' THEN
        requires_handoff := NEW.account_state = 'closed'
            AND OLD.account_state <> 'closed';
    END IF;

    IF requires_handoff AND EXISTS (
        SELECT 1 FROM public.social_group_members m
        WHERE m.user_id = OLD.id
          AND m.role = 'creator'
          AND m.status = 'active'
    ) THEN
        RAISE EXCEPTION 'transfer group creator role before deleting or closing this account'
            USING ERRCODE = '23514';
    END IF;

    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$e08$;

CREATE TRIGGER social_group_creator_account_guard
    BEFORE DELETE OR UPDATE OF account_state ON public.users
    FOR EACH ROW EXECUTE FUNCTION public.social_group_creator_account_guard();

COMMENT ON FUNCTION public.social_group_creator_account_guard() IS
    'Prevent deletion or closure of active group creator without explicit creator-role transfer; no automatic owner election.';
