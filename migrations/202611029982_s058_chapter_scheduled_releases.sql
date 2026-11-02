-- S058: durable first-release schedule; scheduling is never publication.
CREATE TABLE chapter_scheduled_releases (
    chapter_id BIGINT PRIMARY KEY REFERENCES chapters(id) ON DELETE RESTRICT,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    owner_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    version_id BIGINT NOT NULL REFERENCES chapter_versions(id) ON DELETE RESTRICT,
    expected_revision BIGINT NOT NULL CHECK (expected_revision > 0),
    scheduled_at TIMESTAMPTZ NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending','claimed','published','cancelled','stale')),
    claim_generation BIGINT NOT NULL DEFAULT 0 CHECK (claim_generation >= 0),
    lease_until TIMESTAMPTZ,
    published_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT scheduled_claim_lease CHECK (
       (status = 'claimed') = (lease_until IS NOT NULL)
    )
);
CREATE INDEX scheduled_chapter_due_idx
 ON chapter_scheduled_releases(scheduled_at,chapter_id)
 WHERE status IN ('pending','claimed');
