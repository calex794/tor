-- REC-POLICY-01B / F: one versioned, global ORGANIC shelf policy.
-- No rollout flag, viewer event, advertisement accounting, or Book ownership change.
-- Writes are limited to an administrator session through a single SQL CAS function.
CREATE TABLE public.rec_shelf_share_policy (
    singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
    revision bigint NOT NULL DEFAULT 1 CHECK(revision > 0),
    author_pct smallint NOT NULL DEFAULT 50 CHECK(author_pct BETWEEN 0 AND 100),
    teammate_pct smallint NOT NULL DEFAULT 40 CHECK(teammate_pct BETWEEN 0 AND 100),
    other_authors_pct smallint NOT NULL DEFAULT 10 CHECK(other_authors_pct BETWEEN 0 AND 100),
    no_team_author_pct smallint NOT NULL DEFAULT 70 CHECK(no_team_author_pct BETWEEN 0 AND 100),
    no_team_other_authors_pct smallint NOT NULL DEFAULT 30
        CHECK(no_team_other_authors_pct BETWEEN 0 AND 100),
    updated_by bigint,
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CHECK(author_pct + teammate_pct + other_authors_pct = 100),
    CHECK(no_team_author_pct + no_team_other_authors_pct = 100),
    CHECK(updated_by IS NULL OR updated_by > 0)
);

INSERT INTO public.rec_shelf_share_policy(singleton) VALUES(true);

-- Each row is the exact policy snapshot at a revision, not a reader impression.
-- Keep historical actor IDs even if the account is later removed; never cascade
-- updates into an immutable administrative audit.
CREATE TABLE public.rec_shelf_share_policy_audit (
    revision bigint PRIMARY KEY CHECK(revision > 0),
    author_pct smallint NOT NULL CHECK(author_pct BETWEEN 0 AND 100),
    teammate_pct smallint NOT NULL CHECK(teammate_pct BETWEEN 0 AND 100),
    other_authors_pct smallint NOT NULL CHECK(other_authors_pct BETWEEN 0 AND 100),
    no_team_author_pct smallint NOT NULL CHECK(no_team_author_pct BETWEEN 0 AND 100),
    no_team_other_authors_pct smallint NOT NULL
        CHECK(no_team_other_authors_pct BETWEEN 0 AND 100),
    actor_user_id bigint,
    action text NOT NULL CHECK(action IN ('bootstrap','set')),
    changed_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CHECK(author_pct + teammate_pct + other_authors_pct = 100),
    CHECK(no_team_author_pct + no_team_other_authors_pct = 100),
    CHECK((action='bootstrap' AND actor_user_id IS NULL AND revision=1)
       OR (action='set' AND actor_user_id > 0 AND revision>1))
);

INSERT INTO public.rec_shelf_share_policy_audit
    (revision,author_pct,teammate_pct,other_authors_pct,
     no_team_author_pct,no_team_other_authors_pct,actor_user_id,action)
VALUES(1,50,40,10,70,30,NULL,'bootstrap');

-- A: resolve the current, non-revoked real session and active administrator
-- under shared row locks. A caller-supplied user_id or role claim is never trusted.
CREATE FUNCTION public.rec_shelf_share_administrator() RETURNS bigint
LANGUAGE plpgsql SET search_path=pg_catalog,public AS $$
DECLARE digest bytea; actor bigint; locked_actor bigint;
BEGIN
    BEGIN
        digest := decode(current_setting('tor.rec_share_session',true),'hex');
    EXCEPTION WHEN others THEN
        RAISE EXCEPTION 'recommendation administrator required' USING ERRCODE='42501';
    END;
    IF digest IS NULL OR octet_length(digest) <> 32 THEN
        RAISE EXCEPTION 'recommendation administrator required' USING ERRCODE='42501';
    END IF;
    SELECT s.user_id INTO actor FROM public.sessions s WHERE s.token_hash=digest;
    SELECT u.id INTO locked_actor FROM public.users u
      WHERE u.id=actor AND u.account_state='active' FOR SHARE;
    IF locked_actor IS NULL THEN
        RAISE EXCEPTION 'recommendation administrator required' USING ERRCODE='42501';
    END IF;
    SELECT s.user_id INTO locked_actor FROM public.sessions s
      WHERE s.token_hash=digest AND s.user_id=actor AND s.revoked_at IS NULL
        AND s.expires_at>clock_timestamp() FOR SHARE;
    IF locked_actor IS NULL THEN
        RAISE EXCEPTION 'recommendation administrator required' USING ERRCODE='42501';
    END IF;
    SELECT r.user_id INTO locked_actor FROM public.account_roles r
      WHERE r.user_id=actor AND r.role='administrator' AND r.is_active FOR SHARE;
    IF locked_actor IS NULL THEN
        RAISE EXCEPTION 'recommendation administrator required' USING ERRCODE='42501';
    END IF;
    RETURN actor;
END $$;

-- Even the owner of a direct table write cannot delete the policy or
-- silently change a revision without recording an audit snapshot.
CREATE FUNCTION public.rec_shelf_share_policy_guard() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public AS $$
DECLARE actor bigint;
BEGIN
    IF TG_OP <> 'UPDATE' THEN
        RAISE EXCEPTION 'recommendation policy is a permanent singleton'
            USING ERRCODE='42501';
    END IF;
    actor := public.rec_shelf_share_administrator();
    IF NEW.singleton IS DISTINCT FROM OLD.singleton
        OR NEW.revision IS DISTINCT FROM OLD.revision + 1
        OR NEW.updated_by IS DISTINCT FROM actor
        OR NEW.updated_at < OLD.updated_at THEN
        RAISE EXCEPTION 'invalid recommendation policy revision'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER rec_shelf_share_policy_update_guard
    BEFORE INSERT OR UPDATE OR DELETE ON public.rec_shelf_share_policy
    FOR EACH ROW EXECUTE FUNCTION public.rec_shelf_share_policy_guard();

CREATE FUNCTION public.rec_shelf_share_audit_immutable() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public AS $$
BEGIN
    -- An INSERT is only valid from the policy's AFTER UPDATE trigger.
    IF TG_OP <> 'INSERT' OR pg_trigger_depth() <> 2 THEN
        RAISE EXCEPTION 'recommendation policy audit is append-only'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER rec_shelf_share_audit_immutability
    BEFORE INSERT OR UPDATE OR DELETE ON public.rec_shelf_share_policy_audit
    FOR EACH ROW EXECUTE FUNCTION public.rec_shelf_share_audit_immutable();

CREATE FUNCTION public.rec_shelf_share_audit_update() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public AS $$
BEGIN
    INSERT INTO public.rec_shelf_share_policy_audit
        (revision,author_pct,teammate_pct,other_authors_pct,
         no_team_author_pct,no_team_other_authors_pct,actor_user_id,action,changed_at)
    VALUES(NEW.revision,NEW.author_pct,NEW.teammate_pct,NEW.other_authors_pct,
           NEW.no_team_author_pct,NEW.no_team_other_authors_pct,NEW.updated_by,
           'set',NEW.updated_at);
    RETURN NULL;
END $$;
CREATE TRIGGER rec_shelf_share_audit_after_update
    AFTER UPDATE ON public.rec_shelf_share_policy
    FOR EACH ROW EXECUTE FUNCTION public.rec_shelf_share_audit_update();

-- Exactly one winner can apply a given expected_revision. No partial writes:
-- a policy update and its audit row commit in the same transaction.
CREATE FUNCTION public.rec_shelf_share_update(
    p_expected_revision bigint,
    p_author_pct integer,
    p_teammate_pct integer,
    p_other_authors_pct integer,
    p_no_team_author_pct integer,
    p_no_team_other_authors_pct integer
) RETURNS bigint
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,public AS $$
DECLARE actor bigint; next_revision bigint;
BEGIN
    actor := public.rec_shelf_share_administrator();
    IF p_expected_revision IS NULL OR p_expected_revision < 1
       OR p_author_pct IS NULL OR p_author_pct NOT BETWEEN 0 AND 100
       OR p_teammate_pct IS NULL OR p_teammate_pct NOT BETWEEN 0 AND 100
       OR p_other_authors_pct IS NULL OR p_other_authors_pct NOT BETWEEN 0 AND 100
       OR p_no_team_author_pct IS NULL OR p_no_team_author_pct NOT BETWEEN 0 AND 100
       OR p_no_team_other_authors_pct IS NULL
       OR p_no_team_other_authors_pct NOT BETWEEN 0 AND 100
       OR p_author_pct::bigint + p_teammate_pct + p_other_authors_pct <> 100
       OR p_no_team_author_pct::bigint + p_no_team_other_authors_pct <> 100 THEN
        RAISE EXCEPTION 'invalid recommendation policy shares'
            USING ERRCODE='22023';
    END IF;
    UPDATE public.rec_shelf_share_policy
       SET revision=revision+1,
           author_pct=p_author_pct,teammate_pct=p_teammate_pct,
           other_authors_pct=p_other_authors_pct,
           no_team_author_pct=p_no_team_author_pct,
           no_team_other_authors_pct=p_no_team_other_authors_pct,
           updated_by=actor,updated_at=clock_timestamp()
     WHERE singleton AND revision=p_expected_revision
     RETURNING revision INTO next_revision;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'stale recommendation policy revision'
            USING ERRCODE='P4090';
    END IF;
    RETURN next_revision;
END $$;

-- The serving role can read both snapshots but can only write by calling
-- the guarded SECURITY DEFINER CAS. No direct INSERT/UPDATE/DELETE rights.
REVOKE ALL ON TABLE public.rec_shelf_share_policy FROM PUBLIC;
REVOKE ALL ON TABLE public.rec_shelf_share_policy_audit FROM PUBLIC;
REVOKE ALL ON FUNCTION public.rec_shelf_share_update(bigint,integer,integer,integer,integer,integer)
    FROM PUBLIC;

-- The migration-only smoke installs schema before provisioning the runtime
-- login role. Never create a role or assign broad privileges in a migration.
DO $
BEGIN
    IF EXISTS(SELECT 1 FROM pg_catalog.pg_roles WHERE rolname='tor_app') THEN
        REVOKE ALL ON TABLE public.rec_shelf_share_policy FROM tor_app;
        REVOKE ALL ON TABLE public.rec_shelf_share_policy_audit FROM tor_app;
        GRANT SELECT ON TABLE public.rec_shelf_share_policy TO tor_app;
        GRANT SELECT ON TABLE public.rec_shelf_share_policy_audit TO tor_app;
        GRANT EXECUTE ON FUNCTION
            public.rec_shelf_share_update(bigint,integer,integer,integer,integer,integer)
            TO tor_app;
    END IF;
END $;
REVOKE ALL ON FUNCTION public.rec_shelf_share_administrator() FROM PUBLIC;
REVOKE ALL ON FUNCTION public.rec_shelf_share_policy_guard() FROM PUBLIC;
REVOKE ALL ON FUNCTION public.rec_shelf_share_audit_immutable() FROM PUBLIC;
REVOKE ALL ON FUNCTION public.rec_shelf_share_audit_update() FROM PUBLIC;

COMMENT ON TABLE public.rec_shelf_share_policy IS
    'One global, revisioned REC organic-only quota policy; no flags or reader activity';
COMMENT ON TABLE public.rec_shelf_share_policy_audit IS
    'Append-only administrative policy snapshots; each successful CAS creates one row';
