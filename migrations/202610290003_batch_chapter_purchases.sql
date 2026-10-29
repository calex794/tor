-- D/S035: all-or-nothing internal-wallet purchase of 2..20 paid chapters.
-- This records only chapter purchases, not external settlement or creator income.
-- Each purchase continues to have its own immutable wallet journal and B-10 share snapshot.
CREATE TABLE chapter_batch_commands (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    command_key TEXT NOT NULL CHECK (char_length(command_key) = 43),
    request_sha256 BYTEA NOT NULL CHECK (octet_length(request_sha256) = 32),
    item_count SMALLINT NOT NULL CHECK (item_count BETWEEN 2 AND 20),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, command_key)
);

CREATE TABLE chapter_batch_items (
    user_id BIGINT NOT NULL,
    command_key TEXT NOT NULL,
    position SMALLINT NOT NULL CHECK (position BETWEEN 1 AND 20),
    purchase_id BIGINT NOT NULL UNIQUE,
    PRIMARY KEY (user_id, command_key, position),
    FOREIGN KEY (user_id, command_key)
        REFERENCES chapter_batch_commands(user_id, command_key) ON DELETE RESTRICT,
    FOREIGN KEY (purchase_id, user_id)
        REFERENCES chapter_purchase_facts(id, user_id) ON DELETE RESTRICT
);

-- The batch row is claimed first, and all purchased items and journal entries
-- are posted before the same transaction commits. A crash halfway through
-- must not leave a successful but incomplete batch command.
CREATE FUNCTION d_s035_verify_complete_batch()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE actual_count BIGINT;
BEGIN
    SELECT count(*) INTO actual_count
    FROM chapter_batch_items i
    JOIN chapter_purchase_facts p
      ON p.id=i.purchase_id AND p.user_id=i.user_id
    JOIN reader_wallet_entries e
      ON e.purchase_id=p.id AND e.user_id=p.user_id
     AND e.kind='chapter_purchase'
     AND e.delta_minor=-p.amount_minor
     AND e.reference='batch-wallet:'||NEW.user_id::TEXT||':'||
                     NEW.command_key||':'||i.position::TEXT
    JOIN chapter_purchase_share_snapshots s
      ON s.purchase_id=p.id AND s.gross_amount_minor=p.amount_minor
    WHERE i.user_id=NEW.user_id AND i.command_key=NEW.command_key;
    IF actual_count <> NEW.item_count THEN
        RAISE EXCEPTION 'incomplete batch purchase or mismatched wallet evidence'
            USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER d_s035_batch_complete
    AFTER INSERT ON chapter_batch_commands
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION d_s035_verify_complete_batch();

CREATE TRIGGER d_s035_commands_immutable
    BEFORE UPDATE OR DELETE ON chapter_batch_commands
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();

CREATE TRIGGER d_s035_items_immutable
    BEFORE UPDATE OR DELETE ON chapter_batch_items
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
