-- D-06: prepare optional purchase-time policy evidence. This migration does
-- NOT approve a refund policy, publish a refund window or enable refunds.
-- Existing and future S031 checkout writers omit both values until a later,
-- separately authorized policy catalog + purchase-time binding exists.
--
-- IMPORTANT: commerce_layer_terms is already immutable (D-05 trigger).
-- A missing policy CANNOT be backfilled after a term has been committed.
ALTER TABLE commerce_layer_terms
    ADD COLUMN refund_policy_version TEXT,
    ADD COLUMN refund_deadline_at TIMESTAMPTZ,
    ADD CONSTRAINT d06_term_refund_snapshot_complete CHECK (
        (refund_policy_version IS NULL AND refund_deadline_at IS NULL)
        OR (
            refund_policy_version IS NOT NULL
            AND refund_deadline_at IS NOT NULL
            AND char_length(btrim(refund_policy_version)) BETWEEN 1 AND 160
            AND refund_policy_version = btrim(refund_policy_version)
            AND refund_deadline_at >= purchased_at
        )
    );

-- Do not add defaults, synthesize a "no refunds" version, or assume that a
-- non-NULL identifier proves human approval. A later authorized migration
-- must link version IDs to an immutable approved catalog and snapshot them in
-- the SAME transaction as checkout, with C's first-access CAS and D's
-- serializable refund decision/reversal separately reviewed.
