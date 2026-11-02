-- E-12 S047: immutable canonical purchase identity on authorized paid issues.
-- Existing free issues keep purchase_id NULL; the paid path rechecks D.
ALTER TABLE text_issues ADD COLUMN purchase_id BIGINT
    REFERENCES chapter_purchase_facts(id) ON DELETE RESTRICT;
COMMENT ON COLUMN text_issues.purchase_id IS
    'D-authorized paid purchase identity at text issue submission; NULL for free.';
