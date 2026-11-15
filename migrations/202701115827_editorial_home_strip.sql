-- PROMO-P1 / F+I: new nonfinancial home position, no new authority or billing.
-- Keep the original editorial campaign, revision and audit invariants.
ALTER TABLE editorial_campaigns
    DROP CONSTRAINT editorial_campaigns_slot_check;
ALTER TABLE editorial_campaigns
    ADD CONSTRAINT editorial_campaigns_slot_check
    CHECK (slot IN ('home_featured','home_promo_strip','book_detail_related','chapter_end_free'));

-- FLOW anonymous observations bind the editorial slot twice: in the short-lived
-- proof and in its aggregated hourly projection. Update both atomically.
-- The existing three slots and organic (empty slot) remain legal.
ALTER TABLE flow_observation_proofs
    DROP CONSTRAINT flow_observation_proofs_slot_check;
ALTER TABLE flow_observation_proofs
    ADD CONSTRAINT flow_observation_proofs_slot_check
    CHECK (slot IN ('', 'home_featured', 'home_promo_strip',
                   'book_detail_related', 'chapter_end_free'));

ALTER TABLE flow_observation_hourly
    DROP CONSTRAINT flow_observation_hourly_slot_check;
ALTER TABLE flow_observation_hourly
    ADD CONSTRAINT flow_observation_hourly_slot_check
    CHECK (slot IN ('', 'home_featured', 'home_promo_strip',
                   'book_detail_related', 'chapter_end_free'));
