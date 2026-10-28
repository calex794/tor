-- B-10 / S030: versioned Book economics, independent from creator ACL.
-- No migration of old purchase earnings or inferred historical percentages.
-- Books created before this migration have NO authoritative share schedule
-- until explicitly attested by a later B-owned migration or owner command.
CREATE TABLE revenue_share_versions (
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    revision BIGINT NOT NULL CHECK (revision >= 1),
    changed_by_user_id BIGINT REFERENCES users(id) ON DELETE RESTRICT,
    reason TEXT NOT NULL CHECK (
        char_length(reason) BETWEEN 1 AND 160 AND reason=btrim(reason)
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (book_id, revision)
);

CREATE TABLE revenue_share_assignments (
    book_id BIGINT NOT NULL,
    revision BIGINT NOT NULL,
    user_id BIGINT NOT NULL,
    share_percent SMALLINT NOT NULL CHECK (share_percent BETWEEN 1 AND 100),
    PRIMARY KEY (book_id, revision, user_id),
    FOREIGN KEY (book_id,revision)
        REFERENCES revenue_share_versions(book_id,revision) ON DELETE RESTRICT,
    FOREIGN KEY (book_id,user_id)
        REFERENCES book_members(book_id,user_id) ON DELETE RESTRICT
);
CREATE INDEX revenue_share_assignments_recipient
    ON revenue_share_assignments(user_id,book_id,revision);

-- An INSERT into an existing current/previous revision is also a mutation
-- of economic history. Lock state so a simultaneous pointer advance cannot
-- race this check under READ COMMITTED. A new future revision is populated
-- before its state pointer advances in the same B-owned transaction.
CREATE FUNCTION b10_reject_late_assignment_append()
RETURNS trigger LANGUAGE plpgsql AS $
DECLARE
    current_rev BIGINT;
BEGIN
    SELECT current_revision INTO current_rev FROM revenue_share_states
    WHERE book_id=NEW.book_id FOR SHARE;
    IF current_rev IS NOT NULL AND current_rev>=NEW.revision THEN
        RAISE EXCEPTION 'cannot append to an activated revenue share revision'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$;
CREATE TRIGGER b10_sealed_revision_lines
    BEFORE INSERT ON revenue_share_assignments
    FOR EACH ROW EXECUTE FUNCTION b10_reject_late_assignment_append();


CREATE TABLE revenue_share_states (
    book_id BIGINT PRIMARY KEY REFERENCES books(id) ON DELETE RESTRICT,
    current_revision BIGINT NOT NULL CHECK (current_revision >= 1),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (book_id,current_revision)
        REFERENCES revenue_share_versions(book_id,revision) ON DELETE RESTRICT
);

-- Every committed current schedule must contain exactly 100% of eligible
-- contributor shares. DEFERRED permits writing an entire new version inside
-- one transaction without exposing an intermediate 80/110% allocation.
CREATE FUNCTION b10_validate_current_share_total()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    actual_total BIGINT;
    eligible_count BIGINT;
    assignment_count BIGINT;
BEGIN
    SELECT COALESCE(SUM(a.share_percent),0),
           COUNT(*) FILTER (WHERE NOT m.is_active),
           COUNT(*)
    INTO actual_total,eligible_count,assignment_count
    FROM revenue_share_assignments a
    JOIN book_members m ON m.book_id=a.book_id AND m.user_id=a.user_id
    WHERE a.book_id=NEW.book_id AND a.revision=NEW.current_revision;
    IF actual_total <> 100 OR eligible_count <> 0 OR assignment_count=0 THEN
        RAISE EXCEPTION 'invalid current Book share schedule'
            USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER b10_current_schedule_conservation
    AFTER INSERT OR UPDATE ON revenue_share_states
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION b10_validate_current_share_total();

CREATE FUNCTION b10_prevent_revision_rewind()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.book_id <> OLD.book_id
       OR NEW.current_revision <> OLD.current_revision + 1 THEN
        RAISE EXCEPTION 'share revision must advance by exactly one'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER b10_monotonic_state
    BEFORE UPDATE ON revenue_share_states
    FOR EACH ROW EXECUTE FUNCTION b10_prevent_revision_rewind();

-- Book ownership seeding runs in the earlier b09_* AFTER INSERT trigger.
-- The later b10_* trigger initializes only NEW owned books; legacy Books
-- remain unverified and paid checkout must not guess their historical split.
CREATE FUNCTION b10_seed_owner_revenue_shares()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO revenue_share_versions(book_id,revision,changed_by_user_id,reason)
    VALUES (NEW.id,1,NEW.owner_user_id,'owner_bootstrap');
    INSERT INTO revenue_share_assignments(book_id,revision,user_id,share_percent)
    VALUES (NEW.id,1,NEW.owner_user_id,100);
    INSERT INTO revenue_share_states(book_id,current_revision)
    VALUES (NEW.id,1);
    RETURN NEW;
END;
$$;
CREATE TRIGGER b10_seed_revenue_share_after_book_insert
    AFTER INSERT ON books
    FOR EACH ROW WHEN (NEW.owner_user_id IS NOT NULL)
    EXECUTE FUNCTION b10_seed_owner_revenue_shares();

-- Staffing is not financial authority. A member with positive current
-- economic rights cannot be deactivated until the entire split is replaced.
CREATE FUNCTION b10_keep_active_revenue_recipients()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.is_active AND NOT NEW.is_active
       AND EXISTS (
         SELECT 1
         FROM revenue_share_states s
         JOIN revenue_share_assignments a
           ON a.book_id=s.book_id AND a.revision=s.current_revision
         WHERE s.book_id=OLD.book_id AND a.user_id=OLD.user_id
       ) THEN
        RAISE EXCEPTION 'reassign revenue shares before deactivating contributor'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER b10_revenue_recipient_deactivation_fence
    BEFORE UPDATE OF is_active ON book_members
    FOR EACH ROW EXECUTE FUNCTION b10_keep_active_revenue_recipients();

-- Immutable economic provenance for a direct paid Chapter. Percentages
-- describe the AUTHOR POOL, not a payout or gross-money credit. Platform
-- commission, tax, rounding and cash settlement belong to a future D slice.
CREATE TABLE chapter_purchase_share_snapshots (
    purchase_id BIGINT PRIMARY KEY
        REFERENCES chapter_purchase_facts(id) ON DELETE RESTRICT,
    book_id BIGINT NOT NULL,
    share_revision BIGINT NOT NULL,
    gross_amount_minor BIGINT NOT NULL CHECK (gross_amount_minor>0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (book_id,share_revision)
        REFERENCES revenue_share_versions(book_id,revision) ON DELETE RESTRICT
);

CREATE TABLE chapter_purchase_share_lines (
    purchase_id BIGINT NOT NULL
        REFERENCES chapter_purchase_share_snapshots(purchase_id) ON DELETE RESTRICT,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    share_percent SMALLINT NOT NULL CHECK (share_percent BETWEEN 1 AND 100),
    PRIMARY KEY (purchase_id,user_id)
);

-- A separately forged snapshot may not claim another Book, gross amount,
-- or a stale revision. The checkout holds a shared Book lock while posting.
CREATE FUNCTION b10_check_purchase_snapshot()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM chapter_purchase_facts p
        JOIN chapters c ON c.id=p.chapter_id
        JOIN revenue_share_states s
          ON s.book_id=c.book_id AND s.current_revision=NEW.share_revision
        WHERE p.id=NEW.purchase_id AND c.book_id=NEW.book_id
          AND p.amount_minor=NEW.gross_amount_minor
    ) THEN
        RAISE EXCEPTION 'unverified purchase share snapshot'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER b10_purchase_snapshot_source
    BEFORE INSERT ON chapter_purchase_share_snapshots
    FOR EACH ROW EXECUTE FUNCTION b10_check_purchase_snapshot();

-- Only exact immutable revision lines may be stored. This prevents
-- substituting percentages during future payout reconciliation.
CREATE FUNCTION b10_check_purchase_share_line()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
      SELECT 1 FROM chapter_purchase_share_snapshots s
      JOIN revenue_share_assignments a
        ON a.book_id=s.book_id AND a.revision=s.share_revision
      WHERE s.purchase_id=NEW.purchase_id AND a.user_id=NEW.user_id
        AND a.share_percent=NEW.share_percent
    ) THEN
        RAISE EXCEPTION 'purchase line differs from frozen share revision'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER b10_purchase_line_exact
    BEFORE INSERT ON chapter_purchase_share_lines
    FOR EACH ROW EXECUTE FUNCTION b10_check_purchase_share_line();

-- A snapshot must enumerate the COMPLETE frozen revision exactly once.
-- Deferred to transaction commit because D writes the header before its lines.
CREATE FUNCTION b10_verify_snapshot_lines_complete()
RETURNS trigger LANGUAGE plpgsql AS $
DECLARE
    actual_count BIGINT;
    expected_count BIGINT;
    actual_total BIGINT;
BEGIN
    SELECT COUNT(*), COALESCE(SUM(share_percent),0)
      INTO actual_count,actual_total
    FROM chapter_purchase_share_lines WHERE purchase_id=NEW.purchase_id;
    SELECT COUNT(*) INTO expected_count
    FROM revenue_share_assignments
    WHERE book_id=NEW.book_id AND revision=NEW.share_revision;
    IF actual_count<>expected_count OR actual_total<>100 THEN
        RAISE EXCEPTION 'incomplete immutable purchase share snapshot'
            USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$;
CREATE CONSTRAINT TRIGGER b10_complete_purchase_snapshot
    AFTER INSERT ON chapter_purchase_share_snapshots
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION b10_verify_snapshot_lines_complete();

-- Versions, assignments and purchase snapshots cannot be rewritten after
-- commit. A later economics revision does not mutate earlier purchases.
CREATE TRIGGER b10_versions_immutable
    BEFORE UPDATE OR DELETE ON revenue_share_versions
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
CREATE TRIGGER b10_assignments_immutable
    BEFORE UPDATE OR DELETE ON revenue_share_assignments
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
CREATE TRIGGER b10_purchase_snapshots_immutable
    BEFORE UPDATE OR DELETE ON chapter_purchase_share_snapshots
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
CREATE TRIGGER b10_purchase_share_lines_immutable
    BEFORE UPDATE OR DELETE ON chapter_purchase_share_lines
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
