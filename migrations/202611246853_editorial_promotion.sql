-- F/I: editorial promotion is an explicit nonfinancial source, not a zero-price ad.
CREATE TABLE editorial_campaigns (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    book_id bigint NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    slot text NOT NULL CHECK(slot IN ('home_featured','book_detail_related','chapter_end_free')),
    status text NOT NULL DEFAULT 'draft' CHECK(status IN ('draft','active','paused','ended')),
    starts_at timestamptz NOT NULL,
    ends_at timestamptz NOT NULL,
    priority smallint NOT NULL CHECK(priority BETWEEN 0 AND 100),
    revision bigint NOT NULL DEFAULT 1 CHECK(revision>0),
    created_by bigint NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK(ends_at>starts_at AND ends_at<=starts_at+interval '366 days')
);
CREATE INDEX editorial_serving ON editorial_campaigns(slot,priority DESC,id)
    INCLUDE(book_id,revision,starts_at,ends_at) WHERE status='active';
CREATE INDEX editorial_book_campaigns ON editorial_campaigns(book_id,slot,id);

-- Administration receipts and audit contain no reader identities or browsing events.
CREATE TABLE editorial_commands (
    actor_id bigint NOT NULL,
    command_key bytea NOT NULL CHECK(octet_length(command_key)=32),
    payload_hash bytea NOT NULL CHECK(octet_length(payload_hash)=32),
    campaign_id bigint NOT NULL CHECK(campaign_id>0),
    revision bigint NOT NULL CHECK(revision>0),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(actor_id,command_key)
);
CREATE TABLE editorial_audit (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_id bigint NOT NULL,
    campaign_id bigint NOT NULL,
    revision bigint NOT NULL,
    action text NOT NULL CHECK(action IN ('create','update','activate','pause','end')),
    payload_hash bytea NOT NULL CHECK(octet_length(payload_hash)=32),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE(campaign_id,revision)
);

-- I-authorized narrow A boundary: session and role are checked under shared locks
-- in the same transaction as an editorial change. Public-qualified tables reject
-- temporary-table shadowing. No role claim or user ID from a browser is trusted.
CREATE FUNCTION editorial_administrator() RETURNS bigint
LANGUAGE plpgsql SET search_path=pg_catalog,public AS $$
DECLARE digest bytea; actor bigint; locked_actor bigint;
BEGIN
    BEGIN
        digest := decode(current_setting('tor.editorial_session',true),'hex');
    EXCEPTION WHEN others THEN
        RAISE EXCEPTION 'editorial administrator required' USING ERRCODE='42501';
    END;
    IF digest IS NULL OR octet_length(digest)<>32 THEN
        RAISE EXCEPTION 'editorial administrator required' USING ERRCODE='42501';
    END IF;
    SELECT s.user_id INTO actor FROM public.sessions s WHERE s.token_hash=digest;
    SELECT u.id INTO locked_actor FROM public.users u
      WHERE u.id=actor AND u.account_state='active' FOR SHARE;
    IF locked_actor IS NULL THEN
        RAISE EXCEPTION 'editorial administrator required' USING ERRCODE='42501';
    END IF;
    SELECT s.user_id INTO locked_actor FROM public.sessions s
      WHERE s.token_hash=digest AND s.user_id=actor AND s.revoked_at IS NULL
        AND s.expires_at>clock_timestamp() FOR SHARE;
    IF locked_actor IS NULL THEN
        RAISE EXCEPTION 'editorial administrator required' USING ERRCODE='42501';
    END IF;
    SELECT r.user_id INTO locked_actor FROM public.account_roles r
      WHERE r.user_id=actor AND r.role='administrator' AND r.is_active FOR SHARE;
    IF locked_actor IS NULL THEN
        RAISE EXCEPTION 'editorial administrator required' USING ERRCODE='42501';
    END IF;
    RETURN actor;
END $$;

CREATE FUNCTION editorial_mutation_guard() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public AS $$
DECLARE actor bigint;
BEGIN
    actor := public.editorial_administrator();
    IF TG_TABLE_NAME='editorial_campaigns' THEN
        IF TG_OP='INSERT' AND NEW.created_by<>actor THEN
            RAISE EXCEPTION 'editorial actor mismatch' USING ERRCODE='42501';
        END IF;
        IF TG_OP='UPDATE' AND (NEW.book_id<>OLD.book_id OR NEW.slot<>OLD.slot
          OR NEW.created_by<>OLD.created_by OR NEW.created_at<>OLD.created_at
          OR NEW.revision<>OLD.revision+1 OR OLD.status='ended') THEN
            RAISE EXCEPTION 'invalid editorial revision' USING ERRCODE='23514';
        END IF;
    ELSIF NEW.actor_id<>actor THEN
        RAISE EXCEPTION 'editorial actor mismatch' USING ERRCODE='42501';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER editorial_campaign_authority BEFORE INSERT OR UPDATE ON editorial_campaigns
    FOR EACH ROW EXECUTE FUNCTION editorial_mutation_guard();
CREATE TRIGGER editorial_command_authority BEFORE INSERT ON editorial_commands
    FOR EACH ROW EXECUTE FUNCTION editorial_mutation_guard();
CREATE TRIGGER editorial_audit_authority BEFORE INSERT ON editorial_audit
    FOR EACH ROW EXECUTE FUNCTION editorial_mutation_guard();
CREATE FUNCTION editorial_immutable_record() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'editorial administrative record is immutable' USING ERRCODE='23514'; END $$;
CREATE TRIGGER editorial_commands_immutable BEFORE UPDATE OR DELETE ON editorial_commands
    FOR EACH ROW EXECUTE FUNCTION editorial_immutable_record();
CREATE TRIGGER editorial_audit_immutable BEFORE UPDATE OR DELETE ON editorial_audit
    FOR EACH ROW EXECUTE FUNCTION editorial_immutable_record();

-- The only physical removal belongs to B's source deletion cascade. Admins end
-- assignments, preserving command/audit history; raw DELETE cannot skip it.
CREATE FUNCTION editorial_source_delete_only() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public AS $$
BEGIN
    IF EXISTS(SELECT 1 FROM public.books WHERE id=OLD.book_id) THEN
        RAISE EXCEPTION 'end editorial assignment instead of deleting it' USING ERRCODE='42501';
    END IF;
    RETURN OLD;
END $$;
CREATE TRIGGER editorial_campaign_source_delete BEFORE DELETE ON editorial_campaigns
    FOR EACH ROW EXECUTE FUNCTION editorial_source_delete_only();
