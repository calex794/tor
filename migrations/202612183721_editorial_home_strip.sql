-- PROMO-P1 / F+I: new nonfinancial home position, no new authority or billing.
-- Keep the original editorial campaign, revision and audit invariants.
ALTER TABLE editorial_campaigns
    DROP CONSTRAINT editorial_campaigns_slot_check;
ALTER TABLE editorial_campaigns
    ADD CONSTRAINT editorial_campaigns_slot_check
    CHECK (slot IN ('home_featured','home_promo_strip','book_detail_related','chapter_end_free'));
