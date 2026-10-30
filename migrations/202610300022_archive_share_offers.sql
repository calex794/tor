-- D/ARCHIVE-01: owner-approved archival sales by an existing purchaser.
-- Retired is NOT hard deletion: public catalog stays hidden, pinned rights survive.
-- Safe default: no book can sell through archive invitations without explicit opt-in.
ALTER TABLE books
    ADD COLUMN archived_at TIMESTAMPTZ,
    ADD COLUMN archive_invite_sales_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    ADD CONSTRAINT archive_sales_requires_retirement
      CHECK (NOT archive_invite_sales_enabled OR archived_at IS NOT NULL),
    ADD CONSTRAINT retired_books_are_not_public
      CHECK (archived_at IS NULL OR visibility='draft');

-- One opaque offer identifies an exact owned historical edition.
-- A purchase through this offer does NOT transfer the sender's entitlement.
CREATE TABLE archive_share_offers (
    token UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    sender_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    sender_purchase_id BIGINT NOT NULL,
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    quoted_price_minor BIGINT NOT NULL CHECK (quoted_price_minor>0),
    parent_offer_token UUID REFERENCES archive_share_offers(token) ON DELETE RESTRICT,
    recipient_user_id BIGINT REFERENCES users(id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT (now()+INTERVAL '14 days'),
    revoked_at TIMESTAMPTZ,
    FOREIGN KEY (sender_purchase_id,sender_user_id,chapter_id,version_id)
        REFERENCES chapter_purchase_facts(id,user_id,chapter_id,version_id)
        ON DELETE RESTRICT,
    CHECK (recipient_user_id IS NULL OR recipient_user_id<>sender_user_id),
    CHECK (expires_at>created_at)
);
CREATE INDEX archive_share_offers_sender
    ON archive_share_offers(sender_user_id,created_at DESC);

-- Payment provenance is immutable and separate from signup referral attribution.
-- It allows B's new purchase to become an independent share source for C.
CREATE TABLE archive_purchase_sources (
    purchase_id BIGINT PRIMARY KEY REFERENCES chapter_purchase_facts(id) ON DELETE RESTRICT,
    offer_token UUID NOT NULL REFERENCES archive_share_offers(token) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Replays with the same (buyer,command key) cannot switch between a public
-- purchase and an archive purchase, or between distinct invitations.
ALTER TABLE chapter_purchase_commands
    ADD COLUMN archive_offer_token UUID REFERENCES archive_share_offers(token) ON DELETE RESTRICT;

CREATE FUNCTION archive_share_offer_revoke_only()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF to_jsonb(NEW) - 'revoked_at' IS DISTINCT FROM to_jsonb(OLD) - 'revoked_at'
       OR OLD.revoked_at IS NOT NULL OR NEW.revoked_at IS NULL THEN
        RAISE EXCEPTION 'archive offer is immutable except first revocation'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER archive_offer_mutation_guard
    BEFORE UPDATE ON archive_share_offers
    FOR EACH ROW EXECUTE FUNCTION archive_share_offer_revoke_only();

CREATE TRIGGER archive_purchase_source_immutable
    BEFORE UPDATE OR DELETE ON archive_purchase_sources
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
