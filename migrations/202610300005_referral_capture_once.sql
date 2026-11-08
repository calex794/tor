-- F-09/S052: one guest capture cannot sponsor multiple subject accounts.
-- Legacy/replayed outbox rows are allowed to settle as unattributed.
-- This is an attribution constraint, not a commission or wallet ledger.
CREATE UNIQUE INDEX referral_attributions_one_subject_per_capture
    ON referral_attributions(source_capture_id)
    WHERE source_capture_id IS NOT NULL;
