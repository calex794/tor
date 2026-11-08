-- E-11 / S051: explicit status is independent of the administrative read cursor.
-- No data is rewritten except defaulting existing conversations to open.
ALTER TABLE support_conversations
    ADD COLUMN status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','closed')),
    ADD COLUMN status_revision BIGINT NOT NULL DEFAULT 0 CHECK (status_revision>=0),
    ADD COLUMN closed_at TIMESTAMPTZ,
    ADD CONSTRAINT support_conversations_close_consistent CHECK (
        (status='open' AND closed_at IS NULL) OR
        (status='closed' AND closed_at IS NOT NULL)
    );
